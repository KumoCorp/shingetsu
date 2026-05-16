//! mlua-side codegen for enum derives.  Mirrors the shingetsu-side
//! [`crate::lua_enum`] for:
//!  - **unit-variant** string enums (serde default name /
//!    `#[lua(rename)]`), and
//!  - **untagged newtype** enums (`#[lua(untagged)]` / default):
//!    each variant's inner `mlua::FromLua` is tried in the same
//!    order shingetsu uses (`sort_and_validate`), first `Ok` wins.
//!
//! Internally-/adjacently-tagged data enums are still not mirrored
//! on the mlua side (`compile_error!`).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DataEnum, DeriveInput};

use crate::lua_enum::{
    collect_variants, parse_enum_opts, sort_and_validate, unit_string_variants, Tagging,
};

pub fn derive_enum_from_lua(parsed: &DeriveInput, data: &DataEnum) -> TokenStream {
    let name = &parsed.ident;

    // 1) all-unit → string enum
    if let Some(units) = unit_string_variants(data) {
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

    // 2) data-carrying enums: only **untagged** newtype is mirrored.
    let tagging = match parse_enum_opts(&parsed.attrs) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error(),
    };
    if !matches!(tagging, Tagging::Untagged) {
        return syn::Error::new_spanned(
            &parsed.ident,
            "the migration facade only mirrors unit-string and untagged \
             newtype enums on the mlua side; internally-/adjacently-tagged \
             data enums need `derive(shingetsu::LuaRepr)` plus a \
             hand-written mlua impl",
        )
        .to_compile_error();
    }
    let mut variants = match collect_variants(data) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };
    if variants.is_empty() {
        return syn::Error::new_spanned(
            name,
            "FromLua derive requires at least one variant",
        )
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

    // 1) all-unit → string enum (unchanged).
    if let Some(units) = unit_string_variants(data) {
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

    // 2) data-carrying enums: only **untagged** newtype is mirrored
    // (symmetric to `derive_enum_from_lua`).  Each variant delegates
    // to its inner type's `mlua::IntoLua`.
    let tagging = match parse_enum_opts(&parsed.attrs) {
        Ok(t) => t,
        Err(e) => return e.to_compile_error(),
    };
    if !matches!(tagging, Tagging::Untagged) {
        return syn::Error::new_spanned(
            &parsed.ident,
            "the migration facade only mirrors unit-string and untagged \
             newtype enums on the mlua side; internally-/adjacently-tagged \
             data enums need `derive(shingetsu::LuaRepr)` plus a \
             hand-written mlua impl",
        )
        .to_compile_error();
    }
    let variants = match collect_variants(data) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error(),
    };
    if variants.is_empty() {
        return syn::Error::new_spanned(
            name,
            "IntoLua derive requires at least one variant",
        )
        .to_compile_error();
    }

    let arms = variants.iter().map(|v| {
        let vid = v.ident;
        quote! {
            #name::#vid(__inner) => ::mlua::IntoLua::into_lua(__inner, __lua),
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
                }
            }
        }
    }
}
