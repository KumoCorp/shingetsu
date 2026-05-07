//! Composite codegen entry points used by the migration facade.
//! Each function emits both the shingetsu-side and mlua-side impls
//! from a single `#[derive(...)]` invocation, so the user's source
//! has only one derive macro per type and the two engines stay in
//! lockstep on every supported `#[lua(...)]` attribute.

use proc_macro2::TokenStream;
use quote::quote;

/// Both-engines `derive(LuaTable)`.  Emits the shingetsu
/// `FromLua` / `IntoLua` / `LuaTyped` trio plus the mlua
/// `FromLua` / `IntoLua` pair.
pub fn derive_lua_table(input: TokenStream) -> TokenStream {
    let shingetsu = crate::lua_struct::derive_lua_table(input.clone());
    let mlua = crate::lua_struct_mlua::derive_lua_table(input);
    quote! {
        #shingetsu
        #mlua
    }
}

/// Both-engines `derive(FromLua)`.
pub fn derive_from_lua(input: TokenStream) -> TokenStream {
    let shingetsu = crate::lua_struct::derive_from_lua(input.clone());
    let mlua = crate::lua_struct_mlua::derive_from_lua(input);
    quote! {
        #shingetsu
        #mlua
    }
}

/// Both-engines `derive(IntoLua)`.
pub fn derive_into_lua(input: TokenStream) -> TokenStream {
    let shingetsu = crate::lua_struct::derive_into_lua(input.clone());
    let mlua = crate::lua_struct_mlua::derive_into_lua(input);
    quote! {
        #shingetsu
        #mlua
    }
}

/// Shingetsu-side `derive(LuaTyped)` only.  mlua has no equivalent
/// per-type type-info trait, so this passes through to the
/// shingetsu codegen unchanged.
pub fn derive_lua_typed(input: TokenStream) -> TokenStream {
    crate::lua_struct::derive_lua_typed(input)
}

/// Both-engines `#[module]` attribute.  Generates the shingetsu-
/// side wiring (`build_module_table` / `register_global_module` /
/// `register_preload` / `module_type`) plus the mlua-side wiring
/// (`build_mlua_module_table` / `register_mlua_module`) inside the
/// same `mod` body.
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    crate::module::expand_facade(attr, item)
}
