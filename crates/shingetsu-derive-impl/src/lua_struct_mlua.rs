//! mlua-side codegen for the migration-facade `derive(LuaRepr)`.
//!
//! Mirrors the shingetsu-side codegen in [`crate::lua_struct`] using
//! the same `#[lua(...)]` attribute parsing, but emits
//! [`mlua::FromLua`] and [`mlua::IntoLua`] impls that walk fields via
//! `mlua::Table::get` / `mlua::Table::set`.  This guarantees both
//! engines produce identical observable behavior at every supported
//! attribute, removing the cognitive tax of maintaining parallel
//! `#[lua(...)]` and `#[serde(...)]` annotations during migration.
//!
//! `derive_lua_table` is the entry point.  It always emits both
//! `FromLua` and `IntoLua`; there is no separate sub-derive on the
//! mlua side because mlua has no equivalent of shingetsu's
//! `LuaTyped` (mlua's type info is per-impl, not per-derive).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, LitStr};

use crate::lua_struct::{collect_fields, is_option, parse_container_opts, FieldInfo};

/// Derive both `mlua::FromLua` and `mlua::IntoLua` for the input
/// struct or enum.  Honors the same `#[lua(...)]` attribute set as
/// the shingetsu-side derive.
pub fn derive_lua_table(input: TokenStream) -> TokenStream {
    let from = derive_from_lua(input.clone());
    let into = derive_into_lua(input);
    quote! {
        #from
        #into
    }
}

/// Derive `mlua::FromLua`.
pub fn derive_from_lua(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };
    let name = &parsed.ident;
    let data = match &parsed.data {
        Data::Struct(s) => s,
        Data::Enum(e) => return crate::lua_enum_mlua::derive_enum_from_lua(&parsed, e),
        _ => {
            return syn::Error::new_spanned(name, "FromLua derive only supports structs and enums")
                .to_compile_error()
        }
    };

    let container = match parse_container_opts(&parsed.attrs) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    if let Some(intermediate) = &container.try_from {
        return quote! {
            impl ::mlua::FromLua for #name {
                fn from_lua(__v: ::mlua::Value, __lua: &::mlua::Lua) -> ::mlua::Result<Self> {
                    let __interm: #intermediate =
                        <#intermediate as ::mlua::FromLua>::from_lua(__v, __lua)?;
                    <#name as ::core::convert::TryFrom<#intermediate>>::try_from(__interm)
                        .map_err(|__e| ::mlua::Error::FromLuaConversionError {
                            from: "table",
                            to: ::std::stringify!(#name).into(),
                            message: ::std::option::Option::Some(::std::format!("{}", __e)),
                        })
                }
            }
        };
    }

    let fields = match collect_fields(&data.fields) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let extractions: Vec<TokenStream> = fields.iter().map(gen_from_lua_field).collect();
    let field_names: Vec<&syn::Ident> = fields.iter().map(|f| f.ident).collect();

    let known_keys: Vec<TokenStream> = fields
        .iter()
        .filter(|f| !f.opts.skip && !f.opts.flatten)
        .map(|f| {
            let key = LitStr::new(&f.lua_key, proc_macro2::Span::call_site());
            quote! { #key }
        })
        .collect();
    let has_flatten = fields.iter().any(|f| f.opts.flatten);
    let deny_unknown_check = if container.deny_unknown_fields && !has_flatten {
        quote! {
            {
                let __known: &[&str] = &[ #(#known_keys),* ];
                for __pair in __table.clone().pairs::<::mlua::Value, ::mlua::Value>() {
                    let (__k, _) = __pair?;
                    if let ::mlua::Value::String(ref __sb) = __k {
                        let __s = __sb.to_str()?;
                        let __s_ref: &str = &__s;
                        if !__known.iter().any(|__n| *__n == __s_ref) {
                            return ::std::result::Result::Err(
                                ::mlua::Error::FromLuaConversionError {
                                    from: "table",
                                    to: ::std::stringify!(#name).into(),
                                    message: ::std::option::Option::Some(::std::format!(
                                        "unknown field `{}`", __s_ref
                                    )),
                                });
                        }
                    }
                }
            }
        }
    } else {
        quote! {}
    };

    let nil_handler = match &container.default {
        Some(None) => quote! {
            ::mlua::Value::Nil => {
                return ::std::result::Result::Ok(
                    <Self as ::core::default::Default>::default());
            }
        },
        Some(Some(path)) => quote! {
            ::mlua::Value::Nil => {
                return ::std::result::Result::Ok(#path());
            }
        },
        None => quote! {},
    };

    quote! {
        impl ::mlua::FromLua for #name {
            fn from_lua(__v: ::mlua::Value, __lua: &::mlua::Lua) -> ::mlua::Result<Self> {
                let __table: ::mlua::Table = match __v {
                    ::mlua::Value::Table(t) => t,
                    #nil_handler
                    other => {
                        return ::std::result::Result::Err(
                            ::mlua::Error::FromLuaConversionError {
                                from: other.type_name(),
                                to: ::std::stringify!(#name).into(),
                                message: ::std::option::Option::Some("expected table".to_owned()),
                            });
                    }
                };
                let __lua_ref: &::mlua::Lua = __lua;
                #deny_unknown_check
                #(#extractions)*
                ::std::result::Result::Ok(#name { #(#field_names),* })
            }
        }
    }
}

fn gen_from_lua_field(f: &FieldInfo<'_>) -> TokenStream {
    let ident = f.ident;
    let ty = f.ty;

    if f.opts.skip {
        return quote! {
            let #ident: #ty = ::core::default::Default::default();
        };
    }

    if f.opts.flatten {
        return quote! {
            let #ident: #ty = <#ty as ::mlua::FromLua>::from_lua(
                ::mlua::Value::Table(__table.clone()),
                __lua_ref,
            )?;
        };
    }

    let key = LitStr::new(&f.lua_key, proc_macro2::Span::call_site());
    let validate = gen_validate(f);

    if let Some(intermediate) = &f.opts.try_from {
        let extract = if let Some(default_expr) = &f.opts.default {
            quote! {
                let __interm: ::std::option::Option<#intermediate> = __table.get(#key)?;
                let __interm = match __interm {
                    ::std::option::Option::Some(v) => v,
                    ::std::option::Option::None => {
                        let #ident: #ty = #default_expr;
                        #validate
                        return ::std::result::Result::Ok::<_, ::mlua::Error>(#ident);
                    }
                };
            }
        } else {
            quote! {
                let __interm: #intermediate = __table.get(#key)?;
            }
        };
        return quote! {
            let #ident: #ty = (|| -> ::mlua::Result<#ty> {
                #extract
                let #ident: #ty = <#ty as ::core::convert::TryFrom<#intermediate>>::try_from(__interm)
                    .map_err(|__e| ::mlua::Error::FromLuaConversionError {
                        from: "table",
                        to: ::std::stringify!(#ty).into(),
                        message: ::std::option::Option::Some(::std::format!("{}", __e)),
                    })?;
                #validate
                ::std::result::Result::Ok(#ident)
            })()?;
        };
    }

    if let Some(default_expr) = &f.opts.default {
        return quote! {
            let #ident: #ty = match __table.get::<::std::option::Option<#ty>>(#key)? {
                ::std::option::Option::Some(v) => v,
                ::std::option::Option::None => #default_expr,
            };
            #validate
        };
    }

    quote! {
        let #ident: #ty = __table.get(#key)?;
        #validate
    }
}

fn gen_validate(f: &FieldInfo<'_>) -> TokenStream {
    let Some(path) = &f.opts.validate else {
        return quote! {};
    };
    let ident = f.ident;
    let key = &f.lua_key;
    quote! {
        if let ::std::result::Result::Err(__msg) = #path(&#ident) {
            return ::std::result::Result::Err(::mlua::Error::FromLuaConversionError {
                from: "table",
                to: "validated".into(),
                message: ::std::option::Option::Some(::std::format!(
                    "field `{}`: {}", #key, __msg
                )),
            });
        }
    }
}

/// Derive `mlua::IntoLua`.
pub fn derive_into_lua(input: TokenStream) -> TokenStream {
    let parsed: DeriveInput = match syn::parse2(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };
    let name = &parsed.ident;
    let data = match &parsed.data {
        Data::Struct(s) => s,
        Data::Enum(e) => return crate::lua_enum_mlua::derive_enum_into_lua(&parsed, e),
        _ => {
            return syn::Error::new_spanned(name, "IntoLua derive only supports structs and enums")
                .to_compile_error()
        }
    };

    let container = match parse_container_opts(&parsed.attrs) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    if let Some(target) = container.into.as_ref().or(container.try_from.as_ref()) {
        return quote! {
            impl ::mlua::IntoLua for #name {
                fn into_lua(self, __lua: &::mlua::Lua) -> ::mlua::Result<::mlua::Value> {
                    <#target as ::mlua::IntoLua>::into_lua(
                        ::core::convert::Into::<#target>::into(self),
                        __lua,
                    )
                }
            }
        };
    }

    let fields = match collect_fields(&data.fields) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };

    let insertions: Vec<TokenStream> = fields.iter().map(gen_into_lua_field).collect();

    quote! {
        impl ::mlua::IntoLua for #name {
            fn into_lua(self, __lua: &::mlua::Lua) -> ::mlua::Result<::mlua::Value> {
                let __table = __lua.create_table()?;
                #(#insertions)*
                ::std::result::Result::Ok(::mlua::Value::Table(__table))
            }
        }
    }
}

fn gen_into_lua_field(f: &FieldInfo<'_>) -> TokenStream {
    let ident = f.ident;

    if f.opts.skip {
        return quote! {};
    }

    if f.opts.flatten {
        let ty = f.ty;
        return quote! {
            {
                let __inner_value = <#ty as ::mlua::IntoLua>::into_lua(self.#ident, __lua)?;
                if let ::mlua::Value::Table(__inner) = __inner_value {
                    for __pair in __inner.pairs::<::mlua::Value, ::mlua::Value>() {
                        let (__k, __v) = __pair?;
                        __table.set(__k, __v)?;
                    }
                }
            }
        };
    }

    let key = LitStr::new(&f.lua_key, proc_macro2::Span::call_site());

    let value_expr = match (&f.opts.into, &f.opts.try_from) {
        (Some(target), _) => quote! {
            <#target as ::mlua::IntoLua>::into_lua(
                ::core::convert::Into::<#target>::into(self.#ident),
                __lua,
            )?
        },
        (None, Some(intermediate)) => quote! {
            <#intermediate as ::mlua::IntoLua>::into_lua(
                ::core::convert::Into::<#intermediate>::into(self.#ident),
                __lua,
            )?
        },
        (None, None) => quote! {
            ::mlua::IntoLua::into_lua(self.#ident, __lua)?
        },
    };

    if is_option(f.ty) && f.opts.into.is_none() && f.opts.try_from.is_none() {
        // Skip None values to match shingetsu's behavior.
        return quote! {
            if let ::std::option::Option::Some(__v) = self.#ident {
                __table.set(#key, ::mlua::IntoLua::into_lua(__v, __lua)?)?;
            }
        };
    }

    quote! {
        __table.set(#key, #value_expr)?;
    }
}
