//! A captured Lua [`Function`] paired with its [`GlobalEnv`], usable as a
//! host-side callback from arbitrary Rust code (including detached
//! background tasks).
//!
//! `Callable` is the supported way to "accept a Lua function from policy,
//! stash it, and call it later from Rust".  Conversion via [`FromLua`]
//! captures both the function and the environment in which it was
//! supplied, so callers do not need to track an extra engine handle.

use crate::convert::{FromLua, FromLuaMulti, IntoLuaMulti, LuaTyped};
use crate::error::VmError;
use crate::function::Function;
use crate::global_env::GlobalEnv;
use crate::task::Task;
use crate::types::{LuaType, ValueType};
use crate::value::Value;

/// A captured Lua function callable from Rust.
///
/// Cheap to clone (both fields are `Arc`-backed).  `Send + Sync`, so it
/// may be moved into background tasks.
#[derive(Clone)]
pub struct Callable {
    env: GlobalEnv,
    func: Function,
}

impl Callable {
    /// Construct directly from an env + function pair.  Most users obtain
    /// a `Callable` via [`FromLua`] in a parameter position instead.
    pub fn new(env: GlobalEnv, func: Function) -> Self {
        Self { env, func }
    }

    /// The underlying [`Function`].
    pub fn function(&self) -> &Function {
        &self.func
    }

    /// The [`GlobalEnv`] this callable was captured against.
    pub fn env(&self) -> &GlobalEnv {
        &self.env
    }

    /// Invoke the captured function with the given arguments and decode
    /// the result.
    pub async fn call<A, R>(&self, args: A) -> Result<R, VmError>
    where
        A: IntoLuaMulti,
        R: FromLuaMulti,
    {
        let values = args.into_lua_multi();
        let result = Task::new(self.env.clone(), self.func.clone(), values)
            .await
            .map_err(|re| re.error)?;
        R::from_lua_multi(result, &self.env)
    }
}

impl std::fmt::Debug for Callable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Callable").finish_non_exhaustive()
    }
}

impl FromLua for Callable {
    fn from_lua(v: Value, env: &GlobalEnv) -> Result<Self, VmError> {
        let func = Function::from_lua(v, env)?;
        Ok(Callable {
            env: env.clone(),
            func,
        })
    }
}

impl LuaTyped for Callable {
    fn lua_type() -> LuaType {
        Function::lua_type()
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Function)
    }
}
