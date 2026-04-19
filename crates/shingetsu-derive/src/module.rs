use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse2, Attribute, Ident, Item, ItemFn, ItemMod, LitStr};

use crate::util::{
    gen_native_fn, inner_return_type, is_result_return, parse_params, strip_attr, CratePath,
};

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
        params: Vec<crate::util::ParamKind>,
        return_type: Box<syn::Type>,
    },
    /// Eager field: a zero-argument function called once at table construction.
    EagerField {
        ident: Ident,
        lua_name: String,
        is_result: bool,
        return_type: Box<syn::Type>,
    },
}

fn item_lua_name(attr: &Attribute, default: &str) -> syn::Result<String> {
    let mut name = default.to_owned();
    if !matches!(&attr.meta, syn::Meta::Path(_)) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let val: LitStr = meta.value()?.parse()?;
                name = val.value();
                Ok(())
            } else {
                Err(meta.error("unknown attribute key"))
            }
        })?;
    }
    Ok(name)
}

fn classify_fn(f: &mut ItemFn) -> Option<ModuleItem> {
    let fn_name = f.sig.ident.to_string();
    let is_async = f.sig.asyncness.is_some();
    let is_result = is_result_return(&f.sig.output);

    if let Some(attr) = f
        .attrs
        .iter()
        .find(|a| a.path().is_ident("function"))
        .cloned()
    {
        let lua_name = item_lua_name(&attr, &fn_name).ok()?;
        let params = parse_params(&f.sig);
        strip_attr(&mut f.attrs, "function");
        let return_type = inner_return_type(&f.sig.output);
        return Some(ModuleItem::Function {
            ident: f.sig.ident.clone(),
            lua_name,
            is_async,
            is_result,
            params,
            return_type,
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
            } => {
                let key_bytes = lua_name.as_bytes().to_vec();
                let source = format!("=[{lua_mod_name}]");
                let native = gen_native_fn(
                    lua_name,
                    ident,
                    params,
                    *is_async,
                    *is_result,
                    return_type,
                    krate,
                    Some(source.as_bytes()),
                );
                table_stmts.push(quote! {
                    {
                        let __f = #native;
                        __table.raw_set(
                            #k::Value::String(
                                #k::bytes::Bytes::from_static(&[ #(#key_bytes),* ])
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
                                #k::bytes::Bytes::from_static(&[ #(#key_bytes),* ])
                            ),
                            __v,
                        )?
                    }
                });
            }
        }
    }

    // Generate field type entries for module_type().
    let mut type_field_stmts: Vec<TokenStream> = Vec::new();
    for ci in &classified {
        match ci {
            ModuleItem::Function {
                lua_name,
                params,
                return_type,
                ..
            } => {
                let key_bytes = lua_name.as_bytes().to_vec();
                // Build param types from the Rust parameter types.
                let mut param_exprs = Vec::<TokenStream>::new();
                let mut has_variadic = false;
                for p in params {
                    match p {
                        crate::util::ParamKind::Normal(ident, ty) => {
                            let name_str = ident.to_string();
                            let name_bytes = name_str.as_bytes().to_vec();
                            param_exprs.push(quote! {
                                (
                                    ::std::option::Option::Some(
                                        #k::bytes::Bytes::from_static(&[ #(#name_bytes),* ])
                                    ),
                                    <#ty as #k::LuaTyped>::lua_type(),
                                )
                            });
                        }
                        crate::util::ParamKind::CallContext(_) => {}
                        crate::util::ParamKind::Variadic(_) => {
                            has_variadic = true;
                        }
                    }
                }
                let variadic_expr = if has_variadic {
                    quote! { ::std::option::Option::Some(::std::boxed::Box::new(#k::types::LuaType::Any)) }
                } else {
                    quote! { ::std::option::Option::None }
                };
                type_field_stmts.push(quote! {
                    __fields.push((
                        #k::bytes::Bytes::from_static(&[ #(#key_bytes),* ]),
                        #k::types::LuaType::Function(::std::boxed::Box::new(
                            #k::types::FunctionLuaType {
                                type_params: ::std::vec::Vec::new(),
                                params: ::std::vec![ #(#param_exprs),* ],
                                variadic: #variadic_expr,
                                returns: <#return_type as #k::LuaTypedMulti>::lua_types(),
                                is_method: false,
                            }
                        )),
                    ));
                });
            }
            ModuleItem::EagerField {
                lua_name,
                return_type,
                ..
            } => {
                let key_bytes = lua_name.as_bytes().to_vec();
                type_field_stmts.push(quote! {
                    __fields.push((
                        #k::bytes::Bytes::from_static(&[ #(#key_bytes),* ]),
                        <#return_type as #k::LuaTyped>::lua_type(),
                    ));
                });
            }
        }
    }

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
                #k::bytes::Bytes::from_static(&[ #(#lua_mod_name_bytes),* ]),
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
            let mut __fields: ::std::vec::Vec<(#k::bytes::Bytes, #k::types::LuaType)> =
                ::std::vec::Vec::new();
            #(#type_field_stmts)*
            #k::types::ModuleTypeInfo {
                exported_types: ::std::collections::HashMap::new(),
                return_type: ::std::option::Option::Some(
                    #k::types::LuaType::Table(::std::boxed::Box::new(
                        #k::types::TableLuaType {
                            fields: __fields,
                            indexer: ::std::option::Option::None,
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
                #k::bytes::Bytes::from_static(&[ #(#lua_mod_name_bytes),* ]),
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
