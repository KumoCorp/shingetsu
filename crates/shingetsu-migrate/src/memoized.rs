//! Cross-engine snapshot wrapper for memoizable userdata.
//!
//! [`Memoized`] is the mlua-side counterpart of
//! [`shingetsu::Snapshot`]: a closure that, given a live `mlua::Lua`
//! at rebuild time, produces an `mlua::Value` reconstructing the
//! captured userdata.  `#[shingetsu_migrate::userdata(snapshot)]`
//! auto-registers a `__memoize` metamethod returning one of these
//! whenever both engines require a clone-based snapshot.
//!
//! This type only exists to keep kumomta's existing `mod-memoize`
//! cache walker working unchanged during the transition.  Once
//! kumomta has fully migrated to shingetsu-native scripting,
//! `mod-memoize` reaches `shingetsu::Userdata::snapshot()` directly
//! from Rust and `Memoized` is deleted along with the
//! metatable-walking `FromLua` for `CacheValue`.
//!
//! Engines never cross over: the shingetsu engine reaches
//! `Userdata::snapshot()` to get a `shingetsu::Snapshot`; the mlua
//! engine reaches `__memoize` to get a `Memoized`.  The bridge
//! macro emits both at registration time.

#![cfg(feature = "mlua-backend")]

use std::sync::Arc;

/// Cross-engine snapshot wrapper.  See module docs.
#[derive(Clone)]
pub struct Memoized {
    pub to_value: Arc<dyn Fn(&mlua::Lua) -> mlua::Result<mlua::Value> + Send + Sync + 'static>,
}

impl PartialEq for Memoized {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.to_value, &other.to_value)
    }
}

impl std::fmt::Debug for Memoized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Memoized").finish_non_exhaustive()
    }
}

impl mlua::FromLua for Memoized {
    fn from_lua(value: mlua::Value, _lua: &mlua::Lua) -> mlua::Result<Self> {
        match value {
            mlua::Value::UserData(ud) => Ok(ud.borrow::<Self>()?.clone()),
            other => Err(mlua::Error::FromLuaConversionError {
                from: other.type_name(),
                to: "Memoized".to_owned(),
                message: None,
            }),
        }
    }
}

impl mlua::UserData for Memoized {}
