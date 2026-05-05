use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse2, Attribute, Ident, Item, ItemFn, ItemMod, LitStr};

use crate::util::{
    gen_function_signature, gen_native_fn_doc, inner_return_type, is_result_return,
    opt_string_expr, parse_doc_block, parse_params, strip_attr, CratePath, ParamKind,
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
}

impl ModuleOptions {
    fn parse(attr: TokenStream) -> syn::Result<Self> {
        let mut opts = ModuleOptions {
            name: None,
            strict: false,
            krate: CratePath::default(),
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
        params: Vec<ParamKind>,
        return_type: Box<syn::Type>,
        doc: Option<String>,
        param_docs: HashMap<String, String>,
        returns_doc: Vec<String>,
        examples: Option<String>,
    },
    /// Eager field: a zero-argument function called once at table construction.
    EagerField {
        ident: Ident,
        lua_name: String,
        is_result: bool,
        return_type: Box<syn::Type>,
        doc: Option<String>,
        examples: Option<String>,
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

    if let Some(attr) = f
        .attrs
        .iter()
        .find(|a| a.path().is_ident("function"))
        .cloned()
    {
        let fn_opts = parse_function_attr(&attr, &fn_name).ok()?;
        let mut params = parse_params(&f.sig);
        if fn_opts.variadic {
            // Convert the last Normal param into VariadicMulti.
            let last_normal = params
                .iter()
                .rposition(|p| matches!(p, ParamKind::Normal(_, _)));
            if let Some(idx) = last_normal {
                let old = params.remove(idx);
                if let ParamKind::Normal(ident, ty) = old {
                    params.insert(idx, ParamKind::VariadicMulti(ident, ty));
                }
            }
        }
        strip_attr(&mut f.attrs, "function");
        let return_type = inner_return_type(&f.sig.output);
        return Some(ModuleItem::Function {
            ident: f.sig.ident.clone(),
            lua_name: fn_opts.lua_name,
            is_async,
            is_result,
            params,
            return_type,
            doc: doc_block.summary,
            param_docs: doc_block.params,
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

    None
}

// ---------------------------------------------------------------------------
// Main expansion
// ---------------------------------------------------------------------------

pub fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
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

    for ci in &classified {
        match ci {
            ModuleItem::Function {
                ident,
                lua_name,
                is_async,
                is_result,
                params,
                return_type,
                param_docs,
                ..
            } => {
                let key_bytes = lua_name.as_bytes().to_vec();
                let source = format!("=[{lua_mod_name}]");
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
        }
    }

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
                doc,
                param_docs,
                returns_doc,
                examples,
                ..
            } => {
                let name_bytes = lua_name.as_bytes().to_vec();
                let doc_expr = opt_string_expr(doc.as_ref());
                let examples_expr = opt_string_expr(examples.as_ref());
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
                let examples_expr = opt_string_expr(examples.as_ref());
                field_stmts.push(quote! {
                    __fields.push(#k::types::FieldDef {
                        name: #k::Bytes::from(&[ #(#name_bytes),* ][..]),
                        doc: #doc_expr,
                        lua_type: <#return_type as #k::LuaTyped>::lua_type(),
                        kind: #k::types::FieldKind::Eager,
                        examples: #examples_expr,
                    });
                });
            }
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

    // Inject generated functions into the mod body.
    let generated_fns = quote! {
        pub fn build_module_table(
            _env: &#k::GlobalEnv,
        ) -> ::std::result::Result<#k::Table, #k::VmError> {
            use #k::{Function, IntoLua, IntoLuaMulti};
            let __table = #k::Table::new();
            #(#table_stmts)*
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

    if let Some((_, items)) = &mut mod_item.content {
        // Parse the generated functions and push them into the mod.
        match parse2::<syn::File>(generated_fns) {
            Ok(f) => items.extend(f.items),
            Err(e) => return e.into_compile_error(),
        }
    }

    quote! { #mod_item }
}
