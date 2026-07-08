//! VM-level globals: `pcall`, `xpcall`, `require`.
//!
//! These three sit beneath the higher-level built-ins (`type`,
//! `tostring`, `rawget`, ...) provided by the `shingetsu` crate.
//! They live here because their bodies need access to VM internals
//! (the preload/loaded caches, the module loader, the protected-call
//! plumbing on `CallContext`) that are crate-private to
//! `shingetsu-vm`.
//!
//! [`register`] is invoked unconditionally from
//! [`GlobalEnv::new`](crate::GlobalEnv::new); the higher-level
//! `shingetsu::builtins` module merges its entries onto the same
//! `"builtins"` module-type bucket via
//! [`merge_module_types`](crate::types).

use crate::byte_string::Bytes;
use crate::call_context::CallContext;
use crate::convert::{FromLuaMulti, IntoLua, IntoLuaMulti, LuaTyped, Variadic};
use crate::error::VmError;
use crate::function::Function;
use crate::global_env::GlobalEnv;
use crate::types::LuaType;
use crate::value::{Value, ValueVec};
use crate::{valuevec, LuaTypedMulti};

/// Argument shape for `pcall(f, ...)`.
///
/// Captures whether the caller passed *any* first argument so the
/// no-arg case (`pcall()`) can be distinguished from
/// `pcall(nil)`.  The `FromLuaMulti` derive on enums dispatches
/// by exact arg count and doesn't accept a `Variadic` tail, so we
/// hand-roll the impl.
struct PcallArgs {
    /// `None` when the script called `pcall()` with no arguments;
    /// `Some(value)` when at least one argument was supplied (even
    /// if it was explicitly `nil`).
    f: Option<Value>,
    /// Remaining arguments after the first.
    rest: ValueVec,
}

impl FromLuaMulti for PcallArgs {
    fn from_lua_multi(values: ValueVec, _env: &GlobalEnv) -> Result<Self, VmError> {
        let mut it = values.into_iter();
        let f = it.next();
        Ok(PcallArgs {
            f,
            rest: it.collect(),
        })
    }
}

impl LuaTypedMulti for PcallArgs {
    fn lua_types() -> Vec<LuaType> {
        vec![
            Function::lua_type(),
            LuaType::Variadic(Box::new(LuaType::Any)),
        ]
    }
    fn lua_param_names() -> Vec<Option<&'static str>> {
        vec![Some("f"), None]
    }
}

/// Argument shape for `xpcall(f, msgh, ...)`.
///
/// Like [`PcallArgs`] but with a separate slot for the message
/// handler so each of the two leading positions can be queried
/// for presence independently.
struct XpcallArgs {
    /// `None` when the script supplied no first argument.
    f: Option<Value>,
    /// `None` when the script supplied no second argument.
    msgh: Option<Value>,
    /// Remaining arguments forwarded to `f`.
    rest: ValueVec,
}

impl FromLuaMulti for XpcallArgs {
    fn from_lua_multi(values: ValueVec, _env: &GlobalEnv) -> Result<Self, VmError> {
        let mut it = values.into_iter();
        let f = it.next();
        let msgh = it.next();
        Ok(XpcallArgs {
            f,
            msgh,
            rest: it.collect(),
        })
    }
}

impl LuaTypedMulti for XpcallArgs {
    fn lua_types() -> Vec<LuaType> {
        vec![
            Function::lua_type(),
            Function::lua_type(),
            LuaType::Variadic(Box::new(LuaType::Any)),
        ]
    }
    fn lua_param_names() -> Vec<Option<&'static str>> {
        vec![Some("f"), Some("msgh"), None]
    }
}

/// Singleton type whose only inhabitant maps to `Value::Boolean(true)`
/// and whose `LuaType` is `BoolLiteral(true)`.  Used as the
/// discriminator on the success arm of [`ProtectedReturn`] so the
/// type checker sees that arm as `(true, ...any)` rather than
/// `(boolean, ...any)`.
struct TrueLit;

impl IntoLua for TrueLit {
    fn into_lua(self) -> Value {
        Value::Boolean(true)
    }
}

impl LuaTyped for TrueLit {
    fn lua_type() -> LuaType {
        LuaType::BoolLiteral(true)
    }
}

/// Singleton type whose only inhabitant maps to `Value::Boolean(false)`
/// and whose `LuaType` is `BoolLiteral(false)`.  Used as the
/// discriminator on the error arm of [`ProtectedReturn`].
struct FalseLit;

impl IntoLua for FalseLit {
    fn into_lua(self) -> Value {
        Value::Boolean(false)
    }
}

impl LuaTyped for FalseLit {
    fn lua_type() -> LuaType {
        LuaType::BoolLiteral(false)
    }
}

/// Return shape for `pcall` / `xpcall`: either `(true, ...rets)` on
/// success or `(false, msg)` on error.
///
/// The `IntoLuaMulti` and `LuaTypedMulti` impls below mirror what
/// `#[derive(IntoLuaMulti)]` would emit for this enum (a
/// `Union<Tuple>` Lua type), but are written by hand because the
/// enum derives in `shingetsu-derive-impl` hard-code `::shingetsu`
/// paths and have no `crate = "..."` escape hatch for use from
/// inside `shingetsu-vm` itself.
enum ProtectedReturn {
    Ok(TrueLit, Variadic),
    Err(FalseLit, Value),
}

impl IntoLuaMulti for ProtectedReturn {
    fn into_lua_multi(self) -> ValueVec {
        match self {
            ProtectedReturn::Ok(t, v) => {
                let mut out = ValueVec::with_capacity(v.0.len() + 1);
                out.push(t.into_lua());
                out.extend(v.0);
                out
            }
            ProtectedReturn::Err(f, val) => valuevec![f.into_lua(), val.into_lua()],
        }
    }
}

impl LuaTypedMulti for ProtectedReturn {
    fn lua_types() -> Vec<LuaType> {
        vec![LuaType::Union(vec![
            LuaType::Tuple(vec![TrueLit::lua_type(), Variadic::lua_type()]),
            LuaType::Tuple(vec![FalseLit::lua_type(), Value::lua_type()]),
        ])]
    }
}

/// Build a `ProtectedReturn` from the `(ok, rest)` pair returned by
/// [`CallContext::protected_call`].
fn pack(ok: bool, mut rest: ValueVec) -> ProtectedReturn {
    if ok {
        ProtectedReturn::Ok(TrueLit, Variadic(rest))
    } else {
        ProtectedReturn::Err(FalseLit, rest.drain(..).next().unwrap_or(Value::Nil))
    }
}

#[shingetsu_derive::module(name = "builtins", crate = "crate")]
#[allow(clippy::module_inception)] // the derive generates a `builtins` module inside this file
mod builtins {
    use super::*;

    /// Calls `f` in protected mode with the given arguments.
    ///
    /// Returns `(true, ...rets)` on success, where `rets` are the
    /// values `f` returned, and `(false, msg)` if `f` raised an
    /// error.  Errors raised by `os.exit` are not catchable: they
    /// propagate to the surrounding task.
    ///
    /// # Parameters
    ///
    /// - `f` — the callable to invoke.
    /// - `...` — arguments forwarded to `f`.
    ///
    /// # Returns
    ///
    /// - on success, `(true, ...rets)` where `rets` are the values
    ///   returned by `f`; on error, `(false, msg)` where `msg` is
    ///   the error value.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local ok, val = pcall(function() return 42 end)
    /// assert(ok == true and val == 42)
    ///
    /// local ok, err = pcall(function() error("nope") end)
    /// assert(ok == false)
    /// assert(string.find(err, "nope") ~= nil)
    /// ```
    #[function(variadic)]
    async fn pcall(ctx: CallContext, args: PcallArgs) -> Result<ProtectedReturn, VmError> {
        let func = match args.f {
            None => {
                return Ok(ProtectedReturn::Err(
                    FalseLit,
                    Value::string("bad argument #1 to 'pcall' (value expected)"),
                ));
            }
            Some(Value::Function(func)) => func,
            Some(other) => {
                let msg = format!("attempt to call a {} value", other.type_name());
                return Ok(ProtectedReturn::Err(FalseLit, Value::string(msg)));
            }
        };
        let (ok, rest) = ctx.protected_call(func, args.rest).await?;
        Ok(pack(ok, rest))
    }

    /// Calls `f` in protected mode with `msgh` as the message handler.
    ///
    /// Like `pcall`, but if `f` raises an error, the error value is
    /// passed through `msgh(err)` before being returned.  This lets
    /// the caller capture a traceback or rewrite the error message.
    /// `msgh` itself runs unprotected; an error inside it propagates.
    ///
    /// # Parameters
    ///
    /// - `f` — the callable to invoke.
    /// - `msgh` — the message handler called with the error value.
    /// - `...` — arguments forwarded to `f`.
    ///
    /// # Returns
    ///
    /// - on success, `(true, ...rets)` where `rets` are the values
    ///   returned by `f`; on error, `(false, ...handler_rets)`
    ///   where `handler_rets` are whatever `msgh(err)` returned.
    ///
    /// # Examples
    ///
    /// ```lua
    /// local ok, msg = xpcall(
    ///   function() error("boom") end,
    ///   function(err) return "handled: " .. err end
    /// )
    /// assert(ok == false)
    /// assert(string.find(msg, "handled: ") ~= nil)
    /// assert(string.find(msg, "boom") ~= nil)
    /// ```
    #[function(variadic)]
    async fn xpcall(ctx: CallContext, args: XpcallArgs) -> Result<ProtectedReturn, VmError> {
        let func = match args.f {
            None => {
                return Ok(ProtectedReturn::Err(
                    FalseLit,
                    Value::string("bad argument #1 to 'xpcall' (value expected)"),
                ));
            }
            Some(Value::Function(func)) => func,
            Some(other) => {
                let msg = format!("attempt to call a {} value", other.type_name());
                return Ok(ProtectedReturn::Err(FalseLit, Value::string(msg)));
            }
        };
        let handler = match args.msgh {
            Some(Value::Function(h)) => Some(h),
            _ => None,
        };
        let (ok, rest) = ctx.protected_call(func, args.rest).await?;
        if !ok {
            if let Some(h) = handler {
                let err_val = rest.into_iter().next().unwrap_or(Value::Nil);
                let (_, handler_rest) = ctx.protected_call(h, valuevec![err_val]).await?;
                return Ok(pack(false, handler_rest));
            }
        }
        Ok(pack(ok, rest))
    }

    /// Loads and returns the module named `modname`.
    ///
    /// The lookup order is:
    ///
    /// 1. The `loaded` cache, so a module is loaded at most once.
    /// 2. The `preload` registry, populated by hosts via
    ///    `register_preload`.
    /// 3. File-based search through `package.path` if a module
    ///    loader has been configured (typically by enabling
    ///    `Libraries::PACKAGE` from the host).
    ///
    /// Successful results are cached in `loaded`.
    ///
    /// # Parameters
    ///
    /// - `modname` — the module name as a string.
    ///
    /// # Returns
    ///
    /// - the module's value (typically a table).
    ///
    /// # Examples
    ///
    /// ```lua
    /// local math = require("math")
    /// assert(math.pi > 3.14)
    /// ```
    #[function]
    async fn require(ctx: CallContext, modname: Bytes) -> Result<Value, VmError> {
        ctx.global.require(modname).await
    }
}

/// Install the VM-level builtins (`pcall`, `xpcall`, `require`) on
/// `env`.  Called unconditionally from
/// [`GlobalEnv::new`](crate::GlobalEnv::new); embedders do not
/// normally call it directly.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = builtins::build_module_table(env)?;
    env.register_from_table(&table)?;
    env.register_module_type("builtins", builtins::module_type());
    Ok(())
}
