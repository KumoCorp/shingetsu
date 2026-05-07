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

/// Both-engines `#[module]` attribute.  Generates the shingetsu-
/// side wiring plus the mlua-side wiring inside the same `mod` body.
///
/// Mlua-side coverage: sync `#[function]`, async `#[function]` (via
/// `create_async_function`), eager `#[field]`, `#[lazy_field]`,
/// `#[getter]`, and `#[setter]`.  Accessors emit a metatable on the
/// returned module table with `__index` / `__newindex` dispatching
/// to the user's per-key Rust functions.
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    facade::module(attr.into(), item.into()).into()
}

/// Both-engines `#[userdata]` attribute on impl blocks.  Emits the
/// shingetsu-side `Userdata` impl plus an `impl ::mlua::UserData for T`
/// covering sync `#[lua_method]` (`&self` and `&mut self`) and
/// `#[lua_field]` items.  Async methods, `#[lua_metamethod]`,
/// `#[lua_snapshot]`, `Arc<Self>` receivers, and engine-coupled
/// parameter kinds are rejected on the mlua side; keep those types
/// on `#[shingetsu::userdata]` until the corresponding facade
/// support lands.
#[proc_macro_attribute]
pub fn userdata(attr: TokenStream, item: TokenStream) -> TokenStream {
    facade::userdata(attr.into(), item.into()).into()
}
