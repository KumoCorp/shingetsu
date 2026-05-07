//! mlua-side codegen for enum derives.  Mirrors the shingetsu-side
//! [`crate::lua_enum`] using the same tagging modes.  Hosts whose
//! enums need migration coverage today should use
//! `derive(shingetsu::LuaTable)` plus a hand-written `mlua::FromLua`
//! / `IntoLua` until this fills in.

use proc_macro2::TokenStream;
use syn::{DataEnum, DeriveInput};

pub fn derive_enum_from_lua(parsed: &DeriveInput, _data: &DataEnum) -> TokenStream {
    syn::Error::new_spanned(
        &parsed.ident,
        "the migration facade's `LuaTable` derive does not yet support enums; \
         either keep the enum on `derive(shingetsu::LuaTable)` plus a hand-\
         written mlua impl, or restructure as a struct",
    )
    .to_compile_error()
}

pub fn derive_enum_into_lua(parsed: &DeriveInput, _data: &DataEnum) -> TokenStream {
    syn::Error::new_spanned(
        &parsed.ident,
        "the migration facade's `LuaTable` derive does not yet support enums",
    )
    .to_compile_error()
}
