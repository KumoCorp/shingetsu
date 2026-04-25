use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{parse2, Attribute, Ident, ImplItem, ImplItemFn, ItemImpl, LitStr, Meta, Type};

use crate::util::{
    gen_call_body, gen_call_body_styled, gen_param_specs, inner_return_type, is_result_return,
    parse_params, strip_attr, CratePath, ErrorStyle, ParamKind,
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
                    ::shingetsu::Bytes::from(&[ #(#name_bytes),* ][..])
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
    return_type: Box<syn::Type>,
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
            let mm: ::std::result::Result<shingetsu_meta::MetaMethod, _> = variant.parse();
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

pub fn expand_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse optional `crate = "path"` and `index_fallback = "nil"` from
    // the attribute.
    let mut krate = CratePath::default();
    let mut index_fallback_nil = false;
    let mut lua_rename: Option<String> = None;
    if !attr.is_empty() {
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("crate") {
                let val: LitStr = meta.value()?.parse()?;
                krate = CratePath::from_str(&val.value()).map_err(|e| {
                    syn::Error::new(val.span(), format!("invalid crate path: {}", e))
                })?;
                Ok(())
            } else if meta.path.is_ident("index_fallback") {
                let val: LitStr = meta.value()?.parse()?;
                if val.value() == "nil" {
                    index_fallback_nil = true;
                    Ok(())
                } else {
                    Err(syn::Error::new(
                        val.span(),
                        "only `index_fallback = \"nil\"` is supported",
                    ))
                }
            } else if meta.path.is_ident("rename") {
                let val: LitStr = meta.value()?.parse()?;
                lua_rename = Some(val.value());
                Ok(())
            } else {
                Err(meta.error(
                    "unknown attribute key; expected `crate`, `rename`, or `index_fallback`",
                ))
            }
        });
        if let Err(e) = syn::parse::Parser::parse2(parser, attr) {
            return e.into_compile_error();
        }
    }

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
    let self_ty = impl_block.self_ty.clone();

    let mut methods: Vec<MethodInfo> = Vec::new();
    let mut fields: Vec<FieldInfo> = Vec::new();
    let mut metamethods: Vec<MetamethodInfo> = Vec::new();

    // Scan impl items, collecting annotated methods and stripping their attrs.
    // The Lua-facing type name: use `rename = "..."` if provided,
    // otherwise fall back to the Rust struct name.
    let lua_type_name_str = lua_rename.as_deref().unwrap_or(&type_name_str);

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
            let return_type = inner_return_type(&f.sig.output);
            methods.push(MethodInfo {
                ident: f.sig.ident.clone(),
                lua_name,
                is_async,
                is_result,
                params,
                return_type,
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
    let index_arms = gen_index_arms(&type_name_str, &self_ty, &fields, &methods, &krate);
    // Generate __newindex arms for field setters.
    let newindex_arms = gen_newindex_arms(&type_name_str, &fields, &krate);
    // Generate direct metamethod arms.
    let meta_arms = gen_meta_arms(&type_name_str, &metamethods, &krate);
    let index_fallback_nil = index_fallback_nil;

    // Always generate type_name in the trait impl.
    // Use the Lua-facing name if the user defined one, otherwise struct name.
    let type_name_impl = quote! {
        fn type_name(&self) -> &'static str {
            #lua_type_name_str
        }
    };
    let lua_type_name_bytes = lua_type_name_str.as_bytes().to_vec();

    let has_index = !index_arms.is_empty();
    let has_newindex = !newindex_arms.is_empty();

    let k = krate.tokens();

    // Generate lua_type_info() override that returns a structural table type.
    let lua_type_info_impl = gen_lua_type_info(&methods, &fields, &krate);

    let index_fallback = if index_fallback_nil {
        quote! {
            _ => Ok(#k::valuevec![#k::Value::Nil])
        }
    } else {
        quote! {
            _ => Err(#k::VmError::HostError {
                name: ::std::format!("{}:__index", self.type_name()),
                source: ::std::format!(
                    "unknown field '{}'",
                    ::std::string::String::from_utf8_lossy(&__key_bytes)
                ).into(),
            })
        }
    };

    let index_dispatch = if has_index {
        quote! {
            "__index" => {
                let __key = __args.get(1).cloned().unwrap_or(#k::Value::Nil);
                let __key_bytes = match &__key {
                    #k::Value::String(s) => s.clone(),
                    _ => return Err(#k::VmError::BadArgument {
                        position: 2,
                        function: ::std::format!("{}:__index", self.type_name()),
                        expected: "string".to_owned(),
                        got: __key.type_name().to_owned(),
                    }),
                };
                match __key_bytes.as_ref() {
                    #(#index_arms)*
                    #index_fallback
                }
            }
        }
    } else {
        quote! {}
    };

    let newindex_dispatch = if has_newindex {
        quote! {
            "__newindex" => {
                let __key = __args.get(1).cloned().unwrap_or(#k::Value::Nil);
                let __val = __args.get(2).cloned().unwrap_or(#k::Value::Nil);
                let __key_bytes = match &__key {
                    #k::Value::String(s) => s.clone(),
                    _ => return Err(#k::VmError::BadArgument {
                        position: 2,
                        function: ::std::format!("{}:__newindex", self.type_name()),
                        expected: "string".to_owned(),
                        got: __key.type_name().to_owned(),
                    }),
                };
                match __key_bytes.as_ref() {
                    #(#newindex_arms)*
                    _ => Err(#k::VmError::HostError {
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

    // Build the sync `index()` override: handle sync getters and
    // cached sync methods.  Async items return None (fall through).
    let sync_index_impl = if has_index {
        let sync_index_arms =
            gen_sync_index_arms(&type_name_str, &self_ty, &fields, &methods, &krate);
        let has_async_index_items =
            fields.iter().any(|f| !f.is_setter && f.is_async) || methods.iter().any(|m| m.is_async);
        let sync_index_fallback = if index_fallback_nil && !has_async_index_items {
            quote! { _ => ::std::option::Option::Some(Ok(#k::valuevec![#k::Value::Nil])) }
        } else {
            quote! { _ => ::std::option::Option::None }
        };
        quote! {
            fn index(&self, __key: &#k::Value) -> ::std::option::Option<::std::result::Result<#k::ValueVec, #k::VmError>> {
                let __key_bytes = match __key {
                    #k::Value::String(s) => s,
                    _ => return ::std::option::Option::None,
                };
                match __key_bytes.as_ref() {
                    #(#sync_index_arms)*
                    #sync_index_fallback
                }
            }
        }
    } else {
        quote! {}
    };

    // Build the sync `newindex()` override for sync setters.
    let sync_newindex_impl = if has_newindex {
        let sync_newindex_arms = gen_sync_newindex_arms(&type_name_str, &self_ty, &fields, &krate);
        if sync_newindex_arms.is_empty() {
            quote! {}
        } else {
            quote! {
                fn newindex(&self, __key: &#k::Value, __val: &#k::Value) -> ::std::option::Option<::std::result::Result<#k::ValueVec, #k::VmError>> {
                    let __key_bytes = match __key {
                        #k::Value::String(s) => s,
                        _ => return ::std::option::Option::None,
                    };
                    match __key_bytes.as_ref() {
                        #(#sync_newindex_arms)*
                        _ => ::std::option::Option::None,
                    }
                }
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #impl_block

        #[#k::async_trait::async_trait]
        impl #k::Userdata for #self_ty {
            #type_name_impl

            #lua_type_info_impl

            #sync_index_impl

            #sync_newindex_impl

            async fn dispatch(
                self: ::std::sync::Arc<Self>,
                __ctx: #k::CallContext,
                metamethod: &str,
                __args: #k::ValueVec,
            ) -> ::std::result::Result<#k::ValueVec, #k::VmError> {
                match metamethod {
                    #index_dispatch
                    #newindex_dispatch
                    #(#meta_arms)*
                    _ => Err(#k::VmError::HostError {
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

        impl #k::LuaTyped for #self_ty {
            fn lua_type() -> #k::LuaType {
                #k::LuaType::Named(
                    #k::Bytes::from(&[ #(#lua_type_name_bytes),* ][..])
                )
            }
        }

    }
}

// ---------------------------------------------------------------------------
// Code generation helpers
// ---------------------------------------------------------------------------

/// Generate arms for the sync `index(&self, key)` override.
/// Sync field getters call the getter directly; sync methods return
/// the static-cached Function.  Async items are omitted (return None
/// at runtime to fall through to async dispatch).
fn gen_sync_index_arms(
    _type_name: &str,
    self_ty: &Type,
    fields: &[FieldInfo],
    methods: &[MethodInfo],
    krate: &CratePath,
) -> Vec<TokenStream> {
    let k = krate.tokens();
    let mut arms = Vec::new();

    // Sync getter fields — call the getter on &self directly.
    for f in fields.iter().filter(|f| !f.is_setter && !f.is_async) {
        let key = f.lua_name.as_bytes().to_vec();
        let ident = &f.ident;
        let body = if f.is_result {
            quote! { self.#ident().map(|__v| #k::IntoLuaMulti::into_lua_multi(__v)).map_err(|__e| <#k::VmError as ::std::convert::From<_>>::from(__e)) }
        } else {
            quote! { Ok(#k::IntoLuaMulti::into_lua_multi(self.#ident())) }
        };
        arms.push(quote! {
            &[ #(#key),* ] => ::std::option::Option::Some(#body),
        });
    }

    // Sync methods — return the static-cached Function (same statics
    // as gen_index_arms, which live inside the async dispatch path;
    // here we reference a *separate* set of per-method statics).
    for m in methods.iter().filter(|m| !m.is_async) {
        let key = m.lua_name.as_bytes().to_vec();
        let name_bytes = m.lua_name.as_bytes().to_vec();
        let ident = &m.ident;
        let params = &m.params;
        let is_result = m.is_result;
        let return_type = &m.return_type;
        let (param_specs, has_variadic) = gen_param_specs(params, krate);
        let source = format!("=[sync_index]");
        let source_bytes = source.as_bytes().to_vec();
        let call_recv = quote! { __self.#ident };
        let body = gen_call_body_styled(
            call_recv,
            params,
            false,
            is_result,
            ErrorStyle::BadArgument,
            true,
            krate,
        );

        let type_error_msg = _type_name.to_string();
        arms.push(quote! {
            &[ #(#key),* ] => {
                static __CACHED: ::std::sync::LazyLock<#k::Function> =
                    ::std::sync::LazyLock::new(|| {
                        #k::Function::native(#k::NativeFunction {
                            signature: ::std::sync::Arc::new(#k::FunctionSignature {
                                name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                                source: #k::Bytes::from(&[ #(#source_bytes),* ][..]),
                                type_params: ::std::vec::Vec::new(),
                                params: #param_specs,
                                variadic: #has_variadic,
                                arg_offset: 1,
                                returns: None,
                                lua_returns: ::std::option::Option::Some(
                                    <#return_type as #k::LuaTypedMulti>::lua_types()
                                ),
                                line_defined: 0,
                                last_line_defined: 0,
                                num_upvalues: 0,
                            }),
                            call: #k::NativeCall::SyncWithCtx(::std::sync::Arc::new(|__ctx, __args| {
                                let __self: ::std::sync::Arc<#self_ty> = match __args.first() {
                                    ::std::option::Option::Some(#k::Value::Userdata(__u)) => {
                                        let __u: ::std::sync::Arc<dyn #k::Userdata> =
                                            ::std::sync::Arc::clone(__u)
                                                as ::std::sync::Arc<dyn #k::Userdata>;
                                        __u.downcast_arc::<#self_ty>().ok()
                                    }
                                    _ => None,
                                }.ok_or_else(|| #k::VmError::BadArgument {
                                    position: 1,
                                    function: __ctx.native_name.as_ref()
                                        .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                                        .unwrap_or_default(),
                                    expected: #type_error_msg.to_owned(),
                                    got: __args.first()
                                        .map(|v| v.type_name().to_owned())
                                        .unwrap_or_else(|| "no value".to_owned()),
                                })?;
                                let __args = &__args[1..];
                                #body
                            })),
                        })
                    });
                ::std::option::Option::Some(Ok(#k::valuevec![#k::Value::Function((*__CACHED).clone())]))
            }
        });
    }

    arms
}

/// Generate arms for the sync `newindex(&self, key, value)` override.
fn gen_sync_newindex_arms(
    type_name: &str,
    _self_ty: &Type,
    fields: &[FieldInfo],
    krate: &CratePath,
) -> Vec<TokenStream> {
    let k = krate.tokens();
    fields
        .iter()
        .filter(|f| f.is_setter && !f.is_async)
        .map(|f| {
            let key = f.lua_name.as_bytes().to_vec();
            let ident = &f.ident;
            let ctx_name = format!("{}.{}", type_name, f.lua_name);
            let ctx_name_bytes = ctx_name.as_bytes().to_vec();
            let body = gen_call_body_styled(
                quote! { self.#ident },
                &f.params,
                false,
                f.is_result,
                ErrorStyle::FieldAssignment,
                true,
                krate,
            );
            quote! {
                &[ #(#key),* ] => {
                    let __ctx = #k::CallContext::new(
                        #k::GlobalEnv::new(),
                        #k::CallStack::new(),
                        ::std::option::Option::Some(
                            #k::Bytes::from(&[ #(#ctx_name_bytes),* ][..])
                        ),
                    );
                    let __args: &[#k::Value] = ::std::slice::from_ref(__val);
                    ::std::option::Option::Some((|| { #body })())
                }
            }
        })
        .collect()
}

fn gen_index_arms(
    type_name: &str,
    self_ty: &Type,
    fields: &[FieldInfo],
    methods: &[MethodInfo],
    krate: &CratePath,
) -> Vec<TokenStream> {
    let k = krate.tokens();
    let source = format!("=[{type_name}]");
    let source_bytes = source.as_bytes().to_vec();
    let mut arms = Vec::new();

    // Getter fields.
    for f in fields.iter().filter(|f| !f.is_setter) {
        let key = f.lua_name.as_bytes().to_vec();
        let ident = &f.ident;
        let ctx_name = format!("{}.{}", type_name, f.lua_name);
        let ctx_name_bytes = ctx_name.as_bytes().to_vec();
        let body = gen_call_body(
            quote! { self.#ident },
            &f.params,
            f.is_async,
            f.is_result,
            krate,
        );
        arms.push(quote! {
            &[ #(#key),* ] => {
                let __ctx = {
                    let mut __c = __ctx.clone();
                    __c.native_name = ::std::option::Option::Some(
                        #k::Bytes::from(&[ #(#ctx_name_bytes),* ][..])
                    );
                    __c
                };
                #body
            }
        });
    }

    // Methods — return a NativeFunction for each method.
    // Sync methods use a static LazyLock cache: the closure extracts
    // `self` from args[0] via downcast_ref instead of capturing an
    // Arc, so the Function is instance-independent and created once.
    for m in methods {
        let key = m.lua_name.as_bytes().to_vec();
        let name_bytes = m.lua_name.as_bytes().to_vec();
        let ident = &m.ident;
        let params = &m.params;
        let is_async = m.is_async;
        let is_result = m.is_result;

        let return_type = &m.return_type;
        let (param_specs, has_variadic) = gen_param_specs(params, krate);

        if is_async {
            let self_clone = quote! { let __self = ::std::sync::Arc::clone(&self); };
            let call_recv = quote! { __self.#ident };
            let body = gen_call_body(call_recv, params, true, is_result, krate);
            arms.push(quote! {
                &[ #(#key),* ] => {
                    #self_clone
                    let __f = #k::Function::native(#k::NativeFunction {
                        signature: ::std::sync::Arc::new(#k::FunctionSignature {
                            name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                            source: #k::Bytes::from(&[ #(#source_bytes),* ][..]),
                            type_params: ::std::vec::Vec::new(),
                            params: #param_specs,
                            variadic: #has_variadic,
                            arg_offset: 1,
                            returns: None,
                            lua_returns: ::std::option::Option::Some(
                                <#return_type as #k::LuaTypedMulti>::lua_types()
                            ),
                            line_defined: 0,
                            last_line_defined: 0,
                            num_upvalues: 0,
                        }),
                        call: #k::NativeCall::Async(::std::sync::Arc::new(move |__ctx, __args| {
                            let __self = ::std::sync::Arc::clone(&__self);
                            ::std::boxed::Box::pin(async move {
                                let mut __args = __args.into_iter();
                                let _ = __args.next();
                                #body
                            })
                        })),
                    });
                    Ok(#k::valuevec![#k::Value::Function(__f)])
                }
            });
        } else {
            // Sync method: cache the Function in a static LazyLock.
            // The closure extracts `self` from args[0] via downcast_ref,
            // so no instance-specific capture is needed.
            let call_recv = quote! { __self.#ident };
            let body = gen_call_body_styled(
                call_recv,
                params,
                false,
                is_result,
                ErrorStyle::BadArgument,
                true,
                krate,
            );
            let type_error_msg = type_name.to_string();
            arms.push(quote! {
                &[ #(#key),* ] => {
                    static __CACHED: ::std::sync::LazyLock<#k::Function> =
                        ::std::sync::LazyLock::new(|| {
                            #k::Function::native(#k::NativeFunction {
                                signature: ::std::sync::Arc::new(#k::FunctionSignature {
                                    name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                                    source: #k::Bytes::from(&[ #(#source_bytes),* ][..]),
                                    type_params: ::std::vec::Vec::new(),
                                    params: #param_specs,
                                    variadic: #has_variadic,
                                    arg_offset: 1,
                                    returns: None,
                                    lua_returns: ::std::option::Option::Some(
                                        <#return_type as #k::LuaTypedMulti>::lua_types()
                                    ),
                                    line_defined: 0,
                                    last_line_defined: 0,
                                    num_upvalues: 0,
                                }),
                                call: #k::NativeCall::SyncWithCtx(::std::sync::Arc::new(|__ctx, __args| {
                                    let __self: ::std::sync::Arc<#self_ty> = match __args.first() {
                                        ::std::option::Option::Some(#k::Value::Userdata(__u)) => {
                                            let __u: ::std::sync::Arc<dyn #k::Userdata> =
                                                ::std::sync::Arc::clone(__u)
                                                    as ::std::sync::Arc<dyn #k::Userdata>;
                                            __u.downcast_arc::<#self_ty>().ok()
                                        }
                                        _ => None,
                                    }.ok_or_else(|| #k::VmError::BadArgument {
                                        position: 1,
                                        function: __ctx.native_name.as_ref()
                                            .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                                            .unwrap_or_default(),
                                        expected: #type_error_msg.to_owned(),
                                        got: __args.first()
                                            .map(|v| v.type_name().to_owned())
                                            .unwrap_or_else(|| "no value".to_owned()),
                                    })?;
                                    let __args = &__args[1..];
                                    #body
                                })),
                            })
                        });
                    Ok(#k::valuevec![#k::Value::Function((*__CACHED).clone())])
                }
            });
        }
    }

    arms
}

fn gen_newindex_arms(type_name: &str, fields: &[FieldInfo], krate: &CratePath) -> Vec<TokenStream> {
    let k = krate.tokens();
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
                false,
                krate,
            );
            quote! {
                &[ #(#key),* ] => {
                    let __ctx = {
                        let mut __c = __ctx.clone();
                        __c.native_name = ::std::option::Option::Some(
                            #k::Bytes::from(&[ #(#ctx_name_bytes),* ][..])
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

fn gen_meta_arms(
    type_name: &str,
    metamethods: &[MetamethodInfo],
    krate: &CratePath,
) -> Vec<TokenStream> {
    let k = krate.tokens();
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

            let body = gen_call_body(call_recv, &m.params, is_async, is_result, krate);

            let is_binary_op = m
                .meta_name
                .parse::<shingetsu_meta::MetaMethod>()
                .map(|mm| mm.is_binary_op())
                .unwrap_or(false);

            let args_setup = if is_binary_op {
                // For binary ops the userdata may be either operand.
                // Determine which side self is on by pointer identity,
                // then keep only the other operand.
                quote! {
                    let __self_ptr = ::std::sync::Arc::as_ptr(&self) as *const ();
                    let __self_on_left = __args.first()
                        .map(|__v| matches!(
                            __v,
                            #k::Value::Userdata(ref __u)
                                if ::std::sync::Arc::as_ptr(__u) as *const () == __self_ptr
                        ))
                        .unwrap_or(false);
                    let mut __args_vec = __args;
                    __args_vec.retain(|__v| {
                        if let #k::Value::Userdata(ref __u) = __v {
                            ::std::sync::Arc::as_ptr(__u) as *const () != __self_ptr
                        } else {
                            true
                        }
                    });
                    let mut __args = __args_vec.into_iter();
                }
            } else {
                // Non-binary metamethods: self is always args[0].
                quote! {
                    let mut __args = __args.into_iter();
                    let _ = __args.next(); // skip self
                }
            };

            quote! {
                #name => {
                    let __ctx = {
                        let mut __c = __ctx.clone();
                        __c.native_name = ::std::option::Option::Some(
                            #k::Bytes::from(&[ #(#ctx_name_bytes),* ][..])
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

/// Generate the `lua_type_info` override that returns a `LuaType::Table`
/// with entries for each `#[lua_method]` and `#[lua_field]` getter.
fn gen_lua_type_info(
    methods: &[MethodInfo],
    fields: &[FieldInfo],
    krate: &CratePath,
) -> TokenStream {
    let k = krate.tokens();
    let mut field_entries = Vec::<TokenStream>::new();

    // Field getters contribute their return type.
    for f in fields.iter().filter(|f| !f.is_setter) {
        let name_bytes = f.lua_name.as_bytes().to_vec();
        // The field getter's Rust return type determines the Lua type.
        // We use the first Normal param's type if any (for setters),
        // but for getters we need the return type.  Since we don't
        // store it in FieldInfo, look up via the function's params.
        // Getters have no Lua-visible params — the type comes from
        // the function's return type, which we don't have here.
        // For now, use LuaType::Any for field getters.
        // TODO: capture getter return types in FieldInfo for richer types.
        field_entries.push(quote! {
            (
                #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                #k::LuaType::Any,
            )
        });
    }

    // Methods contribute a LuaType::Function with their param/return types.
    for m in methods {
        let name_bytes = m.lua_name.as_bytes().to_vec();
        let return_type = &m.return_type;

        // Build param types from the method's Lua-visible params.
        let mut param_type_entries = Vec::<TokenStream>::new();
        for p in &m.params {
            match p {
                ParamKind::Normal(ident, ty) => {
                    let name_str = ident.to_string();
                    let name_bytes = name_str.as_bytes().to_vec();
                    let lua_ty = crate::util::strip_reference(ty);
                    param_type_entries.push(quote! {
                        (
                            ::std::option::Option::Some(
                                #k::Bytes::from(&[ #(#name_bytes),* ][..])
                            ),
                            <#lua_ty as #k::LuaTyped>::lua_type(),
                        )
                    });
                }
                ParamKind::BinOpSide(_, _) => {}
                ParamKind::CallContext(_) | ParamKind::FrameLocals(_) => {}
                ParamKind::Variadic(_) | ParamKind::VariadicMulti(_, _) => {}
            }
        }

        let has_variadic = m
            .params
            .iter()
            .any(|p| matches!(p, ParamKind::Variadic(_) | ParamKind::VariadicMulti(_, _)));
        let variadic_expr = if has_variadic {
            quote! { ::std::option::Option::Some(::std::boxed::Box::new(#k::LuaType::Any)) }
        } else {
            quote! { ::std::option::Option::None }
        };

        field_entries.push(quote! {
            (
                #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                #k::LuaType::Function(::std::boxed::Box::new(#k::FunctionLuaType {
                    type_params: ::std::vec::Vec::new(),
                    params: ::std::vec![ #(#param_type_entries),* ],
                    variadic: #variadic_expr,
                    returns: <#return_type as #k::LuaTypedMulti>::lua_types(),
                    is_method: true,
                    inferred_unannotated: false,
                })),
            )
        });
    }

    // If no methods or fields, skip the override (default returns Named).
    if field_entries.is_empty() {
        return quote! {};
    }

    quote! {
        fn lua_type_info(&self) -> #k::LuaType {
            let mut fields = ::std::vec![ #(#field_entries),* ];
            fields.sort_by(|(a, _), (b, _)| a.cmp(b));
            #k::LuaType::Table(::std::boxed::Box::new(#k::TableLuaType {
                fields,
                indexer: ::std::option::Option::None,
            }))
        }
    }
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
