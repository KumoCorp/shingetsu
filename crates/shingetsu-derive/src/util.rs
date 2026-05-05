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

/// Parsed rustdoc text for a `#[function]` / `#[lua_method]` /
/// `#[lua_field]` / `#[lua_metamethod]` item, or for a module / impl
/// block.
///
/// `summary` is the prose preceding any recognized section header.
/// `params` are entries collected from a `# Parameters` section,
/// keyed by parameter name.  `returns` are entries from a `# Returns`
/// section, in source order.  `examples` is the verbatim text under
/// a `# Examples` section, including any fenced code blocks.
#[derive(Default, Debug)]
pub struct DocBlock {
    pub summary: Option<String>,
    pub params: std::collections::HashMap<String, String>,
    pub returns: Vec<String>,
    pub examples: Option<String>,
}

/// Concatenate `#[doc = "..."]` attributes into a single string.
///
/// Preserves leading whitespace within each line so that fenced code
/// blocks survive intact.  Strips at most one leading space (the
/// space rustc inserts after `///`).
fn collect_doc_string(attrs: &[Attribute]) -> Option<String> {
    let mut lines = Vec::<String>::new();
    for a in attrs {
        if !a.path().is_ident("doc") {
            continue;
        }
        let syn::Meta::NameValue(nv) = &a.meta else {
            continue;
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) = &nv.value
        else {
            continue;
        };
        let raw = s.value();
        // rustc rewrites `/// foo` to `#[doc = " foo"]`.  Strip at
        // most one leading space so users authoring docs without the
        // conventional space still work.
        let trimmed = raw.strip_prefix(' ').unwrap_or(&raw);
        lines.push(trimmed.to_owned());
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Collect doc text and parse `# Parameters` / `# Returns` /
/// `# Examples` sections out of it.
///
/// The recognized section headers are exactly `# Parameters`,
/// `# Returns`, and `# Examples` on a line by themselves.
///
/// Items inside `# Parameters` and `# Returns` are markdown bullet
/// entries of the form:
///
/// ```text
/// - `name` — description text, possibly
///   continued across multiple indented lines
/// ```
///
/// The em-dash separator `—` is preferred but a plain `-` or `:`
/// after the name is also accepted.  For `# Returns`, the leading
/// `` `name` `` is optional — the description text alone is enough.
///
/// `# Examples` content is captured **verbatim** until the next
/// section header or end-of-doc, preserving fenced code blocks and
/// surrounding prose so renderers can emit it as markdown.
pub fn parse_doc_block(attrs: &[Attribute]) -> DocBlock {
    let Some(text) = collect_doc_string(attrs) else {
        return DocBlock::default();
    };

    enum Section {
        Summary,
        Params,
        Returns,
        Examples,
    }

    let mut section = Section::Summary;
    let mut summary_lines: Vec<String> = Vec::new();
    let mut params: Vec<(String, String)> = Vec::new();
    let mut returns: Vec<String> = Vec::new();
    let mut examples_lines: Vec<String> = Vec::new();
    let mut current: Option<String> = None;

    let flush_current = |current: &mut Option<String>,
                         section: &Section,
                         params: &mut Vec<(String, String)>,
                         returns: &mut Vec<String>| {
        let Some(text) = current.take() else { return };
        match section {
            Section::Summary | Section::Examples => {}
            Section::Params => {
                if let Some((name, desc)) = parse_param_entry(&text) {
                    params.push((name, desc));
                }
            }
            Section::Returns => {
                let entry = strip_optional_name_prefix(&text);
                returns.push(entry);
            }
        }
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "# Parameters" {
            flush_current(&mut current, &section, &mut params, &mut returns);
            section = Section::Params;
            continue;
        }
        if trimmed == "# Returns" {
            flush_current(&mut current, &section, &mut params, &mut returns);
            section = Section::Returns;
            continue;
        }
        if trimmed == "# Examples" {
            flush_current(&mut current, &section, &mut params, &mut returns);
            section = Section::Examples;
            continue;
        }
        match section {
            Section::Summary => {
                summary_lines.push(line.to_owned());
            }
            Section::Examples => {
                examples_lines.push(line.to_owned());
            }
            Section::Params | Section::Returns => {
                if let Some(rest) = trimmed.strip_prefix("- ") {
                    flush_current(&mut current, &section, &mut params, &mut returns);
                    current = Some(rest.to_owned());
                } else if trimmed.is_empty() {
                    flush_current(&mut current, &section, &mut params, &mut returns);
                } else if let Some(buf) = current.as_mut() {
                    // Continuation line for an in-progress bullet.
                    buf.push(' ');
                    buf.push_str(trimmed);
                }
            }
        }
    }
    flush_current(&mut current, &section, &mut params, &mut returns);

    let summary = {
        let joined = summary_lines.join("\n");
        let trimmed = joined.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    };

    let examples = {
        let joined = examples_lines.join("\n");
        let trimmed = joined.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    };

    DocBlock {
        summary,
        params: params.into_iter().collect(),
        returns,
        examples,
    }
}

/// Parse a `# Parameters` bullet entry of the form
/// `` `name` — description `` and return `(name, description)`.
fn parse_param_entry(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let rest = text.strip_prefix('`')?;
    let close = rest.find('`')?;
    let name = rest[..close].trim().to_owned();
    let after = rest[close + 1..].trim_start();
    // Accept `—`, `-`, or `:` as the separator.
    let desc = after
        .strip_prefix('—')
        .or_else(|| after.strip_prefix("--"))
        .or_else(|| after.strip_prefix('-'))
        .or_else(|| after.strip_prefix(':'))
        .unwrap_or(after)
        .trim()
        .to_owned();
    Some((name, desc))
}

/// For `# Returns` entries the leading `` `name` `` is optional;
/// strip it and any separator if present, otherwise return the
/// description verbatim.
fn strip_optional_name_prefix(text: &str) -> String {
    let t = text.trim();
    if t.starts_with('`') {
        if let Some((_, desc)) = parse_param_entry(t) {
            return desc;
        }
    }
    t.to_owned()
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
    /// `FrameLocals` parameter — passed through from the call site directly.
    FrameLocals(Ident),
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
                } else if type_is(ty, "FrameLocals") {
                    out.push(ParamKind::FrameLocals(ident));
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

/// Source of the function name used in error messages and result-context
/// patching.
///
/// Most generated bodies run inside a `NativeCall::SyncWithCtx` (or
/// similar) closure that has `__ctx: CallContext` in scope, and pull the
/// function name from `__ctx.native_name`.  The `Userdata::invoke` fast
/// path doesn't construct a `CallContext`, so it provides the function
/// name as a static expression at macro-generation time.
pub(crate) enum FunctionNameSource {
    /// Read from `__ctx.native_name` at runtime.
    Dynamic,
    /// Statically known.  The `TokenStream` produces a `&str` expression
    /// (typically a string literal) at the call site.  Generated code
    /// must not reference `__ctx`.
    Static(TokenStream),
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
        false,
        &FunctionNameSource::Dynamic,
        krate,
    )
}

pub(crate) fn gen_call_body_styled(
    fn_expr: TokenStream,
    params: &[ParamKind],
    is_async: bool,
    is_result: bool,
    error_style: ErrorStyle,
    args_borrowed: bool,
    function_name_source: &FunctionNameSource,
    krate: &CratePath,
) -> TokenStream {
    let k_for_default = krate.tokens();
    let function_name_expr: TokenStream = match function_name_source {
        FunctionNameSource::Dynamic => quote! {
            __ctx.native_name.as_ref()
                .map(|n| ::std::string::String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default()
        },
        FunctionNameSource::Static(s) => quote! { #s.to_owned() },
    };
    // For result-context patching: dynamic uses with_call_context(&__ctx),
    // static uses with_function_name(name_str).
    let with_ctx_call: TokenStream = match function_name_source {
        FunctionNameSource::Dynamic => quote! {
            #k_for_default::VmResultExt::with_call_context
        },
        FunctionNameSource::Static(_) => quote! {
            #k_for_default::VmResultExt::with_function_name
        },
    };
    let ctx_arg: TokenStream = match function_name_source {
        FunctionNameSource::Dynamic => quote! { &__ctx },
        FunctionNameSource::Static(s) => quote! { #s },
    };
    let k = krate.tokens();
    let mut extractions = Vec::<TokenStream>::new();
    let mut call_args = Vec::<TokenStream>::new();
    // 1-based Lua argument position counter (only Normal params count).
    let mut lua_arg_pos: usize = 0;

    // When any parameter is a reference type and args are borrowed (sync
    // path), we need the original slice to borrow from.  Track whether
    // we need the `__args_slice` binding.
    let needs_slice = args_borrowed
        && params
            .iter()
            .any(|p| matches!(p, ParamKind::Normal(_, ty) if is_reference_type(ty)));

    for p in params {
        match p {
            ParamKind::Normal(id, ty) if args_borrowed && is_reference_type(ty) => {
                let ty = ty.as_ref();
                lua_arg_pos += 1;
                let pos = lua_arg_pos;
                let idx = lua_arg_pos - 1;
                // Borrow directly from the args slice and use FromLuaBorrow.
                // Check Option<&T> by looking at the inner reference.
                let is_option = if let Type::Reference(r) = ty {
                    unwrap_option_inner(&r.elem).is_some()
                } else {
                    false
                };
                let inner_ty = strip_reference(ty);
                let missing_check = if is_option {
                    quote! {}
                } else {
                    quote! {
                        if #idx >= __args_slice.len() {
                            return Err(#k::VmError::BadArgument {
                                position: #pos,
                                function: #function_name_expr,
                                expected: <#inner_ty as #k::LuaTyped>::lua_type().to_string(),
                                got: "no value".to_owned(),
                            });
                        }
                    }
                };
                extractions.push(quote! {
                    #missing_check
                    let __nil = #k::Value::Nil;
                    let __borrow_ref = __args_slice.get(#idx).unwrap_or(&__nil);
                    let _ = __args.next();
                    let #id: #ty = #with_ctx_call(
                        #k::FromLuaBorrow::from_lua_borrow(__borrow_ref), #pos, #ctx_arg
                    )?;
                });
                call_args.push(quote! { #id });
            }
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
                                    function: #function_name_expr,
                                    expected: #vt.type_name().to_owned(),
                                    got: __arg.type_name().to_owned(),
                                });
                            }
                        },
                        ErrorStyle::FieldAssignment => quote! {
                            if !#k::value_matches_type(&__arg, &#vt) {
                                let __field = #function_name_expr;
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
                                    function: #function_name_expr,
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
                    let #id = #with_ctx_call(
                        #k::FromLua::from_lua(__arg), #pos, #ctx_arg
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
                                    function: #function_name_expr,
                                    expected: #vt.type_name().to_owned(),
                                    got: __arg.type_name().to_owned(),
                                });
                            }
                        },
                        ErrorStyle::FieldAssignment => quote! {
                            if !#k::value_matches_type(&__arg, &#vt) {
                                let __field = #function_name_expr;
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
                                    function: #function_name_expr,
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
                    let __binop_inner: #inner_ty = #with_ctx_call(
                        #k::FromLua::from_lua(__arg), #pos, #ctx_arg
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
            ParamKind::FrameLocals(id) => {
                extractions.push(quote! { let #id = __locals; });
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

    let args_iter = if args_borrowed && needs_slice {
        quote! {
            let __args_slice = __args;
            let mut __args = __args_slice.iter().cloned();
        }
    } else if args_borrowed {
        quote! { let mut __args = __args.iter().cloned(); }
    } else {
        quote! { let mut __args = __args.into_iter(); }
    };

    quote! {
        #args_iter
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

/// Returns `true` if the type is a reference (`&T` or `&mut T`).
fn is_reference_type(ty: &Type) -> bool {
    matches!(ty, Type::Reference(_))
}

/// Strip one layer of `&` from a type, returning the inner type.
/// Non-reference types are returned unchanged.
pub(crate) fn strip_reference(ty: &Type) -> &Type {
    if let Type::Reference(r) = ty {
        &r.elem
    } else {
        ty
    }
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

/// Token stream evaluating to `Option<String>` for a parameter's doc,
/// looked up by name.
fn doc_expr_for(param_docs: &std::collections::HashMap<String, String>, name: &str) -> TokenStream {
    match param_docs.get(name) {
        Some(doc) => quote! { ::std::option::Option::Some(#doc.to_owned()) },
        None => quote! { ::std::option::Option::None },
    }
}

/// Token stream evaluating to `Option<String>` for an arbitrary doc string.
pub(crate) fn opt_string_expr(doc: Option<&String>) -> TokenStream {
    match doc {
        Some(d) => quote! { ::std::option::Option::Some(#d.to_owned()) },
        None => quote! { ::std::option::Option::None },
    }
}

/// Generate a `Vec<ParamSpec>` token stream, a `variadic` bool, and a
/// `has_runtime_types` bool from the parameter list.  `CallContext` params
/// are skipped, `Variadic` terminates.
///
/// `param_docs`, if non-empty, supplies per-parameter documentation
/// keyed by parameter name (matching the Rust ident).
pub(crate) fn gen_param_specs(
    params: &[ParamKind],
    krate: &CratePath,
    param_docs: &std::collections::HashMap<String, String>,
) -> (TokenStream, bool, bool) {
    let k = krate.tokens();
    let mut specs = Vec::<TokenStream>::new();
    let mut has_variadic = false;
    let mut has_runtime_types = false;
    let mut variadic_multi_ty: Option<&Box<Type>> = None;

    for p in params {
        match p {
            ParamKind::Normal(ident, ty) => {
                let name_str = ident.to_string();
                let name_bytes = name_str.as_bytes().to_vec();
                // Strip references so LuaTyped and value_type resolve
                // to the concrete type (e.g. `Vec2` not `&Vec2`).
                let lua_ty = strip_reference(ty);
                let rt = if let Some(vt) = rust_type_to_value_type(lua_ty, krate) {
                    has_runtime_types = true;
                    quote! { ::std::option::Option::Some(#vt) }
                } else {
                    quote! { ::std::option::Option::None }
                };
                let doc_expr = doc_expr_for(param_docs, &name_str);
                specs.push(quote! {
                    #k::ParamSpec {
                        name: ::std::option::Option::Some(
                            #k::Bytes::from(&[ #(#name_bytes),* ][..])
                        ),
                        runtime_type: #rt,
                        lua_type: ::std::option::Option::Some(
                            <#lua_ty as #k::LuaTyped>::lua_type()
                        ),
                        doc: #doc_expr,
                    }
                });
            }
            ParamKind::BinOpSide(ident, inner_ty) => {
                let name_str = ident.to_string();
                let name_bytes = name_str.as_bytes().to_vec();
                let rt = if let Some(vt) = rust_type_to_value_type(inner_ty, krate) {
                    has_runtime_types = true;
                    quote! { ::std::option::Option::Some(#vt) }
                } else {
                    quote! { ::std::option::Option::None }
                };
                let doc_expr = doc_expr_for(param_docs, &name_str);
                specs.push(quote! {
                    #k::ParamSpec {
                        name: ::std::option::Option::Some(
                            #k::Bytes::from(&[ #(#name_bytes),* ][..])
                        ),
                        runtime_type: #rt,
                        lua_type: ::std::option::Option::Some(
                            <#inner_ty as #k::LuaTyped>::lua_type()
                        ),
                        doc: #doc_expr,
                    }
                });
            }
            ParamKind::CallContext(_) | ParamKind::FrameLocals(_) => {
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
                        doc: ::std::option::Option::None,
                    });
                }
                __specs
            }
        }
    } else {
        quote! { ::std::vec![ #(#specs),* ] }
    };
    (tokens, has_variadic, has_runtime_types)
}

/// Build a `NativeFunction` literal for a free function in a module,
/// with optional per-parameter docs.
#[allow(clippy::too_many_arguments)]
pub fn gen_native_fn_doc(
    lua_name: &str,
    fn_ident: &Ident,
    params: &[ParamKind],
    is_async: bool,
    is_result: bool,
    return_type: &Type,
    krate: &CratePath,
    module_source: Option<&[u8]>,
    param_docs: &std::collections::HashMap<String, String>,
) -> TokenStream {
    let k = krate.tokens();
    let name_bytes = lua_name.as_bytes().to_vec();
    let needs_locals = has_frame_locals(params);
    let args_borrowed = !is_async;
    let body = gen_call_body_styled(
        quote! { #fn_ident },
        params,
        is_async,
        is_result,
        ErrorStyle::BadArgument,
        args_borrowed,
        &FunctionNameSource::Dynamic,
        krate,
    );
    let (param_specs, has_variadic, has_runtime_types) = gen_param_specs(params, krate, param_docs);
    let source_expr = match module_source {
        Some(bytes) => {
            let b = bytes.to_vec();
            quote! { #k::Bytes::from(&[ #(#b),* ][..]) }
        }
        None => quote! { #k::Bytes::default() },
    };
    let call_expr = if is_async {
        if needs_locals {
            quote! {
                #k::NativeCall::AsyncWithLocals(::std::sync::Arc::new(|__ctx, __locals, __args| {
                    ::std::boxed::Box::pin(async move { #body })
                }))
            }
        } else {
            quote! {
                #k::NativeCall::Async(::std::sync::Arc::new(|__ctx, __args| {
                    ::std::boxed::Box::pin(async move { #body })
                }))
            }
        }
    } else if needs_locals {
        quote! {
            #k::NativeCall::SyncWithLocals(::std::sync::Arc::new(|__ctx, __locals, __args| {
                #body
            }))
        }
    } else if params.is_empty() {
        quote! {
            #k::NativeCall::SyncPlain(::std::sync::Arc::new(|__args| {
                #body
            }))
        }
    } else {
        quote! {
            #k::NativeCall::SyncWithCtx(::std::sync::Arc::new(|__ctx, __args| {
                #body
            }))
        }
    };
    let variadic_doc_expr = opt_string_expr(param_docs.get("..."));
    let signature = quote! {
        #k::FunctionSignature {
            name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
            source: #source_expr,
            type_params: ::std::vec::Vec::new(),
            params: #param_specs,
            variadic: #has_variadic,
            variadic_doc: #variadic_doc_expr,
            arg_offset: 0,
            returns: None,
            lua_returns: ::std::option::Option::Some(
                <#return_type as #k::LuaTypedMulti>::lua_types()
            ),
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
            has_runtime_types: #has_runtime_types,
        }
    };
    quote! {
        #k::NativeFunction {
            signature: ::std::sync::Arc::new(#signature),
            call: #call_expr,
        }
    }
}

/// Build a `FunctionSignature` literal for a function/method, used by
/// docgen-aware `module_type()` and `userdata_type()` emitters.
///
/// `arg_offset` is `1` for userdata methods (skip the implicit
/// `self`) and `0` for free module functions.
pub fn gen_function_signature(
    lua_name: &str,
    params: &[ParamKind],
    return_type: &Type,
    krate: &CratePath,
    source: &[u8],
    arg_offset: usize,
    param_docs: &std::collections::HashMap<String, String>,
) -> TokenStream {
    let k = krate.tokens();
    let name_bytes = lua_name.as_bytes().to_vec();
    let source_bytes = source.to_vec();
    let (param_specs, has_variadic, has_runtime_types) = gen_param_specs(params, krate, param_docs);
    let arg_offset_lit = syn::LitInt::new(&arg_offset.to_string(), Span::call_site());
    let variadic_doc_expr = opt_string_expr(param_docs.get("..."));
    quote! {
        #k::FunctionSignature {
            name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
            source: #k::Bytes::from(&[ #(#source_bytes),* ][..]),
            type_params: ::std::vec::Vec::new(),
            params: #param_specs,
            variadic: #has_variadic,
            variadic_doc: #variadic_doc_expr,
            arg_offset: #arg_offset_lit,
            returns: ::std::option::Option::None,
            lua_returns: ::std::option::Option::Some(
                <#return_type as #k::LuaTypedMulti>::lua_types()
            ),
            line_defined: 0,
            last_line_defined: 0,
            num_upvalues: 0,
            has_runtime_types: #has_runtime_types,
        }
    }
}

fn has_frame_locals(params: &[ParamKind]) -> bool {
    params
        .iter()
        .any(|p| matches!(p, ParamKind::FrameLocals(_)))
}
