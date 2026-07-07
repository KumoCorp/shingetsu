//! A captured Lua [`Function`] paired with its [`GlobalEnv`], usable as a
//! host-side callback from arbitrary Rust code (including detached
//! background tasks).
//!
//! `Callable` is the supported way to "accept a Lua function from policy,
//! stash it, and call it later from Rust".  Conversion via [`FromLua`]
//! captures both the function and the environment in which it was
//! supplied, so callers do not need to track an extra engine handle.

use crate::convert::{FromLua, FromLuaMulti, IntoLuaMulti, LuaTyped, LuaTypedMulti};
use crate::error::{RuntimeError, VmError};
use crate::function::Function;
use crate::global_env::GlobalEnv;
use crate::sync::Mutex;
use crate::task::Task;
use crate::types::{FunctionLuaType, LuaType, TypedParam, ValueType};
use crate::value::Value;
use std::marker::PhantomData;
use std::sync::Arc;

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

/// A captured Lua function with a declared signature `fn(A) -> R`.
///
/// Unlike [`Callable`], the argument tuple `A` and return type `R` are
/// fixed at the type level, so [`call`](TypedCallable::call) needs no
/// turbofish and the type checker learns the exact shape a supplied Lua
/// function must satisfy: [`LuaTyped::lua_type`] reports a concrete
/// [`LuaType::Function`] built from `A` and `R` rather than a bare
/// `function`.
///
/// The parameters reported by [`LuaTyped::lua_type`] are unnamed.  Use
/// the `declare_callable!` macro to attach parameter names and docs.
///
/// Cheap to clone (both live fields are `Arc`-backed).  `Send + Sync`,
/// so it may be moved into background tasks.
pub struct TypedCallable<A, R> {
    env: GlobalEnv,
    func: Function,
    _marker: PhantomData<fn(A) -> R>,
}

impl<A, R> Clone for TypedCallable<A, R> {
    fn clone(&self) -> Self {
        Self {
            env: self.env.clone(),
            func: self.func.clone(),
            _marker: PhantomData,
        }
    }
}

impl<A, R> TypedCallable<A, R> {
    /// Construct directly from an env + function pair.  Most users obtain
    /// a `TypedCallable` via [`FromLua`] in a parameter or field position
    /// instead.
    pub fn new(env: GlobalEnv, func: Function) -> Self {
        Self {
            env,
            func,
            _marker: PhantomData,
        }
    }

    /// The underlying [`Function`].
    pub fn function(&self) -> &Function {
        &self.func
    }

    /// The [`GlobalEnv`] this callable was captured against.
    pub fn env(&self) -> &GlobalEnv {
        &self.env
    }
}

impl<A, R> TypedCallable<A, R>
where
    A: IntoLuaMulti,
    R: FromLuaMulti,
{
    /// Invoke the captured function with `args` and decode the result as
    /// `R`.
    ///
    /// A return-value type mismatch is recast as a
    /// [`VmError::ReturnValueMismatch`] and anchored at the handler's
    /// `return`, so rendering the [`RuntimeError`] points at the
    /// offending source rather than the call site.
    pub async fn call(&self, args: A) -> Result<R, RuntimeError> {
        let values = args.into_lua_multi();
        let site = Arc::new(Mutex::new(None));
        let mut task = Task::new(self.env.clone(), self.func.clone(), values);
        task.set_capture_return_site(Arc::clone(&site));
        let raw = task.await?;
        R::from_lua_multi(raw, &self.env)
            .map_err(|vm| RuntimeError::from_return_conversion(vm, site.lock().take()))
    }
}

impl<A, R> std::fmt::Debug for TypedCallable<A, R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TypedCallable").finish_non_exhaustive()
    }
}

impl<A, R> FromLua for TypedCallable<A, R> {
    fn from_lua(v: Value, env: &GlobalEnv) -> Result<Self, VmError> {
        let func = Function::from_lua(v, env)?;
        Ok(TypedCallable {
            env: env.clone(),
            func,
            _marker: PhantomData,
        })
    }
}

impl<A, R> LuaTyped for TypedCallable<A, R>
where
    A: LuaTypedMulti,
    R: LuaTypedMulti,
{
    fn lua_type() -> LuaType {
        LuaType::Function(Box::new(FunctionLuaType {
            type_params: Vec::new(),
            params: A::lua_types()
                .into_iter()
                .map(TypedParam::unnamed)
                .collect(),
            variadic: None,
            returns: R::lua_types(),
            is_method: false,
            inferred_unannotated: false,
            deprecated: None,
            must_use: None,
        }))
    }
    fn value_type() -> Option<ValueType> {
        Some(ValueType::Function)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_callable_reports_a_concrete_function_type() {
        // The whole point of TypedCallable over Callable: lua_type()
        // exposes the declared parameter and return shape (unnamed
        // params), not a bare `function`.
        let ty = <TypedCallable<(i64, bool), i64> as LuaTyped>::lua_type();
        k9::assert_equal!(
            ty,
            LuaType::Function(Box::new(FunctionLuaType {
                type_params: Vec::new(),
                params: vec![
                    TypedParam::unnamed(LuaType::Number),
                    TypedParam::unnamed(LuaType::Boolean),
                ],
                variadic: None,
                returns: vec![LuaType::Number],
                is_method: false,
                inferred_unannotated: false,
                deprecated: None,
                must_use: None,
            }))
        );
    }

    #[test]
    fn typed_callable_with_no_args_or_returns() {
        let ty = <TypedCallable<(), ()> as LuaTyped>::lua_type();
        k9::assert_equal!(
            ty,
            LuaType::Function(Box::new(FunctionLuaType {
                type_params: Vec::new(),
                params: Vec::new(),
                variadic: None,
                returns: Vec::new(),
                is_method: false,
                inferred_unannotated: false,
                deprecated: None,
                must_use: None,
            }))
        );
    }
}
