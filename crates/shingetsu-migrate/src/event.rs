//! Cross-engine event signature, dispatch, and broadcast.
//!
//! The migration facade exposes two event-dispatch APIs:
//!
//! - [`EventSignature<A, R>`] for typed events with kumomta-style
//!   dispatch (`Single`: first handler wins; `Multiple`: first
//!   non-empty result wins).  Constructed statically via
//!   [`crate::declare_event!`] for kumomta's existing pattern, or
//!   dynamically via [`EventSignature::new_single`] /
//!   [`EventSignature::new_multiple`] for wezterm's
//!   `emit_sync_callback` / `emit_async_callback` use case where
//!   the event name is a runtime string.
//!
//! - [`emit_event`] for wezterm-style untyped broadcast: every
//!   handler runs in registration order; if any returns `false`,
//!   subsequent handlers are skipped and the call returns
//!   `Ok(false)` to indicate that the default action should be
//!   suppressed.  Otherwise returns `Ok(true)`.  Args are
//!   `IntoLuaMulti`-shaped on whichever backend is active.
//!
//! Both APIs accept a polymorphic dispatch target via the
//! [`EventDispatchTarget`] trait so existing call sites that pass
//! `&mlua::Lua` keep working unchanged once the host swaps in the
//! facade; new code passes `&Engine` (or `&shingetsu::GlobalEnv`)
//! and the same call dispatches the right way.
//!
//! On the shingetsu side, both APIs route through the env's
//! [`shingetsu::callback::CallbackRegistry`].  On the mlua side
//! the keys match each consumer's existing layout: kumomta uses
//! the bare event name (matching `CallbackSignature::call`),
//! wezterm's `emit_event` walks the `wezterm-event-<name>` slot
//! (matching `config::lua::emit_event`).  See [`MLUA_KEY_PREFIX`]
//! and [`MLUA_BROADCAST_KEY_PREFIX`].

#![cfg(any(feature = "shingetsu-backend", feature = "mlua-backend"))]

use std::marker::PhantomData;

use crate::Engine;

/// Storage prefix for typed-event handlers on the mlua side.
/// Empty string by design: kumomta's `CallbackSignature::call`
/// reads `lua.named_registry_value(name)` directly with no prefix,
/// so the facade matches that layout for transparent migration.
pub const MLUA_KEY_PREFIX: &str = "";

/// Storage prefix for [`emit_event`] broadcast handlers on the mlua
/// side.  Matches wezterm's existing `wezterm-event-<name>`
/// convention so handlers registered through wezterm's existing
/// `register_event` are visible to the facade unchanged.
pub const MLUA_BROADCAST_KEY_PREFIX: &str = "wezterm-event-";

/// Borrowed reference to the active backend's underlying state.
/// Used internally by [`EventDispatchTarget`] impls so the
/// dispatch entry points have a single shape regardless of what
/// the caller passed in.
#[non_exhaustive]
pub enum EngineRef<'a> {
    #[cfg(feature = "shingetsu-backend")]
    Shingetsu(&'a shingetsu::GlobalEnv),
    #[cfg(feature = "mlua-backend")]
    Mlua(&'a mlua::Lua),
}

/// Polymorphic dispatch target.  Implemented for the wrapper type
/// [`Engine`] and for each backend's native engine handle, so that
/// migrating call sites can pass `&Lua`, `&GlobalEnv`, or
/// `&Engine` interchangeably.  Hosts whose existing code passes
/// `&Lua` keep working unchanged while migrating; new code passes
/// `&Engine`.
pub trait EventDispatchTarget {
    fn engine_ref(&self) -> EngineRef<'_>;
}

#[cfg(feature = "mlua-backend")]
impl EventDispatchTarget for mlua::Lua {
    fn engine_ref(&self) -> EngineRef<'_> {
        EngineRef::Mlua(self)
    }
}

#[cfg(feature = "shingetsu-backend")]
impl EventDispatchTarget for shingetsu::GlobalEnv {
    fn engine_ref(&self) -> EngineRef<'_> {
        EngineRef::Shingetsu(self)
    }
}

impl EventDispatchTarget for Engine {
    fn engine_ref(&self) -> EngineRef<'_> {
        match self {
            #[cfg(feature = "shingetsu-backend")]
            Engine::Shingetsu(env) => EngineRef::Shingetsu(env),
            #[cfg(feature = "mlua-backend")]
            Engine::Mlua(lua) => EngineRef::Mlua(lua),
        }
    }
}

/// Disposition returned by [`EventSignature::call`].  Mirrors the
/// shape of [`shingetsu::CallbackDisposition`] and kumomta's
/// `CallbackDisposition` so hosts pattern-matching on it migrate
/// via field-name renames only.
#[derive(Debug)]
pub struct EventDisposition<R> {
    /// True when at least one handler was registered for this name.
    pub handler_was_defined: bool,
    /// Result returned by the (last) handler, if any.  In the
    /// multi-handler case, the first handler that returned a
    /// non-empty result wins.
    pub result: Option<R>,
    /// The event name that was looked up.
    pub event_name: String,
}

/// Error returned by [`EventSignature::call`] / [`emit_event`]
/// when a handler runs but fails, or when value conversion across
/// the dispatch boundary fails.  Engine-tagged so callers can
/// branch on engine-specific cases when needed; shingetsu errors
/// keep their structured form so the original annotated-snippet
/// diagnostic survives via `Display`.
#[derive(Debug)]
pub enum EventError {
    #[cfg(feature = "shingetsu-backend")]
    Shingetsu(shingetsu::error::RuntimeError),
    #[cfg(feature = "shingetsu-backend")]
    ShingetsuVm(shingetsu::VmError),
    #[cfg(feature = "mlua-backend")]
    Mlua(mlua::Error),
}

impl std::fmt::Display for EventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(feature = "shingetsu-backend")]
            Self::Shingetsu(e) => write!(
                f,
                "{}",
                shingetsu::diagnostic::render_runtime_error(
                    e,
                    shingetsu::diagnostic::RenderStyle::Plain,
                )
            ),
            #[cfg(feature = "shingetsu-backend")]
            Self::ShingetsuVm(e) => write!(f, "{e}"),
            #[cfg(feature = "mlua-backend")]
            Self::Mlua(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EventError {}

#[cfg(feature = "shingetsu-backend")]
impl From<shingetsu::error::RuntimeError> for EventError {
    fn from(e: shingetsu::error::RuntimeError) -> Self {
        Self::Shingetsu(e)
    }
}

#[cfg(feature = "shingetsu-backend")]
impl From<shingetsu::VmError> for EventError {
    fn from(e: shingetsu::VmError) -> Self {
        Self::ShingetsuVm(e)
    }
}

#[cfg(feature = "mlua-backend")]
impl From<mlua::Error> for EventError {
    fn from(e: mlua::Error) -> Self {
        Self::Mlua(e)
    }
}

/// Cross-engine event signature.  `A` is the argument tuple; `R`
/// is the handler's return type.  Constructed via
/// [`crate::declare_event!`] for static names, or via
/// [`Self::new_single`] / [`Self::new_multiple`] for dynamic
/// names (the wezterm `emit_sync_callback` / `emit_async_callback`
/// pattern).
pub struct EventSignature<A, R> {
    name: String,
    allow_multiple: bool,
    _marker: PhantomData<fn(A) -> R>,
}

impl<A, R> EventSignature<A, R> {
    /// Construct a single-handler signature.  Multi-handler
    /// kumomta-style "first non-empty" iteration is opt-in via
    /// [`Self::new_multiple`].  Accepts any `Into<String>` so the
    /// macro can pass a `&'static str` literal and dynamic
    /// callers can pass an owned name.
    pub fn new_single(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            allow_multiple: false,
            _marker: PhantomData,
        }
    }

    /// Construct a multi-handler signature.  Handlers run in
    /// registration order until one returns a non-empty result.
    pub fn new_multiple(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            allow_multiple: true,
            _marker: PhantomData,
        }
    }

    /// The event name this signature dispatches under.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether multiple handlers may be registered for this event.
    pub fn allow_multiple(&self) -> bool {
        self.allow_multiple
    }

    /// Register this signature on the engine's registry so the
    /// active backend recognises the name.  On the shingetsu side
    /// this declares the static name on the env's
    /// `CallbackRegistry`; on the mlua side this is a no-op.
    /// Idempotent.  Hosts call this once at engine construction
    /// for every statically declared signature.
    pub fn register<T: EventDispatchTarget>(&self, target: &T) {
        match target.engine_ref() {
            #[cfg(feature = "shingetsu-backend")]
            EngineRef::Shingetsu(env) => {
                shingetsu::callback::callback_registry(env).declare_static(
                    shingetsu::Bytes::from(self.name.as_str()),
                    self.allow_multiple,
                );
            }
            #[cfg(feature = "mlua-backend")]
            EngineRef::Mlua(_) => {}
        }
    }

    /// The mlua named-registry key used to store handlers for
    /// this signature.  Public so the upcoming `on()` registration
    /// entry point and any host-side callback walker can use the
    /// same key.
    #[cfg(feature = "mlua-backend")]
    pub fn mlua_registry_key(&self) -> String {
        format!("{MLUA_KEY_PREFIX}{}", self.name)
    }
}

#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
impl<A, R> EventSignature<A, R>
where
    A: shingetsu::IntoLuaMulti + mlua::IntoLuaMulti + Clone + 'static,
    R: shingetsu::FromLuaMulti + mlua::FromLuaMulti + 'static,
{
    /// Invoke the registered handler(s) for this event on whichever
    /// backend the dispatch target wraps.  Accepts `&Lua`,
    /// `&GlobalEnv`, or `&Engine`.
    pub async fn call<T: EventDispatchTarget + ?Sized>(
        &self,
        target: &T,
        args: A,
    ) -> Result<EventDisposition<R>, EventError> {
        match target.engine_ref() {
            EngineRef::Shingetsu(env) => self.call_shingetsu(env, args).await,
            EngineRef::Mlua(lua) => self.call_mlua(lua, args).await,
        }
    }

    async fn call_shingetsu(
        &self,
        env: &shingetsu::GlobalEnv,
        args: A,
    ) -> Result<EventDisposition<R>, EventError> {
        let sig: shingetsu::callback::CallbackSignature<A, R> = if self.allow_multiple {
            shingetsu::callback::CallbackSignature::new_multiple(self.name.as_str())
        } else {
            shingetsu::callback::CallbackSignature::new(self.name.as_str())
        };
        let disp = sig.call(env, args).await?;
        Ok(EventDisposition {
            handler_was_defined: disp.handler_was_defined,
            result: disp.result,
            event_name: String::from_utf8_lossy(&disp.event_name).into_owned(),
        })
    }

    async fn call_mlua(&self, lua: &mlua::Lua, args: A) -> Result<EventDisposition<R>, EventError> {
        let key = self.mlua_registry_key();
        let stored: mlua::Value = lua.named_registry_value(&key).unwrap_or(mlua::Value::Nil);
        match stored {
            mlua::Value::Nil => Ok(EventDisposition {
                handler_was_defined: false,
                result: None,
                event_name: self.name.clone(),
            }),
            mlua::Value::Function(f) => {
                let r: R = f.call_async(args).await?;
                Ok(EventDisposition {
                    handler_was_defined: true,
                    result: Some(r),
                    event_name: self.name.clone(),
                })
            }
            mlua::Value::Table(tbl) => {
                // Multi-handler: walk the sequence in registration
                // order, return the first non-empty result.  Empty
                // results (handlers that returned nothing) flow
                // through to the next handler.  Matches kumomta's
                // `CallbackSignature::call` semantics for the
                // `allow_multiple` case.
                for func in tbl.sequence_values::<mlua::Function>() {
                    let func = func?;
                    let raw: mlua::MultiValue = func.call_async(args.clone()).await?;
                    if !raw.is_empty() {
                        let r = <R as mlua::FromLuaMulti>::from_lua_multi(raw, lua)?;
                        return Ok(EventDisposition {
                            handler_was_defined: true,
                            result: Some(r),
                            event_name: self.name.clone(),
                        });
                    }
                }
                Ok(EventDisposition {
                    handler_was_defined: true,
                    result: None,
                    event_name: self.name.clone(),
                })
            }
            other => Err(EventError::Mlua(mlua::Error::external(format!(
                "EventSignature::call: registry slot '{key}' holds a {} (expected nil, function, or table of functions)",
                other.type_name()
            )))),
        }
    }
}

// ---------------------------------------------------------------------------
// emit_event broadcast: wezterm-style "all run, false short-circuits"
// ---------------------------------------------------------------------------

/// Broadcast an event to every handler registered under `name`,
/// matching wezterm's `config::lua::emit_event` semantics.
///
/// Handlers run in registration order; if any handler returns
/// `false` (Lua false), subsequent handlers are skipped and this
/// call returns `Ok(false)` so the host can suppress the default
/// action.  Otherwise (every handler approves, or no handlers are
/// registered) returns `Ok(true)`.
///
/// On the mlua side this walks the
/// `wezterm-event-<name>` named-registry slot (the existing
/// wezterm convention) so handlers registered through wezterm's
/// existing `register_event` continue to work.  On the shingetsu
/// side it walks the env's [`shingetsu::callback::CallbackRegistry`]
/// looking up `name` directly.
#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
pub async fn emit_event<T, A>(target: &T, name: &str, args: A) -> Result<bool, EventError>
where
    T: EventDispatchTarget + ?Sized,
    A: shingetsu::IntoLuaMulti + mlua::IntoLuaMulti + Clone + 'static,
{
    match target.engine_ref() {
        EngineRef::Mlua(lua) => emit_event_mlua(lua, name, args).await,
        EngineRef::Shingetsu(env) => emit_event_shingetsu(env, name, args).await,
    }
}

#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
async fn emit_event_mlua<A>(lua: &mlua::Lua, name: &str, args: A) -> Result<bool, EventError>
where
    A: mlua::IntoLuaMulti + Clone + 'static,
{
    let key = format!("{MLUA_BROADCAST_KEY_PREFIX}{name}");
    let stored: mlua::Value = lua.named_registry_value(&key).unwrap_or(mlua::Value::Nil);
    let tbl = match stored {
        mlua::Value::Table(t) => t,
        _ => return Ok(true),
    };
    for func in tbl.sequence_values::<mlua::Function>() {
        let func = func?;
        match func.call_async(args.clone()).await? {
            mlua::Value::Boolean(false) => return Ok(false),
            _ => {}
        }
    }
    Ok(true)
}

#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
async fn emit_event_shingetsu<A>(
    env: &shingetsu::GlobalEnv,
    name: &str,
    args: A,
) -> Result<bool, EventError>
where
    A: shingetsu::IntoLuaMulti + Clone + 'static,
{
    use shingetsu::Value;

    let registry = shingetsu::callback::callback_registry(env);
    let handlers = registry.handlers(name.as_bytes());
    for func in handlers {
        let argv = args.clone().into_lua_multi();
        let result = shingetsu::Task::new(env.clone(), func, argv).await?;
        if matches!(result.first(), Some(Value::Boolean(false))) {
            return Ok(false);
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// declare_event! macro
// ---------------------------------------------------------------------------

/// Declare a cross-engine event signature in a static slot.
///
/// Mirrors kumomta's `declare_event!` spelling so a migrating host
/// can replace `config::declare_event!` with
/// `shingetsu_migrate::declare_event!` and keep the same syntax:
///
/// ```ignore
/// shingetsu_migrate::declare_event! {
///     pub static GET_QUEUE_CONFIG: Multiple(
///         "get_queue_config",
///         domain: String,
///         tenant: Option<String>,
///     ) -> QueueConfig;
/// }
///
/// shingetsu_migrate::declare_event! {
///     pub static ON_RESET: Single("on_reset") -> ();
/// }
/// ```
///
/// Expands to a `LazyLock<EventSignature<A, R>>` where the
/// parameter list `(name: Type, ...)` becomes the tuple `A` and
/// the return type becomes `R`.
#[macro_export]
macro_rules! declare_event {
    (
        $vis:vis static $sym:ident: Single(
            $name:literal $(, $param:ident : $param_ty:ty )* $(,)?
        ) -> $ret:ty;
    ) => {
        $vis static $sym: ::std::sync::LazyLock<
            $crate::EventSignature<( $($param_ty,)* ), $ret>
        > = ::std::sync::LazyLock::new(|| {
            $crate::EventSignature::new_single($name)
        });
    };
    (
        $vis:vis static $sym:ident: Multiple(
            $name:literal $(, $param:ident : $param_ty:ty )* $(,)?
        ) -> $ret:ty;
    ) => {
        $vis static $sym: ::std::sync::LazyLock<
            $crate::EventSignature<( $($param_ty,)* ), $ret>
        > = ::std::sync::LazyLock::new(|| {
            $crate::EventSignature::new_multiple($name)
        });
    };
}
