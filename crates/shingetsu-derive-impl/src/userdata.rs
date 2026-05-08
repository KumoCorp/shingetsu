use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{parse2, Attribute, Ident, ImplItem, ImplItemFn, ItemImpl, LitStr, Meta, Type};

use crate::util::{
    examples_vec_expr, gen_call_body, gen_call_body_styled, gen_function_signature,
    gen_param_specs, inner_return_type, is_result_return, opt_string_expr, parse_doc_block,
    parse_params, promote_last_normal_to_variadic, strip_attr, CratePath, ErrorStyle,
    FunctionNameSource, ParamKind, ParsedExample,
};
use std::collections::HashMap;

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

    let snapshot_opt = match parse_derive_userdata_attrs(&ast.attrs) {
        Ok(o) => o,
        Err(e) => return e.into_compile_error(),
    };
    let snapshot_impl = if snapshot_opt.snapshot {
        // Requires `Clone + IntoLua` on `#name`.  The closure clones
        // a stored value on each rebuild, leaving the snapshot itself
        // reusable across rebuilds.
        quote! {
            fn snapshot(&self) -> ::std::option::Option<::shingetsu::Snapshot> {
                let cloned = ::std::clone::Clone::clone(self);
                ::std::option::Option::Some(::shingetsu::Snapshot::new(
                    move |_env: &::shingetsu::GlobalEnv|
                        -> ::std::result::Result<::shingetsu::Value, ::shingetsu::VmError>
                    {
                        ::std::result::Result::Ok(
                            ::shingetsu::IntoLua::into_lua(
                                ::std::clone::Clone::clone(&cloned)
                            )
                        )
                    },
                ))
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #[::shingetsu::async_trait::async_trait]
        impl ::shingetsu::Userdata for #name {
            fn type_name(&self) -> &'static str {
                #name_str
            }
            #snapshot_impl
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
    /// `true` when the receiver is `&mut self`.  Drives the mlua
    /// facade's choice between `add_method` and `add_method_mut`.
    is_mut_self: bool,
    params: Vec<ParamKind>,
    return_type: Box<syn::Type>,
    doc: Option<String>,
    param_docs: HashMap<String, String>,
    returns_doc: Vec<String>,
    examples: Vec<ParsedExample>,
}

struct FieldInfo {
    ident: Ident,
    lua_name: String,
    is_setter: bool,
    is_async: bool,
    is_result: bool,
    params: Vec<ParamKind>,
    /// Return type of the getter (`None` for setter-only entries).
    return_type: Option<Box<syn::Type>>,
    doc: Option<String>,
    examples: Vec<ParsedExample>,
}

struct PairsMethod {
    ident: Ident,
    /// `true` when the user's method returns `Result<impl Iterator, VmError>`
    /// rather than `impl Iterator`.
    is_result: bool,
}

struct MetamethodInfo {
    ident: Ident,
    meta_name: String,
    is_async: bool,
    is_result: bool,
    /// `true` when the receiver is `&mut self`.  Drives the mlua
    /// facade's choice between `add_meta_method` and
    /// `add_meta_method_mut`.
    is_mut_self: bool,
    params: Vec<ParamKind>,
    return_type: Box<syn::Type>,
    doc: Option<String>,
    param_docs: HashMap<String, String>,
    returns_doc: Vec<String>,
    examples: Vec<ParsedExample>,
}

/// Parsed `#[lua(...)]` options at the `derive(UserData)` container
/// level.
struct DeriveUserDataOpts {
    /// `#[lua(snapshot)]` opts the type into
    /// [`shingetsu_vm::Userdata::snapshot`] via `Clone + IntoLua`.
    snapshot: bool,
}

fn parse_derive_userdata_attrs(attrs: &[Attribute]) -> syn::Result<DeriveUserDataOpts> {
    let mut opts = DeriveUserDataOpts { snapshot: false };
    for attr in attrs {
        if !attr.path().is_ident("lua") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("snapshot") {
                opts.snapshot = true;
                Ok(())
            } else {
                Err(meta.error(
                    "unknown lua container option for derive(UserData); expected `snapshot`",
                ))
            }
        })?;
    }
    Ok(opts)
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

/// Parsed options for `#[lua_method]` / `#[lua_metamethod]`: the
/// rename target and a `variadic` flag that promotes the last
/// `Normal` parameter to `VariadicMulti`.
struct MethodAttrOpts {
    lua_name: String,
    variadic: bool,
}

fn parse_method_opts(attr: &Attribute, default: &str) -> syn::Result<MethodAttrOpts> {
    let mut opts = MethodAttrOpts {
        lua_name: default.to_owned(),
        variadic: false,
    };
    if matches!(&attr.meta, Meta::Path(_)) {
        return Ok(opts);
    }
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("rename") {
            let val: LitStr = meta.value()?.parse()?;
            opts.lua_name = val.value();
            Ok(())
        } else if meta.path.is_ident("variadic") {
            opts.variadic = true;
            Ok(())
        } else {
            Err(meta.error("unknown attribute key; expected `rename` or `variadic`"))
        }
    })?;
    Ok(opts)
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

/// Returns `true` if the function's receiver is `&mut self`.
fn is_mut_self(f: &ImplItemFn) -> bool {
    matches!(
        f.sig.inputs.first(),
        Some(syn::FnArg::Receiver(syn::Receiver {
            mutability: Some(_),
            reference: Some(_),
            ..
        }))
    )
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

/// Wrapper that emits only the shingetsu-side `Userdata` impl.
pub fn expand_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_inner(attr, item, false)
}

/// Wrapper that emits the shingetsu-side `Userdata` impl plus the
/// mlua-side `UserData` impl, used by the migration facade.
pub fn expand_facade(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_inner(attr, item, true)
}

fn expand_inner(attr: TokenStream, item: TokenStream, also_emit_mlua: bool) -> TokenStream {
    // Parse optional `crate = "path"`, `index_fallback = "nil"`,
    // `rename = "..."`, and the `snapshot` flag from the attribute.
    let mut krate = CratePath::default();
    let mut index_fallback_nil = false;
    let mut lua_rename: Option<String> = None;
    let mut auto_snapshot = false;
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
            } else if meta.path.is_ident("snapshot") {
                auto_snapshot = true;
                Ok(())
            } else {
                Err(meta.error(
                    "unknown attribute key; expected `crate`, `rename`, \
                     `index_fallback`, or `snapshot`",
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
    let impl_doc = parse_doc_block(&impl_block.attrs).summary;

    let mut methods: Vec<MethodInfo> = Vec::new();
    let mut fields: Vec<FieldInfo> = Vec::new();
    let mut metamethods: Vec<MetamethodInfo> = Vec::new();
    // Identifier of the `#[lua_snapshot]`-marked method, when present.
    // The macro emits a `Userdata::snapshot` override that delegates
    // to it.  At most one is permitted per impl block.
    let mut snapshot_method: Option<Ident> = None;
    // Identifier of the `#[lua_pairs]`-marked method, when present.
    // The macro emits a synthesized `__pairs` metamethod that
    // calls this method to materialize the iterator, then stashes
    // it under a `parking_lot::Mutex` so each iter-fn invocation
    // can `.next()` it.
    let mut pairs_method: Option<PairsMethod> = None;

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
        let doc_block = parse_doc_block(&f.attrs);
        let per_arg_docs = crate::util::extract_and_strip_param_docs(&mut f.sig);

        if let Some(attr) = f
            .attrs
            .iter()
            .find(|a| a.path().is_ident("lua_method"))
            .cloned()
        {
            let opts = match parse_method_opts(&attr, &f.sig.ident.to_string()) {
                Ok(n) => n,
                Err(e) => return e.into_compile_error(),
            };
            let lua_name = opts.lua_name;
            // params: skip the Arc<Self> first arg (not a Receiver), then parse rest
            let mut params = if arc_self {
                // The first typed arg is `self: Arc<Self>` — skip it.
                let sig_without_self = skip_first_typed_param(&f.sig);
                parse_params(&sig_without_self)
            } else {
                parse_params(&f.sig)
            };
            if opts.variadic {
                promote_last_normal_to_variadic(&mut params);
            }
            let return_type = inner_return_type(&f.sig.output);
            methods.push(MethodInfo {
                ident: f.sig.ident.clone(),
                lua_name,
                is_async,
                is_result,
                is_mut_self: is_mut_self(f),
                params,
                return_type,
                doc: doc_block.summary,
                param_docs: crate::util::merge_param_docs(doc_block.params, per_arg_docs),
                returns_doc: doc_block.returns,
                examples: doc_block.examples,
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
            // Getter return type drives the field's Lua type for docgen
            // and the type checker.  Setters take their type from the
            // setter's input parameter, so we ignore the return there.
            let return_type = if is_setter {
                None
            } else {
                Some(inner_return_type(&f.sig.output))
            };
            fields.push(FieldInfo {
                ident: f.sig.ident.clone(),
                lua_name,
                is_setter,
                is_async,
                is_result,
                params,
                return_type,
                doc: doc_block.summary,
                examples: doc_block.examples,
            });
            strip_attr(&mut f.attrs, "lua_field");
        } else if f.attrs.iter().any(|a| a.path().is_ident("lua_pairs")) {
            if let Some(prior) = pairs_method.as_ref() {
                let prior_ident = &prior.ident;
                return syn::Error::new_spanned(
                    &f.sig.ident,
                    format!(
                        "duplicate `#[lua_pairs]` (prior was `{prior_ident}`); \
                         only one pairs method is permitted per impl block",
                    ),
                )
                .into_compile_error();
            }
            if is_async {
                return syn::Error::new_spanned(
                    &f.sig.ident,
                    "`#[lua_pairs]` does not yet support async methods; the iterator \
                     materializes synchronously when `__pairs` fires",
                )
                .into_compile_error();
            }
            pairs_method = Some(PairsMethod {
                ident: f.sig.ident.clone(),
                is_result,
            });
            strip_attr(&mut f.attrs, "lua_pairs");
        } else if f.attrs.iter().any(|a| a.path().is_ident("lua_snapshot")) {
            if let Some(prior) = snapshot_method.as_ref() {
                return syn::Error::new_spanned(
                    &f.sig.ident,
                    format!(
                        "duplicate `#[lua_snapshot]` (prior was `{prior}`); \
                         only one snapshot method is permitted per impl block",
                    ),
                )
                .into_compile_error();
            }
            snapshot_method = Some(f.sig.ident.clone());
            strip_attr(&mut f.attrs, "lua_snapshot");
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
            let return_type = inner_return_type(&f.sig.output);
            metamethods.push(MetamethodInfo {
                ident: f.sig.ident.clone(),
                meta_name,
                is_async,
                is_result,
                is_mut_self: is_mut_self(f),
                params,
                return_type,
                doc: doc_block.summary,
                param_docs: crate::util::merge_param_docs(doc_block.params, per_arg_docs),
                returns_doc: doc_block.returns,
                examples: doc_block.examples,
            });
            strip_attr(&mut f.attrs, "lua_metamethod");
        }
    }

    if let Some(p) = pairs_method.as_ref() {
        if metamethods.iter().any(|m| m.meta_name == "__pairs") {
            return syn::Error::new_spanned(
                &p.ident,
                "`#[lua_pairs]` cannot coexist with `#[lua_metamethod(Pairs)]`; \
                 choose one or the other",
            )
            .into_compile_error();
        }
    }

    // Generate __index arms for fields (getters) and methods.
    let index_arms = gen_index_arms(&type_name_str, &self_ty, &fields, &methods, &krate);
    // Generate __newindex arms for field setters.
    let newindex_arms = gen_newindex_arms(&type_name_str, &fields, &krate);
    // Generate direct metamethod arms.
    let mut meta_arms = gen_meta_arms(&type_name_str, &metamethods, &krate);
    if let Some(p) = pairs_method.as_ref() {
        meta_arms.push(gen_pairs_arm(&type_name_str, p, &krate));
    }
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
    let userdata_type_fn = gen_userdata_type_fn(
        lua_type_name_str,
        &impl_doc,
        &fields,
        &methods,
        &metamethods,
        &krate,
    );

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

    // Build the `invoke()` override: directly dispatch sync methods
    // without materialising a `Function` value or constructing a
    // `CallContext`.  Methods that need a `CallContext` (or other
    // ctx-dependent features like `FrameLocals` / `VariadicMulti`) are
    // skipped here — they fall through to the existing `index` path.
    let invoke_impl = {
        let invoke_arms = gen_invoke_arms(&type_name_str, &self_ty, &methods, &krate);
        if invoke_arms.is_empty() {
            quote! {}
        } else {
            quote! {
                fn invoke(
                    &self,
                    __method: &[u8],
                    __args: &[#k::Value],
                ) -> ::std::option::Option<::std::result::Result<#k::ValueVec, #k::VmError>> {
                    match __method {
                        #(#invoke_arms)*
                        _ => ::std::option::Option::None,
                    }
                }
            }
        }
    };

    // Build the `invoke_async()` override: directly dispatch async
    // methods, returning the future the VM yields on without first
    // materialising a `Function` value.  Each arm carries a per-method
    // static `FunctionSignature` for the `Native` stack frame entry.
    let invoke_async_impl = {
        let invoke_async_arms = gen_invoke_async_arms(&type_name_str, &self_ty, &methods, &krate);
        if invoke_async_arms.is_empty() {
            quote! {}
        } else {
            quote! {
                fn invoke_async(
                    self: ::std::sync::Arc<Self>,
                    __method: &[u8],
                    __args: #k::ValueVec,
                ) -> ::std::option::Option<(
                    ::std::sync::Arc<#k::FunctionSignature>,
                    #k::futures::future::BoxFuture<'static, ::std::result::Result<#k::ValueVec, #k::VmError>>,
                )> {
                    match __method {
                        #(#invoke_async_arms)*
                        _ => ::std::option::Option::None,
                    }
                }
            }
        }
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

    if auto_snapshot && snapshot_method.is_some() {
        return syn::Error::new_spanned(
            snapshot_method.as_ref().expect("just checked"),
            "`#[userdata(snapshot)]` and `#[lua_snapshot]` are mutually exclusive; \
             choose one or the other",
        )
        .into_compile_error();
    }
    let snapshot_impl = if let Some(name) = &snapshot_method {
        quote! {
            fn snapshot(&self) -> ::std::option::Option<#k::Snapshot> {
                ::std::option::Option::Some(self.#name())
            }
        }
    } else if auto_snapshot {
        // Auto-snapshot via `Self: Clone + IntoLua`.  Same shape as
        // `derive(UserData)`'s `#[lua(snapshot)]` container attr.
        quote! {
            fn snapshot(&self) -> ::std::option::Option<#k::Snapshot> {
                let __cloned = ::std::clone::Clone::clone(self);
                ::std::option::Option::Some(#k::Snapshot::new(
                    move |_env: &#k::GlobalEnv|
                        -> ::std::result::Result<#k::Value, #k::VmError>
                    {
                        ::std::result::Result::Ok(
                            #k::IntoLua::into_lua(::std::clone::Clone::clone(&__cloned))
                        )
                    },
                ))
            }
        }
    } else {
        quote! {}
    };

    // Build the set of metamethod names this userdata implements
    // for the `has_metamethod` override.  `__index` / `__newindex`
    // are included whenever the type has fields or methods; the
    // explicit `metamethods` and `pairs_method` contribute their
    // names directly.
    let mut mm_names: Vec<String> = metamethods.iter().map(|m| m.meta_name.clone()).collect();
    if pairs_method.is_some() {
        mm_names.push("__pairs".to_owned());
    }
    if has_index {
        mm_names.push("__index".to_owned());
    }
    if has_newindex {
        mm_names.push("__newindex".to_owned());
    }
    let has_metamethod_impl = if mm_names.is_empty() {
        quote! {}
    } else {
        quote! {
            fn has_metamethod(&self, __name: &str) -> bool {
                ::std::matches!(__name, #( #mm_names )|*)
            }
        }
    };

    let mlua_impl = if also_emit_mlua {
        gen_mlua_userdata_impl(
            &self_ty,
            &methods,
            &fields,
            &metamethods,
            &snapshot_method,
            auto_snapshot,
            &pairs_method,
        )
    } else {
        quote! {}
    };

    quote! {
        #impl_block

        #mlua_impl

        #[#k::async_trait::async_trait]
        impl #k::Userdata for #self_ty {
            #type_name_impl

            #lua_type_info_impl

            #snapshot_impl

            #has_metamethod_impl

            #invoke_impl

            #invoke_async_impl

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

        impl #self_ty {
            #userdata_type_fn
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
        let (param_specs, has_variadic, has_runtime_types) =
            gen_param_specs(params, krate, &Default::default());
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
            &FunctionNameSource::Dynamic,
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

                                variadic_doc: ::std::option::Option::None,
                                arg_offset: 1,
                                returns: None,
                                lua_returns: ::std::option::Option::Some(
                                    <#return_type as #k::LuaTypedMulti>::lua_types()
                                ),
                                line_defined: 0,
                                last_line_defined: 0,
                                num_upvalues: 0,
                                has_runtime_types: #has_runtime_types,
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

/// Whether a method's parameter list precludes the `Userdata::invoke`
/// fast paths.  Methods that take a `CallContext`, `FrameLocals`, or
/// `VariadicMulti` parameter can't run without setting up that runtime
/// state, so they fall through to the `index`-then-call path.
fn method_params_block_invoke_fast_path(params: &[ParamKind]) -> bool {
    params.iter().any(|p| {
        matches!(
            p,
            ParamKind::CallContext(_) | ParamKind::FrameLocals(_) | ParamKind::VariadicMulti(_, _)
        )
    })
}

/// Whether a method is eligible for the sync `Userdata::invoke` fast
/// path (`fn`, no `CallContext`/`FrameLocals`/`VariadicMulti`).
fn method_supports_invoke_fast_path(m: &MethodInfo) -> bool {
    !m.is_async && !method_params_block_invoke_fast_path(&m.params)
}

/// Whether a method is eligible for the async `Userdata::invoke_async`
/// fast path (`async fn`, no `CallContext`/`FrameLocals`/`VariadicMulti`).
fn method_supports_invoke_async_fast_path(m: &MethodInfo) -> bool {
    m.is_async && !method_params_block_invoke_fast_path(&m.params)
}

/// Generate match arms for the `Userdata::invoke` fast-path override.
///
/// Each eligible sync method gets an arm that downcasts `self` from
/// `args[0]`, threads explicit args from `args[1..]` through `FromLua`
/// conversion, calls the method body, and returns `ValueVec`.  No
/// `CallContext` is constructed; error messages use a static method
/// name literal supplied via `FunctionNameSource::Static`.
fn gen_invoke_arms(
    type_name: &str,
    self_ty: &Type,
    methods: &[MethodInfo],
    krate: &CratePath,
) -> Vec<TokenStream> {
    let k = krate.tokens();
    let type_error_msg = type_name.to_string();
    let mut arms = Vec::new();

    for m in methods
        .iter()
        .filter(|m| method_supports_invoke_fast_path(m))
    {
        let key = m.lua_name.as_bytes().to_vec();
        let lua_name_lit = syn::LitStr::new(&m.lua_name, proc_macro2::Span::call_site());
        let ident = &m.ident;
        let call_recv = quote! { __self.#ident };
        let body = gen_call_body_styled(
            call_recv,
            &m.params,
            false,
            m.is_result,
            ErrorStyle::BadArgument,
            true,
            &FunctionNameSource::Static(quote! { #lua_name_lit }),
            krate,
        );
        arms.push(quote! {
            &[ #(#key),* ] => {
                let __invoke = || -> ::std::result::Result<#k::ValueVec, #k::VmError> {
                    let __args_slice: &[#k::Value] = __args;
                    let __self: ::std::sync::Arc<#self_ty> = match __args_slice.first() {
                        ::std::option::Option::Some(#k::Value::Userdata(__u)) => {
                            let __u: ::std::sync::Arc<dyn #k::Userdata> =
                                ::std::sync::Arc::clone(__u)
                                    as ::std::sync::Arc<dyn #k::Userdata>;
                            __u.downcast_arc::<#self_ty>().ok()
                        }
                        _ => ::std::option::Option::None,
                    }.ok_or_else(|| #k::VmError::BadArgument {
                        position: 1,
                        function: #lua_name_lit.to_owned(),
                        expected: #type_error_msg.to_owned(),
                        got: __args_slice.first()
                            .map(|v| v.type_name().to_owned())
                            .unwrap_or_else(|| "no value".to_owned()),
                    })?;
                    let __args = &__args_slice[1..];
                    #body
                };
                ::std::option::Option::Some(__invoke())
            }
        });
    }
    arms
}

/// Generate match arms for the `Userdata::invoke_async` fast-path
/// override.
///
/// Each eligible async method gets an arm that takes ownership of `self`
/// (already an `Arc<Self>` from the trait method's receiver), constructs
/// a per-method static `FunctionSignature` (used by the VM to populate
/// the `Native` stack frame entry), and returns a boxed future that
/// runs the method body with arg conversion via the static-name error
/// path.
fn gen_invoke_async_arms(
    type_name: &str,
    _self_ty: &Type,
    methods: &[MethodInfo],
    krate: &CratePath,
) -> Vec<TokenStream> {
    let k = krate.tokens();
    let source = format!("=[{type_name}]");
    let source_bytes = source.as_bytes().to_vec();
    let mut arms = Vec::new();

    for m in methods
        .iter()
        .filter(|m| method_supports_invoke_async_fast_path(m))
    {
        let key = m.lua_name.as_bytes().to_vec();
        let name_bytes = m.lua_name.as_bytes().to_vec();
        let lua_name_lit = syn::LitStr::new(&m.lua_name, proc_macro2::Span::call_site());
        let ident = &m.ident;
        let params = &m.params;
        let is_result = m.is_result;
        let return_type = &m.return_type;
        let (param_specs, has_variadic, has_runtime_types) =
            gen_param_specs(params, krate, &Default::default());

        let call_recv = quote! { __self.#ident };
        let body = gen_call_body_styled(
            call_recv,
            params,
            true,
            is_result,
            ErrorStyle::BadArgument,
            false,
            &FunctionNameSource::Static(quote! { #lua_name_lit }),
            krate,
        );

        arms.push(quote! {
            &[ #(#key),* ] => {
                static __SIG: ::std::sync::LazyLock<::std::sync::Arc<#k::FunctionSignature>> =
                    ::std::sync::LazyLock::new(|| {
                        ::std::sync::Arc::new(#k::FunctionSignature {
                            name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                            source: #k::Bytes::from(&[ #(#source_bytes),* ][..]),
                            type_params: ::std::vec::Vec::new(),
                            params: #param_specs,
                            variadic: #has_variadic,

                            variadic_doc: ::std::option::Option::None,
                            arg_offset: 1,
                            returns: None,
                            lua_returns: ::std::option::Option::Some(
                                <#return_type as #k::LuaTypedMulti>::lua_types()
                            ),
                            line_defined: 0,
                            last_line_defined: 0,
                            num_upvalues: 0,
                            has_runtime_types: #has_runtime_types,
                        })
                    });
                let __self = self;
                let __sig = (*__SIG).clone();
                ::std::option::Option::Some((__sig, ::std::boxed::Box::pin(async move {
                    let mut __args = __args.into_iter();
                    // Skip args[0] (the receiver) — already bound as __self.
                    let _ = __args.next();
                    #body
                })))
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
                &FunctionNameSource::Dynamic,
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
        let (param_specs, has_variadic, has_runtime_types) =
            gen_param_specs(params, krate, &Default::default());

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

                            variadic_doc: ::std::option::Option::None,
                            arg_offset: 1,
                            returns: None,
                            lua_returns: ::std::option::Option::Some(
                                <#return_type as #k::LuaTypedMulti>::lua_types()
                            ),
                            line_defined: 0,
                            last_line_defined: 0,
                            num_upvalues: 0,
                            has_runtime_types: #has_runtime_types,
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
                &FunctionNameSource::Dynamic,
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

                                    variadic_doc: ::std::option::Option::None,
                                    arg_offset: 1,
                                    returns: None,
                                    lua_returns: ::std::option::Option::Some(
                                        <#return_type as #k::LuaTypedMulti>::lua_types()
                                    ),
                                    line_defined: 0,
                                    last_line_defined: 0,
                                    num_upvalues: 0,
                                    has_runtime_types: #has_runtime_types,
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
                &FunctionNameSource::Dynamic,
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

/// Generate the `__pairs` dispatch arm for a `#[lua_pairs]`-marked
/// method.  The arm calls the user's iterator-returning method,
/// stashes the boxed iterator under a `parking_lot::Mutex`, and
/// builds a stateless `Function::wrap` iter-fn that pops the next
/// `(key, value)` pair on each invocation.  Returning `(None, None)`
/// from the iter-fn signals end-of-iteration to Lua's generic-for
/// (any `nil` for the key terminates the loop).
fn gen_pairs_arm(type_name: &str, p: &PairsMethod, krate: &CratePath) -> TokenStream {
    let k = krate.tokens();
    let ident = &p.ident;
    let materialise = if p.is_result {
        quote! { let __iter = self.#ident()?; }
    } else {
        quote! { let __iter = self.#ident(); }
    };
    let iter_name = format!("{type_name}.__pairs.iter");
    let ctx_name = format!("{type_name}:__pairs");
    let ctx_name_bytes = ctx_name.as_bytes().to_vec();
    quote! {
        "__pairs" => {
            let __ctx = {
                let mut __c = __ctx.clone();
                __c.native_name = ::std::option::Option::Some(
                    #k::Bytes::from(&[ #(#ctx_name_bytes),* ][..])
                );
                __c
            };
            #materialise
            let __state = ::std::sync::Arc::new(
                ::parking_lot::Mutex::new(::std::boxed::Box::new(__iter)),
            );
            let __iter_state = ::std::sync::Arc::clone(&__state);
            let __iter_fn = #k::Function::wrap(
                #iter_name,
                move |_state: #k::Value, _control: #k::Value| {
                    // Return type is `(Option<K>, Option<V>)` where
                    // K and V are inferred from the iterator's
                    // `Item = (K, V)`.  Returning `(None, None)`
                    // signals end-of-iteration: Lua's generic-for
                    // terminates as soon as the first returned
                    // value is `nil`.
                    match __iter_state.lock().next() {
                        ::std::option::Option::Some((__key, __val)) => {
                            ::std::result::Result::Ok::<_, #k::VmError>((
                                ::std::option::Option::Some(__key),
                                ::std::option::Option::Some(__val),
                            ))
                        }
                        ::std::option::Option::None => {
                            ::std::result::Result::Ok((
                                ::std::option::Option::None,
                                ::std::option::Option::None,
                            ))
                        }
                    }
                },
            );
            ::std::result::Result::Ok(#k::valuevec![
                #k::Value::Function(__iter_fn),
                #k::Value::Nil,
                #k::Value::Nil,
            ])
        }
    }
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

/// Generate `pub fn userdata_type() -> UserdataType { ... }` for the
/// userdata type, populated with documentation harvested from rustdoc
/// on the impl block, methods, fields, and metamethods.
///
/// Fields are deduplicated by Lua name: if both a getter and a setter
/// exist for the same field, the resulting [`FieldDef`] uses
/// `FieldKind::Getter` and the getter's return type drives `lua_type`.
/// Setter-only fields use `FieldKind::Setter` with the setter's
/// parameter type.
fn gen_userdata_type_fn(
    lua_type_name: &str,
    impl_doc: &Option<String>,
    fields: &[FieldInfo],
    methods: &[MethodInfo],
    metamethods: &[MetamethodInfo],
    krate: &CratePath,
) -> TokenStream {
    let k = krate.tokens();
    let name_bytes = lua_type_name.as_bytes().to_vec();
    let doc_expr = opt_string_expr(impl_doc.as_ref());
    let source = format!("=[{lua_type_name}]");

    // Group field entries by Lua name so getter+setter pairs collapse.
    let mut field_stmts: Vec<TokenStream> = Vec::new();
    let mut emitted_field_names: Vec<String> = Vec::new();
    for f in fields.iter().filter(|f| !f.is_setter) {
        emitted_field_names.push(f.lua_name.clone());
        let name_bytes = f.lua_name.as_bytes().to_vec();
        let doc_expr = opt_string_expr(f.doc.as_ref());
        let examples_expr = examples_vec_expr(&f.examples, krate);
        let lua_type_expr = match &f.return_type {
            Some(rt) => quote! { <#rt as #k::LuaTyped>::lua_type() },
            None => quote! { #k::LuaType::Any },
        };
        // If the same Lua-visible name also has a setter, the field
        // is read-write; otherwise getter-only is read-only.
        let has_setter = fields
            .iter()
            .any(|other| other.is_setter && other.lua_name == f.lua_name);
        let kind_expr = if has_setter {
            quote! { #k::types::FieldKind::ReadWrite }
        } else {
            quote! { #k::types::FieldKind::Getter }
        };
        field_stmts.push(quote! {
            __fields.push(#k::types::FieldDef {
                name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                doc: #doc_expr,
                lua_type: #lua_type_expr,
                kind: #kind_expr,
                examples: #examples_expr,
            });
        });
    }
    // Setter-only fields: derive lua_type from the first Normal param.
    for f in fields.iter().filter(|f| f.is_setter) {
        if emitted_field_names.contains(&f.lua_name) {
            continue;
        }
        emitted_field_names.push(f.lua_name.clone());
        let name_bytes = f.lua_name.as_bytes().to_vec();
        let doc_expr = opt_string_expr(f.doc.as_ref());
        let examples_expr = examples_vec_expr(&f.examples, krate);
        let lua_type_expr = f
            .params
            .iter()
            .find_map(|p| match p {
                ParamKind::Normal(_, ty) => {
                    let stripped = crate::util::strip_reference(ty);
                    Some(quote! { <#stripped as #k::LuaTyped>::lua_type() })
                }
                _ => None,
            })
            .unwrap_or_else(|| quote! { #k::LuaType::Any });
        field_stmts.push(quote! {
            __fields.push(#k::types::FieldDef {
                name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                doc: #doc_expr,
                lua_type: #lua_type_expr,
                kind: #k::types::FieldKind::Setter,
                examples: #examples_expr,
            });
        });
    }

    let mut method_stmts: Vec<TokenStream> = Vec::new();
    for m in methods {
        let name_bytes = m.lua_name.as_bytes().to_vec();
        let doc_expr = opt_string_expr(m.doc.as_ref());
        let examples_expr = examples_vec_expr(&m.examples, krate);
        let signature = gen_function_signature(
            &m.lua_name,
            &m.params,
            &m.return_type,
            krate,
            source.as_bytes(),
            1,
            &m.param_docs,
        );
        let returns_doc_lits: Vec<TokenStream> = m
            .returns_doc
            .iter()
            .map(|s| quote! { #s.to_owned() })
            .collect();
        method_stmts.push(quote! {
            __methods.push(#k::types::FunctionDef {
                name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                doc: #doc_expr,
                signature: #signature,
                returns_doc: ::std::vec![ #(#returns_doc_lits),* ],
                examples: #examples_expr,
            });
        });
    }

    let mut metamethod_stmts: Vec<TokenStream> = Vec::new();
    for mm in metamethods {
        let doc_expr = opt_string_expr(mm.doc.as_ref());
        let examples_expr = examples_vec_expr(&mm.examples, krate);
        let meta_name_str = &mm.meta_name;
        let signature = gen_function_signature(
            &mm.meta_name,
            &mm.params,
            &mm.return_type,
            krate,
            source.as_bytes(),
            1,
            &mm.param_docs,
        );
        let returns_doc_lits: Vec<TokenStream> = mm
            .returns_doc
            .iter()
            .map(|s| quote! { #s.to_owned() })
            .collect();
        metamethod_stmts.push(quote! {
            if let ::std::result::Result::Ok(__mm) =
                <#k::MetaMethod as ::std::str::FromStr>::from_str(#meta_name_str)
            {
                __metamethods.push(#k::types::MetamethodDef {
                    method: __mm,
                    doc: #doc_expr,
                    signature: #signature,
                    returns_doc: ::std::vec![ #(#returns_doc_lits),* ],
                    examples: #examples_expr,
                });
            }
        });
    }

    quote! {
        /// Build the documentation/type descriptor for this userdata.
        ///
        /// Used by `shingetsu-docgen` to emit reference documentation
        /// and type definitions.  Register with
        /// [`GlobalEnv::register_userdata_type`].
        pub fn userdata_type() -> #k::types::UserdataType {
            let mut __fields: ::std::vec::Vec<#k::types::FieldDef> =
                ::std::vec::Vec::new();
            let mut __methods: ::std::vec::Vec<#k::types::FunctionDef> =
                ::std::vec::Vec::new();
            let mut __metamethods: ::std::vec::Vec<#k::types::MetamethodDef> =
                ::std::vec::Vec::new();
            #(#field_stmts)*
            #(#method_stmts)*
            #(#metamethod_stmts)*
            #k::types::UserdataType {
                name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                doc: #doc_expr,
                fields: __fields,
                methods: __methods,
                metamethods: __metamethods,
            }
        }
    }
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
        let lua_type_expr = match &f.return_type {
            Some(rt) => {
                // Getters return a single value; use LuaTyped to map
                // the Rust return type to a LuaType.
                quote! { <#rt as #k::LuaTyped>::lua_type() }
            }
            None => quote! { #k::LuaType::Any },
        };
        field_entries.push(quote! {
            (
                #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                #lua_type_expr,
            )
        });
    }

    // Methods contribute a LuaType::Function with their param/return types.
    for m in methods {
        let name_bytes = m.lua_name.as_bytes().to_vec();
        let return_type = &m.return_type;

        // Build param types from the method's Lua-visible params.
        // Per-param rustdoc captured by parse_doc_block on the
        // method's attrs flows into TypedParam.doc so the type
        // returned by lua_type_info() carries the same docs as
        // the FunctionDef in userdata_type().
        let mut param_type_entries = Vec::<TokenStream>::new();
        for p in &m.params {
            match p {
                ParamKind::Normal(ident, ty) => {
                    let name_str = ident.to_string();
                    let name_bytes = name_str.as_bytes().to_vec();
                    let lua_ty = crate::util::strip_reference(ty);
                    let doc_expr = opt_string_expr(m.param_docs.get(&name_str));
                    param_type_entries.push(quote! {
                        #k::TypedParam::new_with_doc(
                            ::std::option::Option::Some(
                                #k::Bytes::from(&[ #(#name_bytes),* ][..])
                            ),
                            <#lua_ty as #k::LuaTyped>::lua_type(),
                            #doc_expr,
                        )
                    });
                }
                ParamKind::BinOpSide(_, _) => {}
                ParamKind::CallContext(_) | ParamKind::GlobalEnv(_) | ParamKind::FrameLocals(_) => {
                }
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
// mlua-side codegen for the migration facade
// ---------------------------------------------------------------------------

/// Emit `impl ::mlua::UserData for #self_ty` covering sync and
/// async `#[lua_method]`, sync `#[lua_field]`, and
/// `#[lua_metamethod]` items.  Sync methods register through
/// `add_method` / `add_method_mut`; async methods register through
/// `add_async_method` / `add_async_method_mut`.  Non-binary
/// metamethods register through `add_meta_method` /
/// `add_meta_method_mut`; binary metamethods (per
/// [`shingetsu_meta::MetaMethod::is_binary_op`]) register through
/// `add_meta_function` with manual side detection so the userdata
/// works on either operand.
///
/// When `auto_snapshot` is set (from
/// `#[shingetsu_migrate::userdata(snapshot)]`), also registers a
/// `__memoize` metamethod that returns a
/// `shingetsu_migrate::Memoized` capturing a clone of the
/// userdata; this is the mlua-side counterpart to the shingetsu
/// `Userdata::snapshot()` hook and lets kumomta's `mod-memoize`
/// keep walking userdata via the existing `__memoize` convention
/// during the migration.
///
/// When `pairs_method` is set (from `#[lua_pairs]`), also
/// registers a `__pairs` metamethod whose body materializes the
/// user's iterator-returning method, stashes the boxed iterator
/// under a `parking_lot::Mutex`, and builds an
/// `lua.create_function_mut` iter-fn that pops `.next()` per call.
/// `(None, None)` ends Lua's generic-for.
///
/// `__close` registers as a regular non-binary metamethod (sync
/// via `add_meta_method` / `add_meta_method_mut`, async via
/// `add_async_meta_method` / `add_async_meta_method_mut`).  Lua
/// 5.4's optional error argument is delivered as a normal extra
/// parameter to the user's method.
///
/// Items the facade can't yet mirror —
/// `#[lua_snapshot]` (the explicit-body form),
/// `__gc`/`__pairs`/`__ipairs`, async `#[lua_field]`, async binary
/// `#[lua_metamethod]`, methods taking `Arc<Self>` or
/// `CallContext`/`GlobalEnv`/`FrameLocals`, and
/// `Variadic`/`BinOpSide` parameters — emit a `compile_error!` (so
/// the host knows to keep that type on the engine-coupled
/// `#[shingetsu::userdata]` macro until the corresponding facade
/// support lands).
fn gen_mlua_userdata_impl(
    self_ty: &Type,
    methods: &[MethodInfo],
    fields: &[FieldInfo],
    metamethods: &[MetamethodInfo],
    snapshot_method: &Option<Ident>,
    auto_snapshot: bool,
    pairs_method: &Option<PairsMethod>,
) -> TokenStream {
    let mut errors: Vec<TokenStream> = Vec::new();
    let mut metamethod_stmts: Vec<TokenStream> = Vec::new();

    for m in metamethods {
        if let Some(stmt) = gen_mlua_metamethod_stmt(m, &mut errors) {
            metamethod_stmts.push(stmt);
        }
    }

    if let Some(p) = pairs_method {
        // `#[lua_pairs]` on the mlua side: register a `__pairs`
        // metamethod that materializes the user's iterator,
        // stashes it under a `parking_lot::Mutex`, and builds an
        // `lua.create_function_mut`-based iter-fn that pops
        // `.next()` on each call.  Returning `(None, None)` ends
        // Lua's generic-for.
        let ident = &p.ident;
        let materialise = if p.is_result {
            quote! { let __iter = __this.#ident().map_err(::mlua::Error::external)?; }
        } else {
            quote! { let __iter = __this.#ident(); }
        };
        metamethod_stmts.push(quote! {
            __methods.add_meta_method(
                "__pairs",
                |__lua: &::mlua::Lua, __this, _: ()| -> ::mlua::Result<(
                    ::mlua::Function,
                    ::mlua::Value,
                    ::mlua::Value,
                )> {
                    #materialise
                    let __state = ::std::sync::Arc::new(
                        ::parking_lot::Mutex::new(::std::boxed::Box::new(__iter)),
                    );
                    let __iter_state = ::std::sync::Arc::clone(&__state);
                    let __iter_fn = __lua.create_function_mut(
                        move |_lua: &::mlua::Lua, (_state, _control): (
                            ::mlua::Value,
                            ::mlua::Value,
                        )| {
                            // `(Option<K>, Option<V>)` return inferred from
                            // the iterator's `Item = (K, V)`; `(None, None)`
                            // signals end-of-iteration to Lua's generic-for.
                            match __iter_state.lock().next() {
                                ::std::option::Option::Some((__key, __val)) => {
                                    ::std::result::Result::Ok::<_, ::mlua::Error>((
                                        ::std::option::Option::Some(__key),
                                        ::std::option::Option::Some(__val),
                                    ))
                                }
                                ::std::option::Option::None => {
                                    ::std::result::Result::Ok((
                                        ::std::option::Option::None,
                                        ::std::option::Option::None,
                                    ))
                                }
                            }
                        },
                    )?;
                    ::std::result::Result::Ok((
                        __iter_fn,
                        ::mlua::Value::Nil,
                        ::mlua::Value::Nil,
                    ))
                },
            );
        });
    }

    if auto_snapshot {
        // `shingetsu_migrate::Memoized`'s `to_value` clones the
        // captured `Self` and runs it through the live `Lua`'s
        // `IntoLua` at rebuild time — mirrors kumomta's existing
        // `Memoized::impl_memoize::<T>` shape so `mod-memoize`'s
        // cache walker keeps working unchanged through the
        // transition.
        metamethod_stmts.push(quote! {
            __methods.add_meta_method(
                "__memoize",
                |_lua: &::mlua::Lua, __this, _: ()| -> ::mlua::Result<
                    ::shingetsu_migrate::Memoized,
                > {
                    let __captured = ::std::clone::Clone::clone(__this);
                    ::std::result::Result::Ok(::shingetsu_migrate::Memoized {
                        to_value: ::std::sync::Arc::new(
                            move |__lua: &::mlua::Lua| -> ::mlua::Result<::mlua::Value> {
                                ::mlua::IntoLua::into_lua(
                                    ::std::clone::Clone::clone(&__captured),
                                    __lua,
                                )
                            },
                        ),
                    })
                },
            );
        });
    }

    if let Some(ident) = snapshot_method {
        // The explicit-body `#[lua_snapshot]` form has the user
        // hand-write a body returning `shingetsu::Snapshot`, which
        // references shingetsu-only types and can't compile under
        // the mlua-only build path.  Direct hosts to
        // `#[shingetsu_migrate::userdata(snapshot)]` for the
        // auto-clone form that works on both engines.
        errors.push(
            syn::Error::new_spanned(
                ident,
                "the migration facade does not yet mirror the explicit-body `#[lua_snapshot]` \
                 on the mlua side; use `#[shingetsu_migrate::userdata(snapshot)]` for the \
                 auto-clone form (requires `Self: Clone + IntoLua` on both engines)",
            )
            .into_compile_error(),
        );
    }

    let mut method_stmts: Vec<TokenStream> = Vec::new();
    for m in methods {
        let mut idents: Vec<Ident> = Vec::new();
        let mut types: Vec<TokenStream> = Vec::new();
        let mut bad = false;
        for p in &m.params {
            match p {
                ParamKind::Normal(id, ty) => {
                    idents.push(id.clone());
                    types.push(quote! { #ty });
                }
                ParamKind::CallContext(id)
                | ParamKind::GlobalEnv(id)
                | ParamKind::FrameLocals(id) => {
                    errors.push(
                        syn::Error::new_spanned(
                            id,
                            "the migration facade cannot mirror `CallContext`, `GlobalEnv`, \
                             or `FrameLocals` parameters on the mlua side",
                        )
                        .into_compile_error(),
                    );
                    bad = true;
                }
                ParamKind::Variadic(id)
                | ParamKind::VariadicMulti(id, _)
                | ParamKind::BinOpSide(id, _) => {
                    errors.push(
                        syn::Error::new_spanned(
                            id,
                            "the migration facade does not yet mirror variadic or \
                             `BinOpSide` parameters on the mlua side",
                        )
                        .into_compile_error(),
                    );
                    bad = true;
                }
            }
        }
        if bad {
            continue;
        }

        let lua_name = &m.lua_name;
        let ident = &m.ident;
        if m.is_async {
            // Async methods bind via mlua's `add_async_method` /
            // `add_async_method_mut`, which take `Lua` by value
            // and `UserDataRef<T>` / `UserDataRefMut<T>` as the
            // receiver.  The closure must move-capture `__this`
            // so the returned future can hold the borrow across
            // `.await`.
            let adder = if m.is_mut_self {
                quote! { add_async_method_mut }
            } else {
                quote! { add_async_method }
            };
            let async_call = if m.is_result {
                quote! {
                    __this.#ident(#(#idents,)*).await
                        .map_err(::mlua::Error::external)
                }
            } else {
                quote! { ::std::result::Result::Ok(__this.#ident(#(#idents,)*).await) }
            };
            method_stmts.push(quote! {
                __methods.#adder(
                    #lua_name,
                    move |_lua: ::mlua::Lua, __this, ( #(#idents,)* ): ( #(#types,)* )| async move {
                        #async_call
                    },
                );
            });
        } else {
            let call_expr = if m.is_result {
                quote! { __this.#ident(#(#idents,)*).map_err(::mlua::Error::external) }
            } else {
                quote! { ::std::result::Result::Ok(__this.#ident(#(#idents,)*)) }
            };
            let adder = if m.is_mut_self {
                quote! { add_method_mut }
            } else {
                quote! { add_method }
            };
            method_stmts.push(quote! {
                __methods.#adder(
                    #lua_name,
                    |_lua: &::mlua::Lua, __this, ( #(#idents,)* ): ( #(#types,)* )| {
                        #call_expr
                    },
                );
            });
        }
    }

    let mut field_stmts: Vec<TokenStream> = Vec::new();
    for f in fields {
        if f.is_async {
            errors.push(
                syn::Error::new_spanned(
                    &f.ident,
                    "the migration facade does not yet mirror async `#[lua_field]` on the mlua side",
                )
                .into_compile_error(),
            );
            continue;
        }
        let lua_name = &f.lua_name;
        let ident = &f.ident;
        if f.is_setter {
            let val_param = f.params.iter().find_map(|p| match p {
                ParamKind::Normal(id, ty) => Some((id.clone(), ty.clone())),
                _ => None,
            });
            let Some((val_ident, val_ty)) = val_param else {
                errors.push(
                    syn::Error::new_spanned(
                        &f.ident,
                        "`#[lua_field]` setter must take exactly one value parameter",
                    )
                    .into_compile_error(),
                );
                continue;
            };
            let call = if f.is_result {
                quote! {
                    __this.#ident(#val_ident).map_err(::mlua::Error::external)?;
                    ::std::result::Result::Ok(())
                }
            } else {
                quote! {
                    __this.#ident(#val_ident);
                    ::std::result::Result::Ok(())
                }
            };
            field_stmts.push(quote! {
                __fields.add_field_method_set(
                    #lua_name,
                    |_lua: &::mlua::Lua, __this, #val_ident: #val_ty| {
                        #call
                    },
                );
            });
        } else {
            let body = if f.is_result {
                quote! { __this.#ident().map_err(::mlua::Error::external) }
            } else {
                quote! { ::std::result::Result::Ok(__this.#ident()) }
            };
            field_stmts.push(quote! {
                __fields.add_field_method_get(
                    #lua_name,
                    |_lua: &::mlua::Lua, __this| {
                        #body
                    },
                );
            });
        }
    }

    if !errors.is_empty() {
        return quote! { #(#errors)* };
    }

    quote! {
        impl ::mlua::UserData for #self_ty {
            fn add_methods<__M: ::mlua::UserDataMethods<Self>>(__methods: &mut __M) {
                #(#method_stmts)*
                #(#metamethod_stmts)*
            }
            fn add_fields<__F: ::mlua::UserDataFields<Self>>(__fields: &mut __F) {
                #(#field_stmts)*
            }
        }
    }
}

/// Emit a single `add_meta_method` / `add_meta_method_mut` /
/// `add_meta_function` registration for one shingetsu metamethod, or
/// push a `compile_error!` into `errors` and return `None` when the
/// metamethod can't be mirrored on the mlua side yet.
fn gen_mlua_metamethod_stmt(
    m: &MetamethodInfo,
    errors: &mut Vec<TokenStream>,
) -> Option<TokenStream> {
    // mlua restricts `__gc`; `__pairs` / `__ipairs` go through
    // `#[lua_pairs]` instead.  `__close` is supported as a normal
    // non-binary metamethod (sync or async); Lua 5.4's optional
    // error parameter is exposed as a regular extra arg.
    if matches!(m.meta_name.as_str(), "__gc" | "__pairs" | "__ipairs") {
        errors.push(
            syn::Error::new_spanned(
                &m.ident,
                ::std::format!(
                    "the migration facade does not yet mirror `{}` on the mlua side",
                    m.meta_name,
                ),
            )
            .into_compile_error(),
        );
        return None;
    }

    let mut idents: Vec<Ident> = Vec::new();
    let mut types: Vec<TokenStream> = Vec::new();
    let mut bad = false;
    for p in &m.params {
        match p {
            ParamKind::Normal(id, ty) => {
                idents.push(id.clone());
                types.push(quote! { #ty });
            }
            ParamKind::CallContext(id) | ParamKind::GlobalEnv(id) | ParamKind::FrameLocals(id) => {
                errors.push(
                    syn::Error::new_spanned(
                        id,
                        "the migration facade cannot mirror `CallContext`, `GlobalEnv`, \
                         or `FrameLocals` parameters on the mlua side",
                    )
                    .into_compile_error(),
                );
                bad = true;
            }
            ParamKind::Variadic(id) | ParamKind::VariadicMulti(id, _) => {
                errors.push(
                    syn::Error::new_spanned(
                        id,
                        "the migration facade does not yet mirror variadic parameters \
                         on `#[lua_metamethod]` on the mlua side",
                    )
                    .into_compile_error(),
                );
                bad = true;
            }
            ParamKind::BinOpSide(id, _) => {
                errors.push(
                    syn::Error::new_spanned(
                        id,
                        "the migration facade does not yet mirror `BinOpSide<T>` parameters \
                         on the mlua side",
                    )
                    .into_compile_error(),
                );
                bad = true;
            }
        }
    }
    if bad {
        return None;
    }

    let mm_name = &m.meta_name;
    let ident = &m.ident;
    // `__eq` is binary-op-shaped per `is_binary_op`, but Lua only
    // invokes it when both operands are the same userdata type, so
    // mlua's mis-fire concern with `add_meta_method` doesn't apply
    // — register it through `add_meta_method` so the metatable
    // entry matches what mlua's `==` dispatch expects.
    let is_binary = m
        .meta_name
        .parse::<shingetsu_meta::MetaMethod>()
        .map(|mm| mm.is_binary_op())
        .unwrap_or(false)
        && m.meta_name != "__eq";

    if is_binary && m.is_async {
        errors.push(
            syn::Error::new_spanned(
                &m.ident,
                "the migration facade does not yet mirror async binary `#[lua_metamethod]` \
                 on the mlua side",
            )
            .into_compile_error(),
        );
        return None;
    }

    if is_binary {
        // Binary ops use `add_meta_function` because the userdata may
        // appear on either operand side; mlua's `add_meta_method`
        // mis-fires when only the right operand carries the
        // metatable.  We replicate shingetsu's binary-op dispatch:
        // identify which side is `Self`, decode the other side as
        // the operand parameter, and call the user's method.
        if idents.len() != 1 {
            errors.push(
                syn::Error::new_spanned(
                    &m.ident,
                    "binary `#[lua_metamethod]` must take exactly one operand parameter \
                     on the mlua side",
                )
                .into_compile_error(),
            );
            return None;
        }
        let op_id = &idents[0];
        let op_ty = &types[0];
        let call_expr = if m.is_result {
            quote! { (*__this).#ident(#op_id).map_err(::mlua::Error::external) }
        } else {
            quote! { ::std::result::Result::Ok((*__this).#ident(#op_id)) }
        };
        let mismatch_msg = format!(
            "binary metamethod {}: neither operand is the expected userdata type",
            mm_name,
        );
        Some(quote! {
            __methods.add_meta_function(
                #mm_name,
                |__lua: &::mlua::Lua, (__lhs, __rhs): (::mlua::Value, ::mlua::Value)| {
                    let (__this, __operand) = if let ::mlua::Value::UserData(ref __ud) = __lhs {
                        if __ud.is::<Self>() {
                            (__ud.borrow::<Self>()?, __rhs)
                        } else if let ::mlua::Value::UserData(ref __ud2) = __rhs {
                            if __ud2.is::<Self>() {
                                (__ud2.borrow::<Self>()?, __lhs.clone())
                            } else {
                                return ::std::result::Result::Err(
                                    ::mlua::Error::external(#mismatch_msg),
                                );
                            }
                        } else {
                            return ::std::result::Result::Err(
                                ::mlua::Error::external(#mismatch_msg),
                            );
                        }
                    } else if let ::mlua::Value::UserData(ref __ud) = __rhs {
                        if __ud.is::<Self>() {
                            (__ud.borrow::<Self>()?, __lhs)
                        } else {
                            return ::std::result::Result::Err(
                                ::mlua::Error::external(#mismatch_msg),
                            );
                        }
                    } else {
                        return ::std::result::Result::Err(
                            ::mlua::Error::external(#mismatch_msg),
                        );
                    };
                    let #op_id: #op_ty = ::mlua::FromLua::from_lua(__operand, __lua)?;
                    #call_expr
                },
            );
        })
    } else if m.is_async {
        // Async non-binary metamethods (notably `__close` on
        // resource userdata) bind via mlua's `add_async_meta_method`
        // / `add_async_meta_method_mut`, which take `Lua` by value
        // and `UserDataRef<T>` / `UserDataRefMut<T>` as the
        // receiver.
        let adder = if m.is_mut_self {
            quote! { add_async_meta_method_mut }
        } else {
            quote! { add_async_meta_method }
        };
        let async_call = if m.is_result {
            quote! {
                __this.#ident(#(#idents,)*).await
                    .map_err(::mlua::Error::external)
            }
        } else {
            quote! { ::std::result::Result::Ok(__this.#ident(#(#idents,)*).await) }
        };
        Some(quote! {
            __methods.#adder(
                #mm_name,
                move |_lua: ::mlua::Lua, __this, ( #(#idents,)* ): ( #(#types,)* )| async move {
                    #async_call
                },
            );
        })
    } else {
        let call_expr = if m.is_result {
            quote! { __this.#ident(#(#idents,)*).map_err(::mlua::Error::external) }
        } else {
            quote! { ::std::result::Result::Ok(__this.#ident(#(#idents,)*)) }
        };
        let adder = if m.is_mut_self {
            quote! { add_meta_method_mut }
        } else {
            quote! { add_meta_method }
        };
        Some(quote! {
            __methods.#adder(
                #mm_name,
                |_lua: &::mlua::Lua, __this, ( #(#idents,)* ): ( #(#types,)* )| {
                    #call_expr
                },
            );
        })
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
