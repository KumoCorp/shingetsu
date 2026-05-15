//! Migration facade for hosts moving from `mlua` to `shingetsu`.
//!
//! Re-exports a shingetsu-shaped macro and type surface that, when
//! both backends are enabled, also emits the equivalent mlua-side
//! wiring so a host can run either engine at runtime during the
//! migration period.  Once migration completes, deleting this crate
//! is a search-and-replace of `shingetsu_migrate::` for
//! `shingetsu::`.
//!
//! Most modules are currently empty; their contents fill in as the
//! corresponding facade work lands.

#[cfg(feature = "shingetsu-backend")]
#[doc(inline)]
pub use shingetsu;

#[cfg(feature = "mlua-backend")]
#[doc(inline)]
pub use mlua;

// Conversion-derive facade re-exports.  Each derive emits BOTH the
// shingetsu-side and mlua-side impls from a single derive, so the
// host's source has one derive macro per type and the two engines
// stay in lockstep on every supported `#[lua(...)]` attribute.
#[cfg(feature = "mlua-backend")]
#[doc(inline)]
pub use shingetsu_migrate_derive::{module, userdata, FromLua, IntoLua, LuaTable, LuaTyped};

// `#[module]` and `#[userdata]` facade.
pub mod modules {}

mod variadic;
pub use variadic::Variadic;

mod userdata_ref;
pub use userdata_ref::UserDataRef;

mod serde_lua;
pub use serde_lua::SerdeLua;

#[cfg(feature = "mlua-backend")]
mod memoized;
#[cfg(feature = "mlua-backend")]
pub use memoized::Memoized;

// wezterm-dynamic interop bridge.
#[cfg(feature = "dynamic")]
mod dynamic;
#[cfg(feature = "dynamic")]
pub use dynamic::DynamicLua;

// Event registry facade (declare_event!, EventSignature, install_on).
mod event;
#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
pub use event::{emit_event, install_on};
pub use event::{
    EngineRef, EventDispatchTarget, EventDisposition, EventError, EventParam, EventSignature,
    MLUA_KEY_PREFIX,
};

// Memoization shims (`Memoized` / `impl_memoize`) that polyfill the
// lua-side `__memoize` metamethod on the mlua backend.
// Shingetsu-native code reaches the same primitive via
// `Userdata::snapshot()` directly.
pub mod memoize {}

// Runtime engine selector — picks between mlua and shingetsu at
// construction time and exposes a unified call surface.
mod engine;
pub use engine::Engine;

// Smoke-test surface so each feature combination has at least one
// reachable item.
#[doc(hidden)]
pub fn _smoke_test() -> &'static str {
    let mut backends = Vec::new();
    if cfg!(feature = "shingetsu-backend") {
        backends.push("shingetsu");
    }
    if cfg!(feature = "mlua-backend") {
        backends.push("mlua");
    }
    if backends.is_empty() {
        "no backends"
    } else if backends.len() == 1 {
        backends[0]
    } else {
        "shingetsu+mlua"
    }
}
