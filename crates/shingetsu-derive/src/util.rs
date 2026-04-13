use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::{Attribute, FnArg, Pat, PatType, ReturnType, Signature, Type, TypePath};

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
    Normal(Ident),
    /// `CallContext` parameter — passed through from the call site directly.
    CallContext(Ident),
    /// `Variadic` — collects all remaining args into a `Variadic(vec)`.
    Variadic(Ident),
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
                } else {
                    out.push(ParamKind::Normal(ident));
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
/// argument extraction → function call → IntoLuaMulti.
///
/// `fn_call` is already the complete call expression (ident or method path +
/// args are generated here).
pub fn gen_call_body(
    fn_expr: TokenStream,
    params: &[ParamKind],
    is_async: bool,
    is_result: bool,
) -> TokenStream {
    let mut extractions = Vec::<TokenStream>::new();
    let mut call_args = Vec::<TokenStream>::new();

    for p in params {
        match p {
            ParamKind::Normal(id) => {
                extractions.push(quote! {
                    let #id = ::shingetsu::FromLua::from_lua(
                        __args.next().unwrap_or(::shingetsu::Value::Nil)
                    )?;
                });
                call_args.push(quote! { #id });
            }
            ParamKind::CallContext(id) => {
                extractions.push(quote! { let #id = __ctx.clone(); });
                call_args.push(quote! { #id });
            }
            ParamKind::Variadic(id) => {
                extractions.push(quote! {
                    let #id = ::shingetsu::Variadic(__args.collect::<::std::vec::Vec<_>>());
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
        quote! { #awaited.map_err(|__e| <::shingetsu::VmError as ::std::convert::From<_>>::from(__e))? }
    } else {
        awaited
    };

    quote! {
        let mut __args = __args.into_iter();
        #(#extractions)*
        let __result = #result_expr;
        Ok(::shingetsu::IntoLuaMulti::into_lua_multi(__result))
    }
}

/// Build a `NativeFunction` literal for a free function in a module.
pub fn gen_native_fn(
    lua_name: &str,
    fn_ident: &Ident,
    params: &[ParamKind],
    is_async: bool,
    is_result: bool,
) -> TokenStream {
    let name_bytes = lua_name.as_bytes().to_vec();
    let body = gen_call_body(quote! { #fn_ident }, params, is_async, is_result);
    quote! {
        ::shingetsu::NativeFunction {
            signature: ::std::sync::Arc::new(::shingetsu::FunctionSignature {
                name: ::shingetsu::bytes::Bytes::from_static(&[ #(#name_bytes),* ]),
                type_params: ::std::vec::Vec::new(),
                params: ::std::vec::Vec::new(),
                variadic: true,
                returns: None,
                lua_returns: None,
            }),
            call: ::std::sync::Arc::new(|__ctx, __args| {
                ::std::boxed::Box::pin(async move { #body })
            }),
        }
    }
}
