//! mlua-side codegen for enum derives.  Mirrors the shingetsu-side
//! [`crate::lua_enum`] for:
//!  - **unit-variant** string enums (serde default name /
//!    `#[lua(rename)]`),
//!  - **untagged newtype** enums (`#[lua(untagged)]` / default):
//!    each variant's inner `mlua::FromLua` is tried in the same
//!    order shingetsu uses (`sort_and_validate`), first `Ok` wins, and
//!  - **externally-tagged** mixed enums (inferred when a container
//!    mixes unit and newtype variants): unit variants map to/from
//!    a Lua string, newtype variants to/from `{ tag = inner }`.
//!
//! Internally-/adjacently-tagged data enums are still not mirrored
//! on the mlua side (`compile_error!`).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DataEnum, DeriveInput};

use crate::lua_enum::{
    collect_external_variants, collect_variants, nil_variant_idents, parse_enum_opts,
    resolve_tagging, sort_and_validate, unit_string_variants, ExternalVariant, Tagging,
};

pub fn derive_enum_from_lua(parsed: &DeriveInput, data: &DataEnum) -> TokenStream {
    let name = &parsed.ident;

    let opts = match parse_enum_opts(&parsed.attrs) {
        Ok(o) => o,
        Err(e) => return e.to_compile_error(),
    };
    // 1) all-unit → string enum
    if let Some(units) = unit_string_variants(data, opts.rename_all) {
        let units = match units {
            Ok(u) => u,
            Err(e) => return e.to_compile_error(),
        };
        let arms = units.iter().map(|(id, n)| {
            let nb = n.as_bytes().to_vec();
            quote! { &[ #(#nb),* ] => ::std::result::Result::Ok(#name::#id), }
        });
        let expected = units
            .iter()
            .map(|(_, n)| n.as_str())
            .collect::<Vec<_>>()
            .join("`, `");
        return quote! {
            impl ::mlua::FromLua for #name {
                fn from_lua(
                    __value: ::mlua::Value,
                    __lua: &::mlua::Lua,
                ) -> ::mlua::Result<Self> {
                    let __s = <::mlua::String as ::mlua::FromLua>::from_lua(__value, __lua)?;
                    match __s.as_bytes().as_ref() {
                        #(#arms)*
                        __other => ::std::result::Result::Err(::mlua::Error::FromLuaConversionError {
                            from: "string",
                            to: ::std::stringify!(#name).into(),
                            message: ::std::option::Option::Some(::std::format!(
                                "unknown {} variant `{}`; expected one of `{}`",
                                ::std::stringify!(#name),
                                ::std::string::String::from_utf8_lossy(__other),
                                #expected
                            )),
                        }),
                    }
                }
            }
        };
    }

    // 2) data-carrying enums: externally-tagged (inferred) and
    // explicitly untagged newtype enums are mirrored; internally-/
    // adjacently-tagged variants still require a hand-written mlua impl.
    let tagging = resolve_tagging(&opts, data);
    if matches!(tagging, Tagging::External) {
        let externals = match collect_external_variants(data, opts.rename_all) {
            Ok(v) => v,
            Err(e) => return e.to_compile_error(),
        };
        return from_lua_external_mlua(name, &externals);
    }
    if !matches!(tagging, Tagging::Untagged) {
        return syn::Error::new_spanned(
            &parsed.ident,
            "the migration facade only mirrors unit-string, untagged \
             newtype, and externally-tagged (mixed unit + newtype) enums \
             on the mlua side; internally-/adjacently-tagged data enums \
             need `derive(shingetsu::LuaRepr)` plus a hand-written mlua impl",
        )
        .to_compile_error();
    }
    let mut variants = match collect_variants(data, opts.rename_all) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };
    if variants.is_empty() {
        return syn::Error::new_spanned(name, "FromLua derive requires at least one variant")
            .to_compile_error();
    }
    // Mirror shingetsu's variant order exactly (size-ascending,
    // declaration-stable) so both engines try variants identically.
    if let Err(e) = sort_and_validate(&mut variants) {
        return e.to_compile_error();
    }

    let val_ident = quote! { __value };
    let try_arms = variants.iter().map(|v| {
        let vid = v.ident;
        let ty = v.ty;
        // Strict discriminant guard: skip variants whose Lua kind
        // doesn't match, so mlua's coercive scalar `FromLua` can't
        // hijack (e.g. a number being coerced into a `String`
        // variant).  Mirrors shingetsu's non-coercive behavior and
        // the pre-derive explicit `match value { Value::X(..) }`.
        let guard = v.mlua_kind_guard(&val_ident);
        quote! {
            if #guard {
                if let ::std::result::Result::Ok(__inner) =
                    <#ty as ::mlua::FromLua>::from_lua(__value.clone(), __lua)
                {
                    return ::std::result::Result::Ok(#name::#vid(__inner));
                }
            }
        }
    });
    let expected = variants
        .iter()
        .map(|v| {
            let ty = v.ty;
            quote! { ::std::stringify!(#ty) }
        })
        .collect::<Vec<_>>();

    quote! {
        impl ::mlua::FromLua for #name {
            fn from_lua(
                __value: ::mlua::Value,
                __lua: &::mlua::Lua,
            ) -> ::mlua::Result<Self> {
                let __type_name = __value.type_name();
                #(#try_arms)*
                ::std::result::Result::Err(::mlua::Error::FromLuaConversionError {
                    from: __type_name,
                    to: ::std::stringify!(#name).into(),
                    message: ::std::option::Option::Some(::std::format!(
                        "expected one of [{}] for {}",
                        [ #(#expected),* ].join(" | "),
                        ::std::stringify!(#name)
                    )),
                })
            }
        }
    }
}

pub fn derive_enum_into_lua(parsed: &DeriveInput, data: &DataEnum) -> TokenStream {
    let name = &parsed.ident;

    let opts = match parse_enum_opts(&parsed.attrs) {
        Ok(o) => o,
        Err(e) => return e.to_compile_error(),
    };
    // 1) all-unit → string enum (unchanged).
    if let Some(units) = unit_string_variants(data, opts.rename_all) {
        let units = match units {
            Ok(u) => u,
            Err(e) => return e.to_compile_error(),
        };

        let arms = units.iter().map(|(id, n)| {
            quote! { #name::#id => #n, }
        });

        return quote! {
            impl ::mlua::IntoLua for #name {
                fn into_lua(
                    self,
                    __lua: &::mlua::Lua,
                ) -> ::mlua::Result<::mlua::Value> {
                    let __s: &'static str = match self {
                        #(#arms)*
                    };
                    ::std::result::Result::Ok(::mlua::Value::String(
                        __lua.create_string(__s)?,
                    ))
                }
            }
        };
    }

    // 2) data-carrying enums: externally-tagged (inferred) and
    // explicitly untagged newtype enums are mirrored (symmetric to
    // `derive_enum_from_lua`).
    let tagging = resolve_tagging(&opts, data);
    if matches!(tagging, Tagging::External) {
        let externals = match collect_external_variants(data, opts.rename_all) {
            Ok(v) => v,
            Err(e) => return e.to_compile_error(),
        };
        return into_lua_external_mlua(name, &externals);
    }
    if !matches!(tagging, Tagging::Untagged) {
        return syn::Error::new_spanned(
            &parsed.ident,
            "the migration facade only mirrors unit-string, untagged \
             newtype, and externally-tagged (mixed unit + newtype) enums \
             on the mlua side; internally-/adjacently-tagged data enums \
             need `derive(shingetsu::LuaRepr)` plus a hand-written mlua impl",
        )
        .to_compile_error();
    }
    let variants = match collect_variants(data, opts.rename_all) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };
    if variants.is_empty() {
        return syn::Error::new_spanned(name, "IntoLua derive requires at least one variant")
            .to_compile_error();
    }

    let arms = variants.iter().map(|v| {
        let vid = v.ident;
        quote! {
            #name::#vid(__inner) => ::mlua::IntoLua::into_lua(__inner, __lua),
        }
    });
    let nil_arms = nil_variant_idents(data).into_iter().map(|id| {
        quote! {
            #name::#id => ::std::result::Result::Ok(::mlua::Value::Nil),
        }
    });

    quote! {
        impl ::mlua::IntoLua for #name {
            fn into_lua(
                self,
                __lua: &::mlua::Lua,
            ) -> ::mlua::Result<::mlua::Value> {
                match self {
                    #(#arms)*
                    #(#nil_arms)*
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// External tagging codegen (mlua side)
// ---------------------------------------------------------------------------

fn external_expected_str(variants: &[ExternalVariant<'_>]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for v in variants {
        match v {
            ExternalVariant::Unit { lua_name, .. } => parts.push(format!("\"{lua_name}\"")),
            ExternalVariant::Newtype { lua_name, .. } => {
                parts.push(format!("{{ {lua_name} = ... }}"))
            }
        }
    }
    parts.join(" | ")
}

fn from_lua_external_mlua(name: &syn::Ident, variants: &[ExternalVariant<'_>]) -> TokenStream {
    let string_arms = variants.iter().filter_map(|v| match v {
        ExternalVariant::Unit { ident, lua_name } => {
            let nb = lua_name.as_bytes().to_vec();
            Some(quote! {
                &[ #(#nb),* ] => return ::std::result::Result::Ok(#name::#ident),
            })
        }
        _ => None,
    });
    let table_arms = variants.iter().filter_map(|v| match v {
        ExternalVariant::Newtype {
            ident,
            lua_name,
            ty,
        } => Some(quote! {
            {
                let __inner_val: ::mlua::Value = __table.raw_get(#lua_name)?;
                if !matches!(__inner_val, ::mlua::Value::Nil) {
                    let __inner = <#ty as ::mlua::FromLua>::from_lua(__inner_val, __lua)?;
                    return ::std::result::Result::Ok(#name::#ident(__inner));
                }
            }
        }),
        _ => None,
    });
    let expected = external_expected_str(variants);
    quote! {
        impl ::mlua::FromLua for #name {
            fn from_lua(
                __value: ::mlua::Value,
                __lua: &::mlua::Lua,
            ) -> ::mlua::Result<Self> {
                match &__value {
                    ::mlua::Value::String(__s) => {
                        let __bytes = __s.as_bytes();
                        match __bytes.as_ref() {
                            #(#string_arms)*
                            __other => {
                                return ::std::result::Result::Err(
                                    ::mlua::Error::FromLuaConversionError {
                                        from: "string",
                                        to: ::std::stringify!(#name).into(),
                                        message: ::std::option::Option::Some(::std::format!(
                                            "unknown {} variant `{}`; expected one of: {}",
                                            ::std::stringify!(#name),
                                            ::std::string::String::from_utf8_lossy(__other),
                                            #expected,
                                        )),
                                    },
                                );
                            }
                        }
                    }
                    ::mlua::Value::Table(__table) => {
                        #(#table_arms)*
                        return ::std::result::Result::Err(
                            ::mlua::Error::FromLuaConversionError {
                                from: "table",
                                to: ::std::stringify!(#name).into(),
                                message: ::std::option::Option::Some(::std::format!(
                                    "table did not contain any known variant tag for {}; expected one of: {}",
                                    ::std::stringify!(#name),
                                    #expected,
                                )),
                            },
                        );
                    }
                    __other => {
                        return ::std::result::Result::Err(
                            ::mlua::Error::FromLuaConversionError {
                                from: __other.type_name(),
                                to: ::std::stringify!(#name).into(),
                                message: ::std::option::Option::Some(::std::format!(
                                    "expected one of: {}",
                                    #expected,
                                )),
                            },
                        );
                    }
                }
            }
        }
    }
}

fn into_lua_external_mlua(name: &syn::Ident, variants: &[ExternalVariant<'_>]) -> TokenStream {
    let arms = variants.iter().map(|v| match v {
        ExternalVariant::Unit { ident, lua_name } => quote! {
            #name::#ident => {
                ::std::result::Result::Ok(::mlua::Value::String(
                    __lua.create_string(#lua_name)?,
                ))
            }
        },
        ExternalVariant::Newtype {
            ident, lua_name, ..
        } => quote! {
            #name::#ident(__inner) => {
                let __t = __lua.create_table()?;
                __t.raw_set(#lua_name, ::mlua::IntoLua::into_lua(__inner, __lua)?)?;
                ::std::result::Result::Ok(::mlua::Value::Table(__t))
            }
        },
    });
    quote! {
        impl ::mlua::IntoLua for #name {
            fn into_lua(
                self,
                __lua: &::mlua::Lua,
            ) -> ::mlua::Result<::mlua::Value> {
                match self {
                    #(#arms)*
                }
            }
        }
    }
}
