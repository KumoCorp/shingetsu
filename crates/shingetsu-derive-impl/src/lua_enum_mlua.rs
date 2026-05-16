//! mlua-side codegen for enum derives.  Mirrors the shingetsu-side
//! [`crate::lua_enum`] string representation for **unit-variant**
//! enums (serde default: the variant name, or
//! `#[lua(rename = "...")]`).  Data-carrying (newtype/tagged) enums
//! are not yet mirrored on the mlua side; those still emit a
//! `compile_error!` pointing the host at the shingetsu derive plus a
//! hand-written mlua impl.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DataEnum, DeriveInput};

use crate::lua_enum::unit_string_variants;

pub fn derive_enum_from_lua(parsed: &DeriveInput, data: &DataEnum) -> TokenStream {
    let name = &parsed.ident;
    let Some(units) = unit_string_variants(data) else {
        return syn::Error::new_spanned(
            &parsed.ident,
            "the migration facade's enum derive only mirrors unit-variant \
             (string) enums on the mlua side so far; keep data-carrying \
             enums on `derive(shingetsu::LuaRepr)` plus a hand-written \
             mlua impl",
        )
        .to_compile_error();
    };
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

    quote! {
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
    }
}

pub fn derive_enum_into_lua(parsed: &DeriveInput, data: &DataEnum) -> TokenStream {
    let name = &parsed.ident;
    let Some(units) = unit_string_variants(data) else {
        return syn::Error::new_spanned(
            &parsed.ident,
            "the migration facade's enum derive only mirrors unit-variant \
             (string) enums on the mlua side so far",
        )
        .to_compile_error();
    };
    let units = match units {
        Ok(u) => u,
        Err(e) => return e.to_compile_error(),
    };

    let arms = units.iter().map(|(id, n)| {
        quote! { #name::#id => #n, }
    });

    quote! {
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
    }
}
