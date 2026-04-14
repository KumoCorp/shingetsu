use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{parse2, Attribute, Ident, ImplItem, ImplItemFn, ItemImpl, LitStr, Meta, Type};

use crate::util::{
    gen_call_body, gen_call_body_styled, gen_param_specs, is_result_return, parse_params,
    strip_attr, ErrorStyle, ParamKind,
};

// ---------------------------------------------------------------------------
// #[derive(UserData)]
// ---------------------------------------------------------------------------

/// Generates a minimal `Userdata` impl (default dispatch), `impl_downcast!`,
/// and `LuaTyped` for a struct with no annotated methods.
pub fn derive(input: TokenStream) -> TokenStream {
    let ast: syn::DeriveInput = match parse2(input) {
        Ok(v) => v,
        Err(e) => return e.into_compile_error(),
    };
    let name = &ast.ident;
    let name_str = name.to_string();
    let name_bytes = name_str.as_bytes().to_vec();
    quote! {
        #[::shingetsu::async_trait::async_trait]
        impl ::shingetsu::Userdata for #name {
            fn type_name(&self) -> &'static str {
                #name_str
            }
        }
        impl ::shingetsu::LuaTyped for #name {
            fn lua_type() -> ::shingetsu::LuaType {
                ::shingetsu::LuaType::Named(
                    ::shingetsu::bytes::Bytes::from_static(&[ #(#name_bytes),* ])
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// #[shingetsu::userdata] on impl blocks
// ---------------------------------------------------------------------------

struct MethodInfo {
    ident: Ident,
    lua_name: String,
    is_async: bool,
    is_result: bool,
    params: Vec<ParamKind>,
    is_arc_self: bool,
}

struct FieldInfo {
    ident: Ident,
    lua_name: String,
    is_setter: bool,
    is_async: bool,
    is_result: bool,
    params: Vec<ParamKind>,
}

struct MetamethodInfo {
    ident: Ident,
    meta_name: String,
    is_async: bool,
    is_result: bool,
    params: Vec<ParamKind>,
}

/// Parse the `rename = "x"` value from an attribute's args if present,
/// otherwise return the default name.
fn parse_rename(attr: &Attribute, default: &str) -> syn::Result<String> {
    if matches!(&attr.meta, Meta::Path(_)) {
        return Ok(default.to_owned());
    }
    let mut name = default.to_owned();
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("rename") {
            let val: LitStr = meta.value()?.parse()?;
            name = val.value();
            Ok(())
        } else {
            Err(meta.error("unknown attribute key"))
        }
    })?;
    Ok(name)
}

/// Parse the metamethod name from `#[lua_metamethod(Name)]` or
/// `#[lua_metamethod("__name")]`.
fn parse_metamethod_name(attr: &Attribute) -> syn::Result<String> {
    let mut result = None::<String>;
    attr.parse_nested_meta(|meta| {
        if let Some(ident) = meta.path.get_ident() {
            // e.g. #[lua_metamethod(Index)]
            let variant = ident.to_string();
            let mm: ::std::result::Result<shin_vm_meta::MetaMethod, _> = variant.parse();
            result = Some(
                mm.map(|m| m.name().to_owned())
                    .unwrap_or_else(|_| format!("__{}", variant.to_lowercase())),
            );
            Ok(())
        } else {
            // e.g. #[lua_metamethod("__index")]
            Err(meta.error("expected metamethod name"))
        }
    })?;
    // Also try parsing as a string literal directly
    if result.is_none() {
        if let Ok(lit) = attr.parse_args::<LitStr>() {
            result = Some(lit.value());
        }
    }
    result.ok_or_else(|| syn::Error::new_spanned(attr, "expected metamethod name"))
}

/// Returns `true` if the first non-Receiver param of a function is `Arc<Self>`.
fn has_arc_self(f: &ImplItemFn) -> bool {
    for arg in &f.sig.inputs {
        match arg {
            syn::FnArg::Receiver(_) => return false,
            syn::FnArg::Typed(pt) => {
                // Arc<Self> shows up as a typed arg with no receiver
                if let Type::Path(tp) = pt.ty.as_ref() {
                    if tp
                        .path
                        .segments
                        .last()
                        .map(|s| s.ident == "Arc")
                        .unwrap_or(false)
                    {
                        return true;
                    }
                }
                return false;
            }
        }
    }
    false
}

pub fn expand_impl(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut impl_block: ItemImpl = match parse2(item) {
        Ok(v) => v,
        Err(e) => return e.into_compile_error(),
    };

    // Derive the type name from `self_ty`.
    let type_name_ident = match impl_block.self_ty.as_ref() {
        Type::Path(tp) => tp.path.segments.last().map(|s| s.ident.clone()),
        _ => None,
    };
    let type_name_ident = match type_name_ident {
        Some(i) => i,
        None => {
            return syn::Error::new(Span::call_site(), "expected a simple type name")
                .into_compile_error()
        }
    };
    let type_name_str = type_name_ident.to_string();
    let type_name_bytes = type_name_str.as_bytes().to_vec();
    let self_ty = impl_block.self_ty.clone();

    let mut methods: Vec<MethodInfo> = Vec::new();
    let mut fields: Vec<FieldInfo> = Vec::new();
    let mut metamethods: Vec<MetamethodInfo> = Vec::new();

    // Scan impl items, collecting annotated methods and stripping their attrs.
    for item in &mut impl_block.items {
        let ImplItem::Fn(f) = item else { continue };
        // Skip `fn type_name` — the macro always generates it in the trait impl.
        if f.sig.ident == "type_name" {
            continue;
        }

        let is_async = f.sig.asyncness.is_some();
        let is_result = is_result_return(&f.sig.output);
        let arc_self = has_arc_self(f);

        if let Some(attr) = f
            .attrs
            .iter()
            .find(|a| a.path().is_ident("lua_method"))
            .cloned()
        {
            let lua_name = match parse_rename(&attr, &f.sig.ident.to_string()) {
                Ok(n) => n,
                Err(e) => return e.into_compile_error(),
            };
            // params: skip the Arc<Self> first arg (not a Receiver), then parse rest
            let params = if arc_self {
                // The first typed arg is `self: Arc<Self>` — skip it.
                let sig_without_self = skip_first_typed_param(&f.sig);
                parse_params(&sig_without_self)
            } else {
                parse_params(&f.sig)
            };
            methods.push(MethodInfo {
                ident: f.sig.ident.clone(),
                lua_name,
                is_async,
                is_result,
                params,
                is_arc_self: arc_self,
            });
            strip_attr(&mut f.attrs, "lua_method");
        } else if let Some(attr) = f
            .attrs
            .iter()
            .find(|a| a.path().is_ident("lua_field"))
            .cloned()
        {
            let fn_name = f.sig.ident.to_string();
            let is_setter =
                fn_name.strip_prefix("set_").is_some() || attr_has_flag(&attr, "setter");
            // Strip "set_" / "get_" prefixes so that `fn set_value` and
            // `fn get_value` both map to the Lua field name `"value"` by
            // default.  An explicit `rename = "…"` in the attribute overrides
            // the auto-derived name.
            let default_name = if let Some(n) = fn_name.strip_prefix("set_") {
                n.to_owned()
            } else if let Some(n) = fn_name.strip_prefix("get_") {
                n.to_owned()
            } else {
                fn_name.clone()
            };
            let lua_name = match parse_rename(&attr, &default_name) {
                Ok(n) => n,
                Err(e) => return e.into_compile_error(),
            };
            let params = parse_params(&f.sig);
            fields.push(FieldInfo {
                ident: f.sig.ident.clone(),
                lua_name,
                is_setter,
                is_async,
                is_result,
                params,
            });
            strip_attr(&mut f.attrs, "lua_field");
        } else if let Some(attr) = f
            .attrs
            .iter()
            .find(|a| a.path().is_ident("lua_metamethod"))
            .cloned()
        {
            let meta_name = match parse_metamethod_name(&attr) {
                Ok(n) => n,
                Err(e) => return e.into_compile_error(),
            };
            let params = if arc_self {
                let sig_without_self = skip_first_typed_param(&f.sig);
                parse_params(&sig_without_self)
            } else {
                parse_params(&f.sig)
            };
            metamethods.push(MetamethodInfo {
                ident: f.sig.ident.clone(),
                meta_name,
                is_async,
                is_result,
                params,
            });
            strip_attr(&mut f.attrs, "lua_metamethod");
        }
    }

    // Generate __index arms for fields (getters) and methods.
    let index_arms = gen_index_arms(&type_name_str, &fields, &methods);
    // Generate __newindex arms for field setters.
    let newindex_arms = gen_newindex_arms(&type_name_str, &fields);
    // Generate direct metamethod arms.
    let meta_arms = gen_meta_arms(&type_name_str, &metamethods);

    // Always generate type_name in the trait impl (derived from struct name).
    let type_name_impl = quote! {
        fn type_name(&self) -> &'static str {
            #type_name_str
        }
    };

    let has_index = !index_arms.is_empty();
    let has_newindex = !newindex_arms.is_empty();

    let index_dispatch = if has_index {
        quote! {
            "__index" => {
                let __key = __args.get(1).cloned().unwrap_or(::shingetsu::Value::Nil);
                let __key_bytes = match &__key {
                    ::shingetsu::Value::String(s) => s.clone(),
                    _ => return Err(::shingetsu::VmError::BadArgument {
                        position: 2,
                        function: ::std::format!("{}:__index", self.type_name()),
                        expected: "string".to_owned(),
                        got: __key.type_name().to_owned(),
                    }),
                };
                match __key_bytes.as_ref() {
                    #(#index_arms)*
                    _ => Err(::shingetsu::VmError::HostError {
                        name: ::std::format!("{}:__index", self.type_name()),
                        source: ::std::format!(
                            "unknown field '{}'",
                            ::std::string::String::from_utf8_lossy(&__key_bytes)
                        ).into(),
                    }),
                }
            }
        }
    } else {
        quote! {}
    };

    let newindex_dispatch = if has_newindex {
        quote! {
            "__newindex" => {
                let __key = __args.get(1).cloned().unwrap_or(::shingetsu::Value::Nil);
                let __val = __args.get(2).cloned().unwrap_or(::shingetsu::Value::Nil);
                let __key_bytes = match &__key {
                    ::shingetsu::Value::String(s) => s.clone(),
                    _ => return Err(::shingetsu::VmError::BadArgument {
                        position: 2,
                        function: ::std::format!("{}:__newindex", self.type_name()),
                        expected: "string".to_owned(),
                        got: __key.type_name().to_owned(),
                    }),
                };
                match __key_bytes.as_ref() {
                    #(#newindex_arms)*
                    _ => Err(::shingetsu::VmError::HostError {
                        name: ::std::format!("{}:__newindex", self.type_name()),
                        source: ::std::format!(
                            "unknown field '{}'",
                            ::std::string::String::from_utf8_lossy(&__key_bytes)
                        ).into(),
                    }),
                }
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #impl_block

        #[::shingetsu::async_trait::async_trait]
        impl ::shingetsu::Userdata for #self_ty {
            #type_name_impl

            async fn dispatch(
                self: ::std::sync::Arc<Self>,
                __ctx: ::shingetsu::CallContext,
                metamethod: &str,
                __args: ::std::vec::Vec<::shingetsu::Value>,
            ) -> ::std::result::Result<::std::vec::Vec<::shingetsu::Value>, ::shingetsu::VmError> {
                match metamethod {
                    #index_dispatch
                    #newindex_dispatch
                    #(#meta_arms)*
                    _ => Err(::shingetsu::VmError::HostError {
                        name: ::std::format!("{}:{}", self.type_name(), metamethod),
                        source: ::std::format!(
                            "metamethod '{}' not implemented for '{}'",
                            metamethod,
                            self.type_name()
                        ).into(),
                    }),
                }
            }
        }

        impl ::shingetsu::LuaTyped for #self_ty {
            fn lua_type() -> ::shingetsu::LuaType {
                ::shingetsu::LuaType::Named(
                    ::shingetsu::bytes::Bytes::from_static(&[ #(#type_name_bytes),* ])
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Code generation helpers
// ---------------------------------------------------------------------------

fn gen_index_arms(
    type_name: &str,
    fields: &[FieldInfo],
    methods: &[MethodInfo],
) -> Vec<TokenStream> {
    let mut arms = Vec::new();

    // Getter fields.
    for f in fields.iter().filter(|f| !f.is_setter) {
        let key = f.lua_name.as_bytes().to_vec();
        let ident = &f.ident;
        let ctx_name = format!("{}.{}", type_name, f.lua_name);
        let ctx_name_bytes = ctx_name.as_bytes().to_vec();
        let body = gen_call_body(quote! { self.#ident }, &f.params, f.is_async, f.is_result);
        arms.push(quote! {
            &[ #(#key),* ] => {
                let __ctx = {
                    let mut __c = __ctx.clone();
                    __c.native_name = ::std::option::Option::Some(
                        ::shingetsu::bytes::Bytes::from_static(&[ #(#ctx_name_bytes),* ])
                    );
                    __c
                };
                #body
            }
        });
    }

    // Methods — return a NativeFunction capturing Arc<Self>.
    for m in methods {
        let key = m.lua_name.as_bytes().to_vec();
        let name_bytes = m.lua_name.as_bytes().to_vec();
        let ident = &m.ident;
        let params = &m.params;
        let is_async = m.is_async;
        let is_result = m.is_result;
        let is_arc_self = m.is_arc_self;

        let self_clone = quote! { let __self = ::std::sync::Arc::clone(&self); };

        // Build the call expression.  For Arc<Self> methods the first arg is
        // `self: Arc<Self>`, so we pass `__self` as the explicit receiver.
        let (inner_self_skip, call_recv) = if is_arc_self {
            // Skip the first Lua arg (which is the object passed by Lua).
            let skip = quote! { let _ = __args.next(); };
            let recv = quote! { __self.#ident };
            (skip, recv)
        } else {
            (quote! { let _ = __args.next(); }, quote! { __self.#ident })
        };

        let body = gen_call_body(call_recv, params, is_async, is_result);
        let (param_specs, has_variadic) = gen_param_specs(params);

        arms.push(quote! {
            &[ #(#key),* ] => {
                #self_clone
                let __f = ::shingetsu::Function::native(::shingetsu::NativeFunction {
                    signature: ::std::sync::Arc::new(::shingetsu::FunctionSignature {
                        name: ::shingetsu::bytes::Bytes::from_static(&[ #(#name_bytes),* ]),
                        type_params: ::std::vec::Vec::new(),
                        params: #param_specs,
                        variadic: #has_variadic,
                        arg_offset: 1,
                        returns: None,
                        lua_returns: None,
                    }),
                    call: ::std::sync::Arc::new(move |__ctx, __args| {
                        let __self = ::std::sync::Arc::clone(&__self);
                        ::std::boxed::Box::pin(async move {
                            let mut __args = __args.into_iter();
                            #inner_self_skip
                            #body
                        })
                    }),
                });
                Ok(::std::vec![::shingetsu::Value::Function(__f)])
            }
        });
    }

    arms
}

fn gen_newindex_arms(type_name: &str, fields: &[FieldInfo]) -> Vec<TokenStream> {
    fields
        .iter()
        .filter(|f| f.is_setter)
        .map(|f| {
            let key = f.lua_name.as_bytes().to_vec();
            let ident = &f.ident;
            let is_async = f.is_async;
            let is_result = f.is_result;
            let ctx_name = format!("{}.{}", type_name, f.lua_name);
            let ctx_name_bytes = ctx_name.as_bytes().to_vec();
            // Setters take one Lua value argument (the new value).
            // We use __val directly since __newindex gives us [obj, key, val].
            let val_extraction = quote! {
                let mut __args = ::std::iter::once(__val);
            };
            let body = gen_call_body_styled(
                quote! { self.#ident },
                &f.params,
                is_async,
                is_result,
                ErrorStyle::FieldAssignment,
            );
            quote! {
                &[ #(#key),* ] => {
                    let __ctx = {
                        let mut __c = __ctx.clone();
                        __c.native_name = ::std::option::Option::Some(
                            ::shingetsu::bytes::Bytes::from_static(&[ #(#ctx_name_bytes),* ])
                        );
                        __c
                    };
                    #val_extraction
                    #body
                }
            }
        })
        .collect()
}

fn gen_meta_arms(type_name: &str, metamethods: &[MetamethodInfo]) -> Vec<TokenStream> {
    metamethods
        .iter()
        .map(|m| {
            let name = &m.meta_name;
            let ident = &m.ident;
            let is_async = m.is_async;
            let is_result = m.is_result;
            let ctx_name = format!("{}:{}", type_name, m.meta_name);
            let ctx_name_bytes = ctx_name.as_bytes().to_vec();

            let call_recv = quote! { self.#ident };

            let body = gen_call_body(call_recv, &m.params, is_async, is_result);
            // Create the iterator and skip the first Lua arg (the self/receiver
            // object that the VM passes as args[0] for all metamethods).
            let args_setup = quote! {
                let mut __args = __args.into_iter();
                let _ = __args.next(); // skip self
            };

            quote! {
                #name => {
                    let __ctx = {
                        let mut __c = __ctx.clone();
                        __c.native_name = ::std::option::Option::Some(
                            ::shingetsu::bytes::Bytes::from_static(&[ #(#ctx_name_bytes),* ])
                        );
                        __c
                    };
                    #args_setup
                    #body
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns `true` if the attribute contains a bare flag `name` in its args.
fn attr_has_flag(attr: &Attribute, flag: &str) -> bool {
    let mut found = false;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident(flag) {
            found = true;
        }
        Ok(())
    });
    found
}

/// Return a copy of `sig` with the first typed (non-receiver) parameter removed.
fn skip_first_typed_param(sig: &Signature) -> Signature {
    let mut sig = sig.clone();
    let mut skipped = false;
    sig.inputs = sig
        .inputs
        .into_iter()
        .filter(|arg| {
            if !skipped {
                if matches!(arg, FnArg::Typed(_)) {
                    skipped = true;
                    return false;
                }
            }
            true
        })
        .collect();
    sig
}

use syn::{FnArg, Signature};
