//! `derive(FromLua)`, `derive(IntoLua)`, `derive(LuaTyped)`, and
//! `derive(LuaTable)` for struct ↔ Lua table conversion.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr, Type};

use crate::util::type_is;

// ---------------------------------------------------------------------------
// #[lua(...)] field attribute parsing
// ---------------------------------------------------------------------------

struct FieldOpts {
    /// Override the Lua key name.
    rename: Option<String>,
    /// Default expression when the field is nil/absent.
    default: Option<syn::Expr>,
}

fn parse_field_opts(attrs: &[syn::Attribute]) -> syn::Result<FieldOpts> {
    let mut opts = FieldOpts {
        rename: None,
        default: None,
    };
    for attr in attrs {
        if !attr.path().is_ident("lua") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let val: LitStr = meta.value()?.parse()?;
                opts.rename = Some(val.value());
                Ok(())
            } else if meta.path.is_ident("default") {
                let val: syn::Expr = meta.value()?.parse()?;
                opts.default = Some(val);
                Ok(())
            } else {
                Err(meta.error("unknown lua field option; expected `rename` or `default`"))
            }
        })?;
    }
    Ok(opts)
}

/// Returns `true` if `ty` is `Option<T>`.
fn is_option(ty: &Type) -> bool {
    type_is(ty, "Option")
}

// ---------------------------------------------------------------------------
// Shared: collect field info
// ---------------------------------------------------------------------------

struct FieldInfo<'a> {
    ident: &'a syn::Ident,
    ty: &'a Type,
    lua_key: String,
    opts: FieldOpts,
}

fn collect_fields(fields: &Fields) -> syn::Result<Vec<FieldInfo<'_>>> {
    match fields {
        Fields::Named(named) => named
            .named
            .iter()
            .map(|f| {
                let ident = f.ident.as_ref().expect("named field");
                let opts = parse_field_opts(&f.attrs)?;
                let lua_key = opts.rename.clone().unwrap_or_else(|| ident.to_string());
                Ok(FieldInfo {
                    ident,
                    ty: &f.ty,
                    lua_key,
                    opts,
                })
            })
            .collect(),
        _ => Err(syn::Error::new_spanned(
            fields,
            "FromLua/IntoLua derive only supports structs with named fields",
        )),
    }
}

// ---------------------------------------------------------------------------
// Shared: generate LuaTyped field type specs
// ---------------------------------------------------------------------------

fn gen_type_fields(fields: &[FieldInfo<'_>]) -> Vec<TokenStream> {
    fields
        .iter()
        .map(|f| {
            let key = &f.lua_key;
            let key_bytes = key.as_bytes().to_vec();
            let ty = f.ty;
            // If the field has a default or is Option<T>, wrap in Optional.
            let lua_ty = if f.opts.default.is_some() || is_option(ty) {
                quote! {
                    ::shingetsu::LuaType::Optional(
                        ::std::boxed::Box::new(<#ty as ::shingetsu::LuaTyped>::lua_type())
                    )
                }
            } else {
                quote! { <#ty as ::shingetsu::LuaTyped>::lua_type() }
            };
            quote! {
                (
                    ::shingetsu::Bytes::from(&[ #(#key_bytes),* ][..]),
                    #lua_ty,
                )
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// derive(FromLua)
// ---------------------------------------------------------------------------

pub fn derive_from_lua(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let name = &parsed.ident;
    let data = match &parsed.data {
        Data::Struct(s) => s,
        Data::Enum(e) => return crate::lua_enum::derive_enum_from_lua(&parsed, e),
        _ => {
            return syn::Error::new_spanned(name, "FromLua derive only supports structs and enums")
                .to_compile_error()
        }
    };

    let fields = match collect_fields(&data.fields) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    // Generate field extraction statements.
    let extractions: Vec<TokenStream> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            let key = &f.lua_key;
            let ty = f.ty;

            if let Some(ref default_expr) = f.opts.default {
                // Field with a default: get as Option<T>, fallback to default.
                quote! {
                    let #ident: #ty = match __table.get_field::<::std::option::Option<#ty>>(#key)? {
                        ::std::option::Option::Some(v) => v,
                        ::std::option::Option::None => #default_expr,
                    };
                }
            } else {
                // Required field (or Option<T> which handles nil via FromLua).
                quote! {
                    let #ident: #ty = __table.get_field(#key)?;
                }
            }
        })
        .collect();

    let field_names: Vec<&syn::Ident> = fields.iter().map(|f| f.ident).collect();

    quote! {
        impl ::shingetsu::FromLua for #name {
            fn from_lua(v: ::shingetsu::Value) -> ::std::result::Result<Self, ::shingetsu::VmError> {
                let __table = match v {
                    ::shingetsu::Value::Table(t) => t,
                    other => {
                        return ::std::result::Result::Err(::shingetsu::VmError::BadArgument {
                            position: 0,
                            function: ::std::string::String::new(),
                            expected: "table".to_owned(),
                            got: other.type_name().to_owned(),
                        });
                    }
                };
                #(#extractions)*
                ::std::result::Result::Ok(#name { #(#field_names),* })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// derive(IntoLua)
// ---------------------------------------------------------------------------

pub fn derive_into_lua(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let name = &parsed.ident;
    let data = match &parsed.data {
        Data::Struct(s) => s,
        Data::Enum(e) => return crate::lua_enum::derive_enum_into_lua(&parsed, e),
        _ => {
            return syn::Error::new_spanned(name, "IntoLua derive only supports structs and enums")
                .to_compile_error()
        }
    };

    let fields = match collect_fields(&data.fields) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    // Generate field insertion statements.
    let insertions: Vec<TokenStream> = fields
        .iter()
        .map(|f| {
            let ident = f.ident;
            let key = &f.lua_key;
            let key_bytes = key.as_bytes().to_vec();

            if is_option(f.ty) {
                // Skip None values — don't insert nil keys.
                quote! {
                    if let ::std::option::Option::Some(v) = self.#ident {
                        __table.raw_set(
                            ::shingetsu::Value::String(
                                ::shingetsu::Bytes::from(&[ #(#key_bytes),* ][..])
                            ),
                            ::shingetsu::IntoLua::into_lua(v),
                        ).expect("table set");
                    }
                }
            } else {
                quote! {
                    __table.raw_set(
                        ::shingetsu::Value::String(
                            ::shingetsu::Bytes::from(&[ #(#key_bytes),* ][..])
                        ),
                        ::shingetsu::IntoLua::into_lua(self.#ident),
                    ).expect("table set");
                }
            }
        })
        .collect();

    quote! {
        impl ::shingetsu::IntoLua for #name {
            fn into_lua(self) -> ::shingetsu::Value {
                let __table = ::shingetsu::Table::new();
                #(#insertions)*
                ::shingetsu::Value::Table(__table)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// derive(LuaTyped)
// ---------------------------------------------------------------------------

pub fn derive_lua_typed(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let name = &parsed.ident;
    match &parsed.data {
        Data::Struct(s) => {
            let fields = match collect_fields(&s.fields) {
                Ok(v) => v,
                Err(e) => return e.to_compile_error(),
            };
            let type_fields = gen_type_fields(&fields);
            quote! {
                impl ::shingetsu::LuaTyped for #name {
                    fn lua_type() -> ::shingetsu::LuaType {
                        ::shingetsu::LuaType::Table(::std::boxed::Box::new(
                            ::shingetsu::TableLuaType {
                                fields: ::std::vec![ #(#type_fields),* ],
                                indexer: ::std::option::Option::None,
                            }
                        ))
                    }
                }
            }
        }
        Data::Enum(e) => crate::lua_enum::derive_enum_lua_typed(&parsed, e),
        _ => syn::Error::new_spanned(name, "LuaTyped derive only supports structs and enums")
            .to_compile_error(),
    }
}

// ---------------------------------------------------------------------------
// derive(LuaTable) — convenience for FromLua + IntoLua + LuaTyped
// ---------------------------------------------------------------------------

pub fn derive_lua_table(input: TokenStream) -> TokenStream {
    let from = derive_from_lua(input.clone());
    let into = derive_into_lua(input.clone());
    let typed = derive_lua_typed(input);
    quote! {
        #from
        #into
        #typed
    }
}
