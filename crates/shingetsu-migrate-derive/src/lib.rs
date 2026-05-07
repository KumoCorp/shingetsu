//! Proc-macro entry points for the migration facade.  Each derive
//! emits BOTH the shingetsu-side and the mlua-side impls from a
//! single attribute on the user's struct, so the two engines stay in
//! lockstep on every supported `#[lua(...)]` attribute without the
//! host having to maintain parallel `#[serde(...)]` annotations.
//!
//! The actual codegen lives in `shingetsu-derive-impl`; this crate
//! is a thin wrapper.  Migration is a search-and-replace of
//! `shingetsu_migrate::` for `shingetsu::` (or, for `LuaTable` and
//! the conversion derives, removing the `_migrate` segment from the
//! `use` import).

use proc_macro::TokenStream;
use shingetsu_derive_impl::facade;

/// Both-engines `derive(LuaTable)` — emits shingetsu's `FromLua`,
/// `IntoLua`, and `LuaTyped` impls, plus mlua's `FromLua` and
/// `IntoLua` impls, from a single derive.  Honors the full
/// `#[lua(...)]` attribute set on both engines.
#[proc_macro_derive(LuaTable, attributes(lua))]
pub fn derive_lua_table(input: TokenStream) -> TokenStream {
    facade::derive_lua_table(input.into()).into()
}

/// Both-engines `derive(FromLua)`.
#[proc_macro_derive(FromLua, attributes(lua))]
pub fn derive_from_lua(input: TokenStream) -> TokenStream {
    facade::derive_from_lua(input.into()).into()
}

/// Both-engines `derive(IntoLua)`.
#[proc_macro_derive(IntoLua, attributes(lua))]
pub fn derive_into_lua(input: TokenStream) -> TokenStream {
    facade::derive_into_lua(input.into()).into()
}

/// `derive(LuaTyped)` — shingetsu-only (mlua has no per-type type
/// info trait), included here so users can write a single derive
/// list against the facade.
#[proc_macro_derive(LuaTyped, attributes(lua))]
pub fn derive_lua_typed(input: TokenStream) -> TokenStream {
    facade::derive_lua_typed(input.into()).into()
}
