use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse2, Attribute, Ident, Item, ItemFn, ItemMod, LitStr};

use crate::util::{
    examples_vec_expr, gen_function_signature, gen_native_fn_doc, inner_return_type,
    is_result_return, merge_param_docs, opt_string_expr, parse_doc_block, parse_params,
    promote_last_normal_to_variadic, return_is_iterator, strip_attr, CratePath, ParamKind,
    ParsedExample,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Attribute option parsing
// ---------------------------------------------------------------------------

struct ModuleOptions {
    /// Override the Lua-visible module name (default: mod ident).
    name: Option<String>,
    /// Enable strict mode (`__index`/`__newindex` raise on unknown keys).
    strict: bool,
    /// Override the crate path used in generated code (default: `::shingetsu`).
    krate: CratePath,
    /// `Some(message)` when `#[module(deprecated = "...")]` was set;
    /// propagates into [`ModuleType::deprecated`].  Bare
    /// `#[module(deprecated)]` (no value) stores the empty string.
    deprecated: Option<String>,
}

impl ModuleOptions {
    fn parse(attr: TokenStream) -> syn::Result<Self> {
        let mut opts = ModuleOptions {
            name: None,
            strict: false,
            krate: CratePath::default(),
            deprecated: None,
        };
        if attr.is_empty() {
            return Ok(opts);
        }
        // Parse as a list of `key = value` or bare flags.
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("name") {
                let val: LitStr = meta.value()?.parse()?;
                opts.name = Some(val.value());
                Ok(())
            } else if meta.path.is_ident("strict") {
                opts.strict = true;
                Ok(())
            } else if meta.path.is_ident("deprecated") {
                // Accept both `deprecated` (bare flag) and
                // `deprecated = "message"`.  The empty string is
                // the bare-flag form, matching how function-level
                // `@deprecated` without an explanation flows
                // through the rest of the pipeline.
                if let Ok(v) = meta.value() {
                    let val: LitStr = v.parse()?;
                    opts.deprecated = Some(val.value());
                } else {
                    opts.deprecated = Some(String::new());
                }
                Ok(())
            } else if meta.path.is_ident("crate") {
                let val: LitStr = meta.value()?.parse()?;
                opts.krate = CratePath::from_str(&val.value()).map_err(|e| {
                    syn::Error::new(val.span(), format!("invalid crate path: {}", e))
                })?;
                Ok(())
            } else {
                Err(meta.error("unknown module option"))
            }
        });
        syn::parse::Parser::parse2(parser, attr)?;
        Ok(opts)
    }
}

// ---------------------------------------------------------------------------
// Per-item classification
// ---------------------------------------------------------------------------

enum ModuleItem {
    Function {
        ident: Ident,
        lua_name: String,
        is_async: bool,
        is_result: bool,
        /// `true` when the return is `impl Iterator<...>`: wrapped
        /// into a Lua generic-for iter-fn on both engines
        /// (auto-detected, same rule as userdata iter methods).
        is_iter: bool,
        params: Vec<ParamKind>,
        return_type: Box<syn::Type>,
        doc: Option<String>,
        param_docs: HashMap<String, String>,
        returns_doc: Vec<String>,
        examples: Vec<ParsedExample>,
    },
    /// Eager field: a zero-argument function called once at table construction.
    EagerField {
        ident: Ident,
        lua_name: String,
        is_result: bool,
        return_type: Box<syn::Type>,
        doc: Option<String>,
        examples: Vec<ParsedExample>,
    },
    /// Read accessor: invoked on every `__index` lookup.  Spelled
    /// either `#[lazy_field]` (default name = fn ident, optional
    /// `rename`) or `#[getter("name")]` (explicit name).
    Getter {
        ident: Ident,
        lua_name: String,
        is_result: bool,
        return_type: Box<syn::Type>,
        doc: Option<String>,
        examples: Vec<ParsedExample>,
    },
    /// Write accessor: invoked on every `__newindex` lookup matching
    /// `lua_name`.  Spelled `#[setter("name")]`.  The function must
    /// take exactly one argument.
    Setter {
        ident: Ident,
        lua_name: String,
        is_result: bool,
        value_type: Box<syn::Type>,
    },
}

struct FunctionAttrOptions {
    lua_name: String,
    variadic: bool,
}

fn parse_function_attr(attr: &Attribute, default_name: &str) -> syn::Result<FunctionAttrOptions> {
    let mut opts = FunctionAttrOptions {
        lua_name: default_name.to_owned(),
        variadic: false,
    };
    if !matches!(&attr.meta, syn::Meta::Path(_)) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let val: LitStr = meta.value()?.parse()?;
                opts.lua_name = val.value();
                Ok(())
            } else if meta.path.is_ident("variadic") {
                opts.variadic = true;
                Ok(())
            } else {
                Err(meta.error("unknown attribute key"))
            }
        })?;
    }
    Ok(opts)
}

fn item_lua_name(attr: &Attribute, default: &str) -> syn::Result<String> {
    parse_function_attr(attr, default).map(|o| o.lua_name)
}

fn classify_fn(f: &mut ItemFn) -> Option<ModuleItem> {
    let fn_name = f.sig.ident.to_string();
    let is_async = f.sig.asyncness.is_some();
    let is_result = is_result_return(&f.sig.output);
    let doc_block = parse_doc_block(&f.attrs);
    let per_arg_docs = crate::util::extract_and_strip_param_docs(&mut f.sig);

    if let Some(attr) = f
        .attrs
        .iter()
        .find(|a| a.path().is_ident("function"))
        .cloned()
    {
        let fn_opts = parse_function_attr(&attr, &fn_name).ok()?;
        let mut params = parse_params(&f.sig);
        if fn_opts.variadic {
            promote_last_normal_to_variadic(&mut params);
        }
        strip_attr(&mut f.attrs, "function");
        let return_type = inner_return_type(&f.sig.output);
        let is_iter = return_is_iterator(&f.sig.output);
        return Some(ModuleItem::Function {
            ident: f.sig.ident.clone(),
            lua_name: fn_opts.lua_name,
            is_async,
            is_result,
            is_iter,
            params,
            return_type,
            doc: doc_block.summary,
            param_docs: merge_param_docs(doc_block.params, per_arg_docs),
            returns_doc: doc_block.returns,
            examples: doc_block.examples,
        });
    }

    if let Some(attr) = f.attrs.iter().find(|a| a.path().is_ident("field")).cloned() {
        let lua_name = item_lua_name(&attr, &fn_name).ok()?;
        strip_attr(&mut f.attrs, "field");
        let return_type = inner_return_type(&f.sig.output);
        return Some(ModuleItem::EagerField {
            ident: f.sig.ident.clone(),
            lua_name,
            is_result,
            return_type,
            doc: doc_block.summary,
            examples: doc_block.examples,
        });
    }

    if let Some(attr) = f
        .attrs
        .iter()
        .find(|a| a.path().is_ident("lazy_field"))
        .cloned()
    {
        let lua_name = item_lua_name(&attr, &fn_name).ok()?;
        strip_attr(&mut f.attrs, "lazy_field");
        let return_type = inner_return_type(&f.sig.output);
        return Some(ModuleItem::Getter {
            ident: f.sig.ident.clone(),
            lua_name,
            is_result,
            return_type,
            doc: doc_block.summary,
            examples: doc_block.examples,
        });
    }

    if let Some(attr) = f
        .attrs
        .iter()
        .find(|a| a.path().is_ident("getter"))
        .cloned()
    {
        let lua_name = parse_accessor_name(&attr, &fn_name, "get_").ok()?;
        strip_attr(&mut f.attrs, "getter");
        let return_type = inner_return_type(&f.sig.output);
        return Some(ModuleItem::Getter {
            ident: f.sig.ident.clone(),
            lua_name,
            is_result,
            return_type,
            doc: doc_block.summary,
            examples: doc_block.examples,
        });
    }

    if let Some(attr) = f
        .attrs
        .iter()
        .find(|a| a.path().is_ident("setter"))
        .cloned()
    {
        let lua_name = parse_accessor_name(&attr, &fn_name, "set_").ok()?;
        strip_attr(&mut f.attrs, "setter");
        let value_type = setter_value_type(&f.sig)?;
        return Some(ModuleItem::Setter {
            ident: f.sig.ident.clone(),
            lua_name,
            is_result,
            value_type,
        });
    }

    None
}

/// Parse the accessor name from `#[getter("name")]` /
/// `#[setter("name")]`.  If the attribute is bare
/// (`#[getter]` / `#[setter]`), strip a `get_` / `set_` prefix
/// from the function ident as the lua name.
fn parse_accessor_name(attr: &Attribute, fn_name: &str, prefix: &str) -> syn::Result<String> {
    if let syn::Meta::Path(_) = &attr.meta {
        return Ok(fn_name.strip_prefix(prefix).unwrap_or(fn_name).to_owned());
    }
    // Accept `#[getter("x")]`, `#[getter(name = "x")]`, or `#[getter(rename = "x")]`.
    let mut name: Option<String> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") || meta.path.is_ident("rename") {
            let val: LitStr = meta.value()?.parse()?;
            name = Some(val.value());
            Ok(())
        } else {
            Err(meta.error("unknown accessor attribute key; expected `name` or `rename`"))
        }
    });
    // Try positional string literal first: `#[getter("name")]`.
    if let Ok(lit) = attr.parse_args::<LitStr>() {
        return Ok(lit.value());
    }
    attr.parse_args_with(parser)?;
    name.ok_or_else(|| {
        syn::Error::new_spanned(
            attr,
            "expected `#[getter(\"name\")]` or `#[getter(rename = \"name\")]`",
        )
    })
}

fn setter_value_type(sig: &syn::Signature) -> Option<Box<syn::Type>> {
    use syn::FnArg;
    let last = sig.inputs.iter().last()?;
    let pat_ty = match last {
        FnArg::Typed(p) => p,
        FnArg::Receiver(_) => return None,
    };
    Some(pat_ty.ty.clone())
}

// ---------------------------------------------------------------------------
// Main expansion
// ---------------------------------------------------------------------------

/// Wrapper that emits only the shingetsu-side wiring.
pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_inner(attr, item, false)
}

/// Wrapper that emits both shingetsu-side and mlua-side wiring,
/// used by the migration facade.
pub fn expand_facade(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_inner(attr, item, true)
}

fn expand_inner(attr: TokenStream, item: TokenStream, also_emit_mlua: bool) -> TokenStream {
    let opts = match ModuleOptions::parse(attr) {
        Ok(o) => o,
        Err(e) => return e.into_compile_error(),
    };

    let mut mod_item: ItemMod = match parse2(item) {
        Ok(v) => v,
        Err(e) => return e.into_compile_error(),
    };

    let mod_ident = &mod_item.ident.clone();
    let lua_mod_name = opts.name.unwrap_or_else(|| mod_ident.to_string());
    let lua_mod_name_bytes = lua_mod_name.as_bytes().to_vec();
    let krate = &opts.krate;
    let k = krate.tokens();

    let content = match &mut mod_item.content {
        Some((_, items)) => items,
        None => {
            return syn::Error::new_spanned(&mod_item.ident, "module must have a body")
                .into_compile_error()
        }
    };

    let mut classified: Vec<ModuleItem> = Vec::new();

    for item in content.iter_mut() {
        if let Item::Fn(f) = item {
            if let Some(ci) = classify_fn(f) {
                classified.push(ci);
            }
        }
    }

    // Generate table-building statements.
    let mut table_stmts: Vec<TokenStream> = Vec::new();

    // Collect getter / setter dispatch arms for the metatable closures.
    let mut getter_arms: Vec<TokenStream> = Vec::new();
    let mut setter_arms: Vec<TokenStream> = Vec::new();

    for ci in &classified {
        match ci {
            ModuleItem::Function {
                ident,
                lua_name,
                is_async,
                is_result,
                is_iter,
                params,
                return_type,
                param_docs,
                ..
            } => {
                if *is_iter && *is_async {
                    table_stmts.push(
                        syn::Error::new_spanned(
                            ident,
                            "a `#[function]` returning `impl Iterator<...>` cannot be \
                             `async`; the iterator materializes synchronously when the \
                             returned iter-fn is first called",
                        )
                        .into_compile_error(),
                    );
                    continue;
                }
                let key_bytes = lua_name.as_bytes().to_vec();
                let source = format!("=[{lua_mod_name}]");
                // Iter functions surface as a returned function; the
                // written `impl Iterator<...>` type is not
                // `LuaTypedMulti`, so describe the return as the
                // krate `Function` type.
                let iter_rt: syn::Type = {
                    let k = krate.tokens();
                    syn::parse_quote! { #k::Function }
                };
                let return_type: &syn::Type = if *is_iter { &iter_rt } else { return_type };
                let native = gen_native_fn_doc(
                    lua_name,
                    ident,
                    params,
                    *is_async,
                    *is_result,
                    return_type,
                    krate,
                    Some(source.as_bytes()),
                    param_docs,
                    *is_iter,
                );
                table_stmts.push(quote! {
                    {
                        let __f = #native;
                        __table.raw_set(
                            #k::Value::String(
                                #k::Bytes::from(&[ #(#key_bytes),* ][..])
                            ),
                            #k::Value::Function(
                                #k::Function::native(__f)
                            ),
                        )?
                    }
                });
            }
            ModuleItem::EagerField {
                ident,
                lua_name,
                is_result,
                ..
            } => {
                let key_bytes = lua_name.as_bytes().to_vec();
                let call_expr = if *is_result {
                    quote! { #ident().map_err(::std::convert::Into::into)? }
                } else {
                    quote! { #ident() }
                };
                table_stmts.push(quote! {
                    {
                        let __v = #k::IntoLua::into_lua(#call_expr);
                        __table.raw_set(
                            #k::Value::String(
                                #k::Bytes::from(&[ #(#key_bytes),* ][..])
                            ),
                            __v,
                        )?
                    }
                });
            }
            ModuleItem::Getter {
                ident,
                lua_name,
                is_result,
                ..
            } => {
                let key_bytes = lua_name.as_bytes().to_vec();
                let call_expr = if *is_result {
                    quote! { #ident().map_err(::std::convert::Into::into)? }
                } else {
                    quote! { #ident() }
                };
                getter_arms.push(quote! {
                    [ #(#key_bytes),* ] => {
                        return ::std::result::Result::Ok(
                            #k::IntoLua::into_lua(#call_expr)
                        );
                    }
                });
            }
            ModuleItem::Setter {
                ident,
                lua_name,
                is_result,
                value_type,
            } => {
                let key_bytes = lua_name.as_bytes().to_vec();
                let call_expr = if *is_result {
                    quote! { #ident(__v).map_err(::std::convert::Into::into)? }
                } else {
                    quote! { { #ident(__v); } }
                };
                setter_arms.push(quote! {
                    [ #(#key_bytes),* ] => {
                        let __v = <#value_type as #k::FromLua>::from_lua(__value, env)?;
                        #call_expr
                        return ::std::result::Result::Ok(());
                    }
                });
            }
        }
    }

    // If any getters or setters exist, build a metatable with __index /
    // __newindex closures and attach it to the module table.
    let metatable_stmt = if !getter_arms.is_empty() || !setter_arms.is_empty() {
        let index_name = format!("{lua_mod_name}.__index");
        let newindex_name = format!("{lua_mod_name}.__newindex");
        let index_fn = if !getter_arms.is_empty() {
            quote! {
                let __index_fn = #k::Function::wrap(
                    #index_name,
                    |__self_table: #k::Table, __key: #k::Value|
                        -> ::std::result::Result<#k::Value, #k::VmError>
                    {
                        if let #k::Value::String(ref __sb) = __key {
                            let __bytes: &[u8] = __sb.as_ref();
                            match __bytes {
                                #(#getter_arms)*
                                _ => {}
                            }
                        }
                        // Fall through: native table key (function or eager field).
                        __self_table.raw_get(&__key)
                    },
                );
                __mt.raw_set(
                    #k::Value::String(#k::Bytes::from(b"__index")),
                    #k::Value::Function(__index_fn),
                )?;
            }
        } else {
            quote! {}
        };
        let newindex_fn = if !setter_arms.is_empty() {
            quote! {
                let __newindex_fn = #k::Function::wrap(
                    #newindex_name,
                    |__ctx: #k::CallContext, __self_table: #k::Table, __key: #k::Value, __value: #k::Value|
                        -> ::std::result::Result<(), #k::VmError>
                    {
                        let env = &__ctx.global;
                        if let #k::Value::String(ref __sb) = __key {
                            let __bytes: &[u8] = __sb.as_ref();
                            match __bytes {
                                #(#setter_arms)*
                                _ => {}
                            }
                        }
                        __self_table.raw_set(__key, __value)
                    },
                );
                __mt.raw_set(
                    #k::Value::String(#k::Bytes::from(b"__newindex")),
                    #k::Value::Function(__newindex_fn),
                )?;
            }
        } else {
            quote! {}
        };
        quote! {
            {
                let __mt = #k::Table::new();
                #index_fn
                #newindex_fn
                __table.set_metatable(::std::option::Option::Some(__mt))?;
            }
        }
    } else {
        quote! {}
    };

    // Generate FieldDef / FunctionDef entries for module_type().
    let module_source = format!("=[{lua_mod_name}]");
    let mut field_stmts: Vec<TokenStream> = Vec::new();
    let mut function_stmts: Vec<TokenStream> = Vec::new();
    for ci in &classified {
        match ci {
            ModuleItem::Function {
                lua_name,
                params,
                return_type,
                is_iter,
                doc,
                param_docs,
                returns_doc,
                examples,
                ..
            } => {
                let name_bytes = lua_name.as_bytes().to_vec();
                let doc_expr = opt_string_expr(doc.as_ref());
                let examples_expr = examples_vec_expr(examples, krate);
                let iter_rt: syn::Type = {
                    let k = krate.tokens();
                    syn::parse_quote! { #k::Function }
                };
                let return_type: &syn::Type = if *is_iter { &iter_rt } else { return_type };
                let signature = gen_function_signature(
                    lua_name,
                    params,
                    return_type,
                    krate,
                    module_source.as_bytes(),
                    0,
                    param_docs,
                );
                let returns_doc_lits: Vec<TokenStream> = returns_doc
                    .iter()
                    .map(|s| quote! { #s.to_owned() })
                    .collect();
                function_stmts.push(quote! {
                    __functions.push(#k::types::FunctionDef {
                        name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                        doc: #doc_expr,
                        signature: #signature,
                        returns_doc: ::std::vec![ #(#returns_doc_lits),* ],
                        examples: #examples_expr,
                    });
                });
            }
            ModuleItem::EagerField {
                lua_name,
                return_type,
                doc,
                examples,
                ..
            } => {
                let name_bytes = lua_name.as_bytes().to_vec();
                let doc_expr = opt_string_expr(doc.as_ref());
                let examples_expr = examples_vec_expr(examples, krate);
                field_stmts.push(quote! {
                    __fields.push(#k::types::FieldDef {
                        name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                        doc: #doc_expr,
                        lua_type: <#return_type as #k::LuaTyped>::lua_type(),
                        kind: #k::types::FieldKind::Eager,
                        examples: #examples_expr,
                                            deprecated: ::std::option::Option::None,
                    });
                });
            }
            // Getter / Setter type metadata is emitted in a second pass
            // below so we can pair them into ReadWrite where applicable.
            ModuleItem::Getter { .. } | ModuleItem::Setter { .. } => {}
        }
    }

    // Pair getter/setter by lua_name into FieldDefs with matching FieldKind.
    {
        use std::collections::BTreeMap;
        struct Acc<'a> {
            getter: Option<(
                &'a Box<syn::Type>,
                &'a Option<String>,
                &'a Vec<ParsedExample>,
            )>,
            setter: Option<&'a Box<syn::Type>>,
        }
        let mut by_name: BTreeMap<&str, Acc> = BTreeMap::new();
        for ci in &classified {
            match ci {
                ModuleItem::Getter {
                    lua_name,
                    return_type,
                    doc,
                    examples,
                    ..
                } => {
                    by_name
                        .entry(lua_name.as_str())
                        .or_insert(Acc {
                            getter: None,
                            setter: None,
                        })
                        .getter = Some((return_type, doc, examples));
                }
                ModuleItem::Setter {
                    lua_name,
                    value_type,
                    ..
                } => {
                    by_name
                        .entry(lua_name.as_str())
                        .or_insert(Acc {
                            getter: None,
                            setter: None,
                        })
                        .setter = Some(value_type);
                }
                _ => {}
            }
        }
        for (name, acc) in by_name {
            let name_bytes = name.as_bytes().to_vec();
            let (lua_type_expr, doc_expr, examples_expr, kind_tokens) =
                match (acc.getter, acc.setter) {
                    (Some((rt, doc, ex)), Some(_)) => (
                        quote! { <#rt as #k::LuaTyped>::lua_type() },
                        opt_string_expr(doc.as_ref()),
                        examples_vec_expr(ex, krate),
                        quote! { #k::types::FieldKind::ReadWrite },
                    ),
                    (Some((rt, doc, ex)), None) => (
                        quote! { <#rt as #k::LuaTyped>::lua_type() },
                        opt_string_expr(doc.as_ref()),
                        examples_vec_expr(ex, krate),
                        quote! { #k::types::FieldKind::Getter },
                    ),
                    (None, Some(vt)) => (
                        quote! { <#vt as #k::LuaTyped>::lua_type() },
                        quote! { ::std::option::Option::None },
                        quote! { ::std::vec::Vec::new() },
                        quote! { #k::types::FieldKind::Setter },
                    ),
                    (None, None) => unreachable!("map key implies at least one"),
                };
            field_stmts.push(quote! {
                __fields.push(#k::types::FieldDef {
                    name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                    doc: #doc_expr,
                    lua_type: #lua_type_expr,
                    kind: #kind_tokens,
                    examples: #examples_expr,
                                    deprecated: ::std::option::Option::None,
                });
            });
        }
    }

    // Module-level doc and strict flag.
    let mod_doc = parse_doc_block(&mod_item.attrs).summary;
    let mod_doc_expr = opt_string_expr(mod_doc.as_ref());
    let strict_lit = if opts.strict {
        quote! { true }
    } else {
        quote! { false }
    };
    let deprecated_expr = opt_string_expr(opts.deprecated.as_ref());

    // Inject generated functions into the mod body.
    let generated_fns = quote! {
        pub fn build_module_table(
            _env: &#k::GlobalEnv,
        ) -> ::std::result::Result<#k::Table, #k::VmError> {
            use #k::{Function, IntoLua, IntoLuaMulti};
            let __table = #k::Table::new();
            #(#table_stmts)*
            #metatable_stmt
            ::std::result::Result::Ok(__table)
        }

        pub fn register_global_module(
            env: &#k::GlobalEnv,
        ) -> ::std::result::Result<(), #k::VmError> {
            let __table = build_module_table(env)?;
            env.set_global(
                #k::Bytes::from(&[ #(#lua_mod_name_bytes),* ][..]),
                #k::Value::Table(__table),
            );
            ::std::result::Result::Ok(())
        }

        /// Build compile-time type info for this module.
        ///
        /// Returns a [`ModuleTypeInfo`] describing the module's exported
        /// functions so the compiler can type-check `require`'d calls
        /// without loading the module at runtime.
        pub fn module_type() -> #k::types::ModuleTypeInfo {
            let mut __fields: ::std::vec::Vec<#k::types::FieldDef> =
                ::std::vec::Vec::new();
            let mut __functions: ::std::vec::Vec<#k::types::FunctionDef> =
                ::std::vec::Vec::new();
            #(#field_stmts)*
            #(#function_stmts)*
            #k::types::ModuleTypeInfo {
                exported_types: ::std::collections::HashMap::new(),
                return_location: ::std::option::Option::None,
                has_explicit_return: false,
                documented_locals: ::std::vec::Vec::new(),
                module_return_local: ::std::option::Option::None,
                return_type: ::std::option::Option::Some(
                    #k::types::LuaType::Module(::std::boxed::Box::new(
                        #k::types::ModuleType {
                            name: #k::Bytes::from(&[ #(#lua_mod_name_bytes),* ][..]),
                            doc: #mod_doc_expr,
                            strict: #strict_lit,
                            fields: __fields,
                            functions: __functions,
                            methods: ::std::vec::Vec::new(),
                            metamethods: ::std::vec::Vec::new(),
                            deprecated: #deprecated_expr,
                        }
                    ))
                ),
            }
        }

        /// Register this module as a `require`-able preload entry.
        ///
        /// After the first `require(name)` call the result is cached in
        /// `package.loaded`; subsequent calls return the cached value.
        /// Also registers compile-time type info so the type checker can
        /// verify calls on the `require`'d module.
        pub fn register_preload(env: &#k::GlobalEnv) {
            env.register_preload_typed(
                #k::Bytes::from(&[ #(#lua_mod_name_bytes),* ][..]),
                build_module_table,
                module_type(),
            );
        }
    };

    let mlua_fns = if also_emit_mlua {
        match gen_mlua_module_fns(&classified, &lua_mod_name) {
            Ok(ts) => ts,
            Err(e) => return e.into_compile_error(),
        }
    } else {
        TokenStream::new()
    };

    let combined = quote! {
        #generated_fns
        #mlua_fns
    };

    if let Some((_, items)) = &mut mod_item.content {
        // Parse the generated functions and push them into the mod.
        match parse2::<syn::File>(combined) {
            Ok(f) => items.extend(f.items),
            Err(e) => return e.into_compile_error(),
        }
    }

    quote! { #mod_item }
}

// ---------------------------------------------------------------------------
// mlua-side codegen for the migration facade
// ---------------------------------------------------------------------------

/// Generate `build_mlua_module_table` and `register_mlua_module`
/// functions to splice into the module body alongside the
/// shingetsu-side ones.
///
/// `build_mlua_module_table(lua) -> mlua::Result<mlua::Table>`
/// returns a populated table; the host decides where to attach it
/// (typical sub-module pattern).
///
/// `register_mlua_module(lua) -> mlua::Result<()>` sets the table
/// at the top-level global named after the module.  Mirror of
/// shingetsu's `register_global_module(env)`.
fn gen_mlua_module_fns(classified: &[ModuleItem], lua_mod_name: &str) -> syn::Result<TokenStream> {
    let mut stmts: Vec<TokenStream> = Vec::new();
    let mut getter_arms: Vec<TokenStream> = Vec::new();
    let mut setter_arms: Vec<TokenStream> = Vec::new();

    for ci in classified {
        match ci {
            ModuleItem::Function {
                ident,
                lua_name,
                is_async,
                is_result,
                is_iter,
                params,
                return_type: _,
                ..
            } => {
                stmts.push(gen_mlua_function_stmt(
                    ident, lua_name, *is_async, *is_result, *is_iter, params,
                )?);
            }
            ModuleItem::EagerField {
                ident,
                lua_name,
                is_result,
                ..
            } => {
                let call_expr = if *is_result {
                    quote! { #ident().map_err(::mlua::Error::external)? }
                } else {
                    quote! { #ident() }
                };
                stmts.push(quote! {
                    __table.set(#lua_name, #call_expr)?;
                });
            }
            ModuleItem::Getter {
                ident,
                lua_name,
                is_result,
                ..
            } => {
                let call_expr = if *is_result {
                    quote! { #ident().map_err(::mlua::Error::external)? }
                } else {
                    quote! { #ident() }
                };
                getter_arms.push(quote! {
                    #lua_name => {
                        return ::mlua::IntoLua::into_lua(#call_expr, __lua_inner);
                    }
                });
            }
            ModuleItem::Setter {
                ident,
                lua_name,
                is_result,
                value_type,
            } => {
                let call_expr = if *is_result {
                    quote! { #ident(__v).map_err(::mlua::Error::external)? }
                } else {
                    quote! { { #ident(__v); } }
                };
                setter_arms.push(quote! {
                    #lua_name => {
                        let __v: #value_type =
                            <#value_type as ::mlua::FromLua>::from_lua(__value, __lua_inner)?;
                        #call_expr
                        return ::std::result::Result::Ok(());
                    }
                });
            }
        }
    }

    let metatable_stmt = if !getter_arms.is_empty() || !setter_arms.is_empty() {
        let index_fn = if !getter_arms.is_empty() {
            quote! {
                let __index_fn = __lua.create_function(
                    |__lua_inner: &::mlua::Lua,
                     (__self_table, __key): (::mlua::Table, ::mlua::Value)|
                        -> ::mlua::Result<::mlua::Value>
                    {
                        if let ::mlua::Value::String(ref __sb) = __key {
                            let __s = __sb.to_str()?;
                            let __s_ref: &str = &__s;
                            match __s_ref {
                                #(#getter_arms)*
                                _ => {}
                            }
                        }
                        // Fall through: raw read of native table keys
                        // (functions and eager fields).
                        __self_table.raw_get(__key)
                    },
                )?;
                __mt.set("__index", __index_fn)?;
            }
        } else {
            quote! {}
        };
        let newindex_fn = if !setter_arms.is_empty() {
            quote! {
                let __newindex_fn = __lua.create_function(
                    |__lua_inner: &::mlua::Lua,
                     (__self_table, __key, __value): (
                         ::mlua::Table, ::mlua::Value, ::mlua::Value,
                     )|
                        -> ::mlua::Result<()>
                    {
                        if let ::mlua::Value::String(ref __sb) = __key {
                            let __s = __sb.to_str()?;
                            let __s_ref: &str = &__s;
                            match __s_ref {
                                #(#setter_arms)*
                                _ => {}
                            }
                        }
                        __self_table.raw_set(__key, __value)
                    },
                )?;
                __mt.set("__newindex", __newindex_fn)?;
            }
        } else {
            quote! {}
        };
        quote! {
            {
                let __mt = __lua.create_table()?;
                #index_fn
                #newindex_fn
                __table.set_metatable(::std::option::Option::Some(__mt))?;
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        /// Build a populated mlua module table.  The host decides
        /// where to attach it (typically a sub-module of a host-
        /// owned top-level table).
        pub fn build_mlua_module_table(
            __lua: &::mlua::Lua,
        ) -> ::mlua::Result<::mlua::Table> {
            let __table = __lua.create_table()?;
            #(#stmts)*
            #metatable_stmt
            ::std::result::Result::Ok(__table)
        }

        /// Register the module as a top-level global named after the
        /// `#[module(name = ...)]` (or the mod ident if unset).
        /// Mirrors the shingetsu-side `register_global_module`.
        pub fn register_mlua_module(__lua: &::mlua::Lua) -> ::mlua::Result<()> {
            let __table = build_mlua_module_table(__lua)?;
            __lua.globals().set(#lua_mod_name, __table)?;
            ::std::result::Result::Ok(())
        }
    })
}

fn gen_mlua_function_stmt(
    ident: &syn::Ident,
    lua_name: &str,
    is_async: bool,
    is_result: bool,
    is_iter: bool,
    params: &[ParamKind],
) -> syn::Result<TokenStream> {
    if is_iter && is_async {
        return Err(syn::Error::new_spanned(
            ident,
            "a `#[function]` returning `impl Iterator<...>` cannot be `async`",
        ));
    }
    // Build the (idents,) tuple pattern and (types,) tuple type for the
    // mlua-side closure.  Reject parameter kinds that have no mlua
    // equivalent (CallContext, GlobalEnv, FrameLocals) so the user
    // gets a clear error rather than a confusing trait-bound failure.
    let mut idents: Vec<syn::Ident> = Vec::new();
    let mut types: Vec<TokenStream> = Vec::new();
    let mut call_args: Vec<TokenStream> = Vec::new();
    for p in params {
        match p {
            ParamKind::Normal(id, ty) => {
                idents.push(id.clone());
                types.push(quote! { #ty });
                call_args.push(quote! { #id });
            }
            ParamKind::Variadic(id) => {
                // Bare `Variadic` (untyped, `Variadic(ValueVec)` on
                // shingetsu / `Variadic<Value>` on mlua) doesn't
                // bridge cleanly: the engines' `Value` types differ,
                // so a single body can't read either.  Use a typed
                // `shingetsu_migrate::Variadic<T>` bridge with
                // `#[function(variadic)]` instead.
                return Err(syn::Error::new_spanned(
                    id,
                    "the migration facade does not yet mirror untyped `Variadic` on the \
                     mlua side; use a typed `shingetsu_migrate::Variadic<T>` with \
                     `#[function(variadic)]`, or keep this function on `#[shingetsu::module]`",
                ));
            }
            ParamKind::VariadicMulti(id, ty) => {
                // The user's parameter type already impls
                // `mlua::FromLuaMulti` (e.g. `Variadic<T>` from this
                // crate, or `mlua::Variadic<T>`).  Pass it straight
                // through; mlua's tuple `FromLuaMulti` invokes
                // `FromLuaMulti` for the last tuple element so the
                // remaining args flow into the variadic.
                idents.push(id.clone());
                types.push(quote! { #ty });
                call_args.push(quote! { #id });
            }
            ParamKind::CallContext(id) | ParamKind::GlobalEnv(id) | ParamKind::FrameLocals(id) => {
                return Err(syn::Error::new_spanned(
                    id,
                    "the migration facade cannot mirror `CallContext`, `GlobalEnv`, \
                     or `FrameLocals` parameters on the mlua side because mlua has \
                     no equivalent.  Use `#[shingetsu::module]` for engine-coupled \
                     functions, or restructure to take only lua-visible parameters",
                ));
            }
            ParamKind::BinOpSide(id, _) => {
                return Err(syn::Error::new_spanned(
                    id,
                    "the migration facade does not yet mirror `BinOpSide` parameters \
                     on the mlua side",
                ));
            }
        }
    }

    let call_expr = if is_result {
        quote! { #ident(#(#call_args,)*).map_err(::mlua::Error::external) }
    } else {
        quote! { ::std::result::Result::Ok(#ident(#(#call_args,)*)) }
    };

    let create = if is_async {
        // `create_async_function` takes the closure with `Lua` by
        // value (not reference) per mlua's design, and the user fn
        // is awaited.
        let async_call = if is_result {
            quote! { #ident(#(#call_args,)*).await.map_err(::mlua::Error::external) }
        } else {
            quote! { ::std::result::Result::Ok(#ident(#(#call_args,)*).await) }
        };
        quote! {
            __lua.create_async_function(
                |_: ::mlua::Lua, ( #(#idents,)* ): ( #(#types,)* )| async move {
                    #async_call
                },
            )?
        }
    } else if is_iter {
        let materialise = if is_result {
            quote! {
                let __iter = #ident(#(#call_args,)*)
                    .map_err(::mlua::Error::external)?;
            }
        } else {
            quote! { let __iter = #ident(#(#call_args,)*); }
        };
        quote! {
            __lua.create_function(
                |__lua: &::mlua::Lua, ( #(#idents,)* ): ( #(#types,)* )|
                    -> ::mlua::Result<::mlua::Function> {
                    #materialise
                    let __state = ::std::sync::Arc::new(
                        ::parking_lot::Mutex::new(::std::boxed::Box::new(__iter)),
                    );
                    let __iter_state = ::std::sync::Arc::clone(&__state);
                    __lua.create_function_mut(
                        move |__lua: &::mlua::Lua, (_s, _c): (
                            ::mlua::Value,
                            ::mlua::Value,
                        )| -> ::mlua::Result<::mlua::MultiValue> {
                            match __iter_state.lock().next() {
                                ::std::option::Option::Some(__item) => {
                                    ::mlua::IntoLuaMulti::into_lua_multi(
                                        __item, __lua,
                                    )
                                }
                                ::std::option::Option::None => {
                                    ::std::result::Result::Ok(
                                        ::mlua::MultiValue::new(),
                                    )
                                }
                            }
                        },
                    )
                },
            )?
        }
    } else {
        quote! {
            __lua.create_function(
                |_: &::mlua::Lua, ( #(#idents,)* ): ( #(#types,)* )| {
                    #call_expr
                },
            )?
        }
    };

    Ok(quote! {
        {
            let __f = #create;
            __table.set(#lua_name, __f)?;
        }
    })
}
