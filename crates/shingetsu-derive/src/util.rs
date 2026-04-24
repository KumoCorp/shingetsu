use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::{Attribute, FnArg, Pat, PatType, ReturnType, Signature, Type, TypePath};

/// The crate path to use in generated code.  Defaults to `::shingetsu`
/// but can be overridden to `crate` (or any other path) via the
/// `crate = "..."` attribute option, allowing the macros to be used
/// from within `shingetsu-vm` itself.
#[derive(Clone)]
pub struct CratePath {
    path: syn::Path,
}

impl Default for CratePath {
    fn default() -> Self {
        Self {
            path: syn::parse_str("::shingetsu").expect("valid path"),
        }
    }
}

impl CratePath {
    pub fn from_str(s: &str) -> syn::Result<Self> {
        Ok(Self {
            path: syn::parse_str(s)?,
        })
    }

    /// Return the path as a token stream for interpolation in `quote!`.
    pub fn tokens(&self) -> &syn::Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Attribute helpers
// ---------------------------------------------------------------------------

/// Collect `/// doc` lines from attributes into a single string.
#[allow(dead_code)]
pub fn extract_doc(attrs: &[Attribute]) -> Option<String> {
    let parts: Vec<String> = attrs
        .iter()
        .filter_map(|a| {
            if !a.path().is_ident("doc") {
                return None;
            }
            if let syn::Meta::NameValue(nv) = &a.meta {
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
                {
                    return Some(s.value().trim().to_owned());
                }
            }
            None
        })
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Remove all attributes whose path matches `name` from `attrs`.
pub fn strip_attr(attrs: &mut Vec<Attribute>, name: &str) {
    attrs.retain(|a| !a.path().is_ident(name));
}

// ---------------------------------------------------------------------------
// Type inspection
// ---------------------------------------------------------------------------

/// Returns `true` if the last path segment of a `Type::Path` is `name`.
pub fn type_is(ty: &Type, name: &str) -> bool {
    if let Type::Path(TypePath { path, .. }) = ty {
        path.segments
            .last()
            .map(|s| s.ident == name)
            .unwrap_or(false)
    } else {
        false
    }
}

/// Returns `true` if the outermost return type is `Result<…>`.
pub fn is_result_return(ret: &ReturnType) -> bool {
    match ret {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => type_is(ty, "Result"),
    }
}

/// Extract the inner return type suitable for `LuaTypedMulti`.
///
/// - `-> Result<T, VmError>` → `T`
/// - `-> T` (non-Result) → `T`
/// - default (no return) → `()`
pub fn inner_return_type(ret: &ReturnType) -> Box<Type> {
    match ret {
        ReturnType::Default => {
            syn::parse_quote! { () }
        }
        ReturnType::Type(_, ty) => {
            if type_is(ty, "Result") {
                // Extract first generic arg from Result<T, E>
                if let Type::Path(TypePath { path, .. }) = ty.as_ref() {
                    if let Some(seg) = path.segments.last() {
                        if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                                return Box::new(inner.clone());
                            }
                        }
                    }
                }
                // Fallback: just use the whole type
                ty.clone()
            } else {
                ty.clone()
            }
        }
    }
}

/// Returns `true` if the return type is omitted or `-> ()`.
#[allow(dead_code)]
pub fn is_unit_return(ret: &ReturnType) -> bool {
    match ret {
        ReturnType::Default => true,
        ReturnType::Type(_, ty) => {
            matches!(ty.as_ref(), Type::Tuple(t) if t.elems.is_empty())
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter classification
// ---------------------------------------------------------------------------

pub enum ParamKind {
    /// Regular Lua argument — extracted via `FromLua::from_lua(next)?`.
    /// The `Type` is the original Rust type from the signature.
    Normal(Ident, Box<Type>),
    /// `CallContext` parameter — passed through from the call site directly.
    CallContext(Ident),
    /// `Variadic` — collects all remaining args into a `Variadic(vec)`.
    Variadic(Ident),
    /// A typed variadic parameter decoded via `FromLuaMulti`.
    /// All remaining Lua args are collected and passed to
    /// `FromLuaMulti::from_lua_multi`.
    VariadicMulti(Ident, Box<Type>),
    /// `BinOpSide<T>` parameter for binary metamethods — the inner `T` is
    /// extracted via `FromLua` and wrapped in the correct `BinOpSide` variant
    /// based on which side `self` was on in the original expression.
    BinOpSide(Ident, Box<Type>),
}

/// Parse the non-`self` parameters of a function signature.
pub fn parse_params(sig: &Signature) -> Vec<ParamKind> {
    let mut out = Vec::new();
    for arg in &sig.inputs {
        match arg {
            FnArg::Receiver(_) => {}
            FnArg::Typed(PatType { pat, ty, .. }) => {
                let ident = match pat.as_ref() {
                    Pat::Ident(pi) => pi.ident.clone(),
                    _ => Ident::new(&format!("__arg{}", out.len()), Span::call_site()),
                };
                if type_is(ty, "CallContext") {
                    out.push(ParamKind::CallContext(ident));
                } else if type_is(ty, "Variadic") {
                    out.push(ParamKind::Variadic(ident));
                } else if let Some(inner) = unwrap_binopside_inner(ty) {
                    out.push(ParamKind::BinOpSide(ident, Box::new(inner.clone())));
                } else {
                    out.push(ParamKind::Normal(ident, ty.clone()));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Call body generation
// ---------------------------------------------------------------------------

/// Generate the body of a NativeFunction call closure:
/// Controls the error format emitted by the inline type checks in
/// `gen_call_body`.
pub(crate) enum ErrorStyle {
    /// `bad argument #N to 'func' (expected, got)` — for functions,
    /// methods, and metamethods.
    BadArgument,
    /// `bad value in assignment to 'Type.field' (expected, got)` — for
    /// field setters where a positional "argument" doesn't make sense.
    FieldAssignment,
}

/// argument extraction → function call → IntoLuaMulti.
///
/// `fn_call` is already the complete call expression (ident or method path +
/// args are generated here).
pub fn gen_call_body(
    fn_expr: TokenStream,
    params: &[ParamKind],
    is_async: bool,
    is_result: bool,
    krate: &CratePath,
) -> TokenStream {
    gen_call_body_styled(
        fn_expr,
        params,
        is_async,
        is_result,
        ErrorStyle::BadArgument,
        krate,
    )
}

pub(crate) fn gen_call_body_styled(
    fn_expr: TokenStream,
    params: &[ParamKind],
    is_async: bool,
    is_result: bool,
    error_style: ErrorStyle,
    krate: &CratePath,
) -> TokenStream {
    let k = krate.tokens();
    let mut extractions = Vec::<TokenStream>::new();
    let mut call_args = Vec::<TokenStream>::new();
    // 1-based Lua argument position counter (only Normal params count).
    let mut lua_arg_pos: usize = 0;

    for p in params {
        match p {
            ParamKind::Normal(id, ty) => {
                lua_arg_pos += 1;
                let pos = lua_arg_pos;
                // If we can infer a runtime type, emit an early check before
                // FromLua so that the error message uses the canonical
                // ValueType name and carries the correct position/function.
                let precheck = if let Some(vt) = rust_type_to_value_type(ty, krate) {
                    match error_style {
                        ErrorStyle::BadArgument => quote! {
                            if !#k::value_matches_type(&__arg, &#vt) {
                                return Err(#k::VmError::BadArgument {
                                    position: #pos,
                                    function: __ctx.native_name.as_ref()
                                        .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                                        .unwrap_or_default(),
                                    expected: #vt.type_name().to_owned(),
                                    got: __arg.type_name().to_owned(),
                                });
                            }
                        },
                        ErrorStyle::FieldAssignment => quote! {
                            if !#k::value_matches_type(&__arg, &#vt) {
                                let __field = __ctx.native_name.as_ref()
                                    .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                                    .unwrap_or_default();
                                let __msg = ::std::format!(
                                    "bad value in assignment to '{}' ({} expected, got {})",
                                    __field,
                                    #vt.type_name(),
                                    __arg.type_name(),
                                );
                                return Err(#k::VmError::LuaError {
                                    display: __msg.clone(),
                                    value: #k::Value::String(
                                        #k::Bytes::from(__msg)
                                    ),
                                });
                            }
                        },
                    }
                } else {
                    quote! {}
                };
                let is_option = unwrap_option_inner(ty).is_some();
                let arg_fetch = if is_option {
                    quote! {
                        let __arg = __args.next().unwrap_or(#k::Value::Nil);
                    }
                } else {
                    quote! {
                        let __arg = match __args.next() {
                            Some(v) => v,
                            None => {
                                return Err(#k::VmError::BadArgument {
                                    position: #pos,
                                    function: __ctx.native_name.as_ref()
                                        .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                                        .unwrap_or_default(),
                                    expected: "value".to_owned(),
                                    got: "no value".to_owned(),
                                });
                            }
                        };
                    }
                };
                extractions.push(quote! {
                    #arg_fetch
                    #precheck
                    let #id = #k::VmResultExt::with_call_context(
                        #k::FromLua::from_lua(__arg), #pos, &__ctx
                    )?;
                });
                call_args.push(quote! { #id });
            }
            ParamKind::BinOpSide(id, inner_ty) => {
                lua_arg_pos += 1;
                let pos = lua_arg_pos;
                let precheck = if let Some(vt) = rust_type_to_value_type(inner_ty, krate) {
                    match error_style {
                        ErrorStyle::BadArgument => quote! {
                            if !#k::value_matches_type(&__arg, &#vt) {
                                return Err(#k::VmError::BadArgument {
                                    position: #pos,
                                    function: __ctx.native_name.as_ref()
                                        .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                                        .unwrap_or_default(),
                                    expected: #vt.type_name().to_owned(),
                                    got: __arg.type_name().to_owned(),
                                });
                            }
                        },
                        ErrorStyle::FieldAssignment => quote! {
                            if !#k::value_matches_type(&__arg, &#vt) {
                                let __field = __ctx.native_name.as_ref()
                                    .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                                    .unwrap_or_default();
                                let __msg = ::std::format!(
                                    "bad value in assignment to '{}' ({} expected, got {})",
                                    __field,
                                    #vt.type_name(),
                                    __arg.type_name(),
                                );
                                return Err(#k::VmError::LuaError {
                                    display: __msg.clone(),
                                    value: #k::Value::String(
                                        #k::Bytes::from(__msg)
                                    ),
                                });
                            }
                        },
                    }
                } else {
                    quote! {}
                };
                let is_option = unwrap_option_inner(inner_ty).is_some();
                let arg_fetch = if is_option {
                    quote! {
                        let __arg = __args.next().unwrap_or(#k::Value::Nil);
                    }
                } else {
                    quote! {
                        let __arg = match __args.next() {
                            Some(v) => v,
                            None => {
                                return Err(#k::VmError::BadArgument {
                                    position: #pos,
                                    function: __ctx.native_name.as_ref()
                                        .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                                        .unwrap_or_default(),
                                    expected: "value".to_owned(),
                                    got: "no value".to_owned(),
                                });
                            }
                        };
                    }
                };
                extractions.push(quote! {
                    #arg_fetch
                    #precheck
                    let __binop_inner: #inner_ty = #k::VmResultExt::with_call_context(
                        #k::FromLua::from_lua(__arg), #pos, &__ctx
                    )?;
                    let #id = if __self_on_left {
                        #k::BinOpSide::RightOfOperator(__binop_inner)
                    } else {
                        #k::BinOpSide::LeftOfOperator(__binop_inner)
                    };
                });
                call_args.push(quote! { #id });
            }
            ParamKind::CallContext(id) => {
                extractions.push(quote! { let #id = __ctx.clone(); });
                call_args.push(quote! { #id });
            }
            ParamKind::Variadic(id) => {
                extractions.push(quote! {
                    let #id = #k::Variadic(__args.collect::<#k::ValueVec>());
                });
                call_args.push(quote! { #id });
            }
            ParamKind::VariadicMulti(id, ty) => {
                extractions.push(quote! {
                    let __remaining: #k::ValueVec = __args.collect();
                    let #id = <#ty as #k::FromLuaMulti>::from_lua_multi(__remaining)
                        .map_err(|__e| __e.with_function_context(&__ctx))?;
                });
                call_args.push(quote! { #id });
            }
        }
    }

    let raw_call = quote! { #fn_expr(#(#call_args),*) };
    let awaited = if is_async {
        quote! { #raw_call.await }
    } else {
        raw_call
    };
    let result_expr = if is_result {
        // Use an explicit closure so the compiler knows the target type is
        // VmError without ambiguity (otherwise `Into::into` is underspecified
        // when multiple `From<_> for VmError` impls are in scope).
        quote! { #awaited.map_err(|__e| <#k::VmError as ::std::convert::From<_>>::from(__e))? }
    } else {
        awaited
    };

    quote! {
        let mut __args = __args.into_iter();
        #(#extractions)*
        let __result = #result_expr;
        Ok(#k::IntoLuaMulti::into_lua_multi(__result))
    }
}

// ---------------------------------------------------------------------------
// Runtime type inference from Rust types
// ---------------------------------------------------------------------------

/// If `ty` is `BinOpSide<T>`, return `Some(T)`.  Otherwise `None`.
fn unwrap_binopside_inner(ty: &Type) -> Option<&Type> {
    if let Type::Path(TypePath { path, .. }) = ty {
        let seg = path.segments.last()?;
        if seg.ident != "BinOpSide" {
            return None;
        }
        if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                return Some(inner);
            }
        }
    }
    None
}

/// If `ty` is `Option<T>`, return `Some(T)`.  Otherwise `None`.
fn unwrap_option_inner(ty: &Type) -> Option<&Type> {
    if let Type::Path(TypePath { path, .. }) = ty {
        let seg = path.segments.last()?;
        if seg.ident != "Option" {
            return None;
        }
        if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
            if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                return Some(inner);
            }
        }
    }
    None
}

/// Map a Rust type to the `ValueType` token stream for use in generated
/// `ParamSpec`.  Returns `None` for types that are unconstrained at runtime
/// (e.g. `Value`).
fn rust_type_to_value_type(ty: &Type, krate: &CratePath) -> Option<TokenStream> {
    // Option<T> params accept nil, so we don't emit a runtime_type.
    // The FromLua impl for Option<T> already validates non-nil values.
    if unwrap_option_inner(ty).is_some() {
        return None;
    }

    let k = krate.tokens();
    if let Type::Path(TypePath { path, .. }) = ty {
        let seg = path.segments.last()?;
        let name = seg.ident.to_string();
        let vt = match name.as_str() {
            "bool" => quote! { #k::ValueType::Boolean },
            "i64" | "i32" | "u32" | "usize" | "f64" | "f32" => quote! { #k::ValueType::Number },
            "Bytes" | "String" => quote! { #k::ValueType::String },
            "Table" => quote! { #k::ValueType::Table },
            "Function" => quote! { #k::ValueType::Function },
            // `Value` is unconstrained — accept anything.
            "Value" => return None,
            _ => return None,
        };
        Some(vt)
    } else {
        None
    }
}

/// Generate a `Vec<ParamSpec>` token stream and a `variadic` bool from the
/// parameter list.  `CallContext` params are skipped, `Variadic` terminates.
pub(crate) fn gen_param_specs(params: &[ParamKind], krate: &CratePath) -> (TokenStream, bool) {
    let k = krate.tokens();
    let mut specs = Vec::<TokenStream>::new();
    let mut has_variadic = false;
    let mut variadic_multi_ty: Option<&Box<Type>> = None;

    for p in params {
        match p {
            ParamKind::Normal(ident, ty) => {
                let name_str = ident.to_string();
                let name_bytes = name_str.as_bytes().to_vec();
                let rt = if let Some(vt) = rust_type_to_value_type(ty, krate) {
                    quote! { ::std::option::Option::Some(#vt) }
                } else {
                    quote! { ::std::option::Option::None }
                };
                specs.push(quote! {
                    #k::ParamSpec {
                        name: ::std::option::Option::Some(
                            #k::Bytes::from(&[ #(#name_bytes),* ][..])
                        ),
                        runtime_type: #rt,
                        lua_type: ::std::option::Option::Some(
                            <#ty as #k::LuaTyped>::lua_type()
                        ),
                    }
                });
            }
            ParamKind::BinOpSide(ident, inner_ty) => {
                let name_str = ident.to_string();
                let name_bytes = name_str.as_bytes().to_vec();
                let rt = if let Some(vt) = rust_type_to_value_type(inner_ty, krate) {
                    quote! { ::std::option::Option::Some(#vt) }
                } else {
                    quote! { ::std::option::Option::None }
                };
                specs.push(quote! {
                    #k::ParamSpec {
                        name: ::std::option::Option::Some(
                            #k::Bytes::from(&[ #(#name_bytes),* ][..])
                        ),
                        runtime_type: #rt,
                        lua_type: ::std::option::Option::Some(
                            <#inner_ty as #k::LuaTyped>::lua_type()
                        ),
                    }
                });
            }
            ParamKind::CallContext(_) => {
                // Not a Lua-visible parameter — skip.
            }
            ParamKind::Variadic(_) => {
                has_variadic = true;
            }
            ParamKind::VariadicMulti(_, ty) => {
                variadic_multi_ty = Some(ty);
            }
        }
    }

    let tokens = if let Some(ty) = variadic_multi_ty {
        quote! {
            {
                let mut __specs = ::std::vec![ #(#specs),* ];
                for __lua_ty in <#ty as #k::LuaTypedMulti>::lua_types() {
                    __specs.push(#k::ParamSpec {
                        name: ::std::option::Option::None,
                        runtime_type: ::std::option::Option::None,
                        lua_type: ::std::option::Option::Some(__lua_ty),
                    });
                }
                __specs
            }
        }
    } else {
        quote! { ::std::vec![ #(#specs),* ] }
    };
    (tokens, has_variadic)
}

/// Build a `NativeFunction` literal for a free function in a module.
pub fn gen_native_fn(
    lua_name: &str,
    fn_ident: &Ident,
    params: &[ParamKind],
    is_async: bool,
    is_result: bool,
    return_type: &Type,
    krate: &CratePath,
    module_source: Option<&[u8]>,
) -> TokenStream {
    let k = krate.tokens();
    let name_bytes = lua_name.as_bytes().to_vec();
    let body = gen_call_body(quote! { #fn_ident }, params, is_async, is_result, krate);
    let (param_specs, has_variadic) = gen_param_specs(params, krate);
    let source_expr = match module_source {
        Some(bytes) => {
            let b = bytes.to_vec();
            quote! { #k::Bytes::from(&[ #(#b),* ][..]) }
        }
        None => quote! { #k::Bytes::default() },
    };
    quote! {
        #k::NativeFunction {
            signature: ::std::sync::Arc::new(#k::FunctionSignature {
                name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                source: #source_expr,
                type_params: ::std::vec::Vec::new(),
                params: #param_specs,
                variadic: #has_variadic,
                arg_offset: 0,
                returns: None,
                lua_returns: ::std::option::Option::Some(
                    <#return_type as #k::LuaTypedMulti>::lua_types()
                ),
                line_defined: 0,
                last_line_defined: 0,
                num_upvalues: 0,
            }),
            call: #k::NativeCall::Async(::std::sync::Arc::new(|__ctx, __args| {
                ::std::boxed::Box::pin(async move { #body })
            })),
        }
    }
}
