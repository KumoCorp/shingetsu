//! [`LuaCallback`] — a portable, host-invocable Lua function value.
//!
//! Captures a Lua function from policy and exposes a host-side async
//! invocation API.  When both backends are compiled, `LuaCallback`
//! implements both [`mlua::FromLua`] and [`shingetsu::FromLua`] so the
//! same type works in `#[derive(FromLua)]` fields and `#[function]`
//! params on either backend.
//!
//! - Shingetsu backend: wraps [`shingetsu::Callable`]; `call` drives
//!   the captured function through a fresh [`shingetsu::Task`].
//! - Mlua backend: captures `(mlua::Lua, mlua::Function)`; `call`
//!   performs a synchronous `mlua::Function::call` (the future is
//!   trivially-ready so the public API is async on both engines).
//!
//! After the end-state `s/shingetsu_migrate::/shingetsu::` rewrite,
//! `LuaCallback` resolves to [`shingetsu::LuaCallback`] (an alias for
//! [`shingetsu::Callable`]), so policy-facing code migrates without
//! source changes.  At that point the mlua arm of this enum is
//! dropped and the wrapper collapses into the underlying primitive.

/// A Lua function captured from policy and callable from Rust.
///
/// Clone is cheap on both engines (Arc-backed handles).
#[derive(Clone)]
pub struct LuaCallback(Backend);

#[derive(Clone)]
enum Backend {
    #[cfg(feature = "shingetsu-backend")]
    Shingetsu(shingetsu::Callable),
    #[cfg(feature = "mlua-backend")]
    Mlua {
        lua: mlua::Lua,
        func: mlua::Function,
    },
}

impl LuaCallback {
    /// Construct from an already-built [`shingetsu::Callable`].
    #[cfg(feature = "shingetsu-backend")]
    pub fn from_shingetsu(c: shingetsu::Callable) -> Self {
        Self(Backend::Shingetsu(c))
    }

    /// Construct from an mlua `(Lua, Function)` pair.
    #[cfg(feature = "mlua-backend")]
    pub fn from_mlua(lua: mlua::Lua, func: mlua::Function) -> Self {
        Self(Backend::Mlua { lua, func })
    }

    /// Borrow the captured shingetsu callable, if the value was
    /// produced via the shingetsu backend.
    #[cfg(feature = "shingetsu-backend")]
    pub fn as_shingetsu(&self) -> Option<&shingetsu::Callable> {
        match &self.0 {
            Backend::Shingetsu(c) => Some(c),
            #[cfg(feature = "mlua-backend")]
            Backend::Mlua { .. } => None,
        }
    }

    /// Borrow the captured mlua `(Lua, Function)` pair, if the value
    /// was produced via the mlua backend.
    #[cfg(feature = "mlua-backend")]
    pub fn as_mlua(&self) -> Option<(&mlua::Lua, &mlua::Function)> {
        match &self.0 {
            Backend::Mlua { lua, func } => Some((lua, func)),
            #[cfg(feature = "shingetsu-backend")]
            Backend::Shingetsu(_) => None,
        }
    }
}

#[cfg(feature = "shingetsu-backend")]
mod shingetsu_impls {
    use super::{Backend, LuaCallback};
    use shingetsu::{FromLua, GlobalEnv, LuaType, LuaTyped, Value, VmError};

    impl FromLua for LuaCallback {
        fn from_lua(v: Value, env: &GlobalEnv) -> Result<Self, VmError> {
            let inner = <shingetsu::Callable as FromLua>::from_lua(v, env)?;
            Ok(LuaCallback(Backend::Shingetsu(inner)))
        }
    }

    impl LuaTyped for LuaCallback {
        fn lua_type() -> LuaType {
            <shingetsu::Callable as LuaTyped>::lua_type()
        }
    }

    impl LuaCallback {
        /// Invoke the captured function with `args` and decode the
        /// return list as `R`.
        ///
        /// Async on both engines so the public surface is uniform:
        /// the shingetsu path drives a fresh [`shingetsu::Task`], the
        /// mlua path forwards through `mlua::Function::call_async`.
        /// Both error types funnel into [`shingetsu::VmError`] for a
        /// single return type.
        ///
        /// Bounds use [`CallableArgs`]/[`CallableResult`], which
        /// require the engine-native conversion traits for whichever
        /// backends are compiled.  Callers typically use
        /// engine-portable scalars or [`crate::SerdeLua`] wrappers.
        /// After the end-state `s/shingetsu_migrate::/shingetsu::`
        /// rewrite the type is `shingetsu::Callable` and the mlua
        /// half of these bounds disappears.
        pub async fn call<A, R>(&self, args: A) -> Result<R, VmError>
        where
            A: super::CallableArgs,
            R: super::CallableResult,
        {
            match &self.0 {
                Backend::Shingetsu(c) => c.call::<A, R>(args).await,
                #[cfg(feature = "mlua-backend")]
                Backend::Mlua { lua, func } => {
                    let ret: mlua::MultiValue = func
                        .call_async::<mlua::MultiValue>(args)
                        .await
                        .map_err(map_mlua_err)?;
                    <R as mlua::FromLuaMulti>::from_lua_multi(ret, lua).map_err(map_mlua_err)
                }
            }
        }
    }

    #[cfg(feature = "mlua-backend")]
    fn map_mlua_err(e: mlua::Error) -> VmError {
        VmError::HostError {
            name: "LuaCallback::call".to_owned(),
            source: format!("{e}").into(),
        }
    }
}

/// Trait alias for argument types accepted by
/// [`LuaCallback::call`].  Just sums the engine-native
/// `IntoLuaMulti` bounds for whichever backends are compiled.
#[cfg(feature = "shingetsu-backend")]
pub trait CallableArgs: shingetsu::IntoLuaMulti + __MluaArgsBound {}

#[cfg(feature = "shingetsu-backend")]
impl<T: shingetsu::IntoLuaMulti + __MluaArgsBound> CallableArgs for T {}

/// Trait alias for return types decodable from
/// [`LuaCallback::call`].  Just sums the engine-native
/// `FromLuaMulti` bounds for whichever backends are compiled.
#[cfg(feature = "shingetsu-backend")]
pub trait CallableResult: shingetsu::FromLuaMulti + __MluaResultBound {}

#[cfg(feature = "shingetsu-backend")]
impl<T: shingetsu::FromLuaMulti + __MluaResultBound> CallableResult for T {}

// When the mlua backend is enabled the bound is the engine-native
// `IntoLuaMulti`/`FromLuaMulti`; otherwise it's an empty
// constraint satisfied by all types.
#[doc(hidden)]
#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
pub trait __MluaArgsBound: mlua::IntoLuaMulti {}
#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
impl<T: mlua::IntoLuaMulti> __MluaArgsBound for T {}

#[doc(hidden)]
#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
pub trait __MluaResultBound: mlua::FromLuaMulti {}
#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
impl<T: mlua::FromLuaMulti> __MluaResultBound for T {}

#[doc(hidden)]
#[cfg(all(feature = "shingetsu-backend", not(feature = "mlua-backend")))]
pub trait __MluaArgsBound {}
#[cfg(all(feature = "shingetsu-backend", not(feature = "mlua-backend")))]
impl<T> __MluaArgsBound for T {}

#[doc(hidden)]
#[cfg(all(feature = "shingetsu-backend", not(feature = "mlua-backend")))]
pub trait __MluaResultBound {}
#[cfg(all(feature = "shingetsu-backend", not(feature = "mlua-backend")))]
impl<T> __MluaResultBound for T {}

#[cfg(feature = "mlua-backend")]
mod mlua_impls {
    use super::{Backend, LuaCallback};

    impl mlua::FromLua for LuaCallback {
        fn from_lua(value: mlua::Value, lua: &mlua::Lua) -> mlua::Result<Self> {
            let func = mlua::Function::from_lua(value, lua)?;
            Ok(LuaCallback(Backend::Mlua {
                lua: lua.clone(),
                func,
            }))
        }
    }
}
