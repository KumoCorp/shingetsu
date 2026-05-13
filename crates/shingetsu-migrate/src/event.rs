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

/// Storage prefix for event handlers on the mlua side.  Both
/// [`EventSignature::call`] (typed dispatch) and [`emit_event`]
/// (broadcast) walk `host-event-<name>` named-registry slots.
///
/// kumomta and wezterm each use slightly different prefixes today
/// (`kumomta-on-<name>` and `wezterm-event-<name>` respectively).
/// Migrating either codebase includes a small fixup to use this
/// prefix instead so the facade and the host's existing dispatch
/// paths share a registry slot during incremental migration.
pub const MLUA_KEY_PREFIX: &str = "host-event-";

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
/// shape of shingetsu's `CallbackDisposition` and kumomta's
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

/// Per-parameter metadata captured by [`crate::declare_event!`].
/// `name` and `doc` are populated regardless of which backend is
/// active so kumomta-style mlua-only doc-build pipelines can
/// consume the metadata.  `lua_type` is shingetsu-only and feeds
/// the compile-time event-handler checker (handler-lambda arity,
/// parameter-name transposition, return-shape validation).
#[derive(Debug, Clone)]
pub struct EventParam {
    pub name: String,
    /// Per-parameter rustdoc captured from `///` attributes on
    /// the parameter inside the `declare_event!` invocation.
    /// `None` when the parameter has no doc comment.
    pub doc: Option<&'static str>,
    #[cfg(feature = "shingetsu-backend")]
    pub lua_type: shingetsu::LuaType,
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
    /// Per-parameter metadata.  Populated by
    /// [`Self::new_single_typed`] / [`Self::new_multiple_typed`]
    /// (driven by the typed [`crate::declare_event!`] macro);
    /// empty for runtime-constructed signatures via
    /// [`Self::new_single`] / [`Self::new_multiple`].
    params: Vec<EventParam>,
    /// Summary doc for the event captured from `///` rustdoc on
    /// the `static` declaration inside the `declare_event!`
    /// invocation.  `None` for runtime-constructed signatures or
    /// when the user wrote no doc comment.  Always available --
    /// kumomta-style mlua-only doc-build pipelines read this to
    /// render the per-event reference pages.
    doc: Option<&'static str>,
    /// Optional rustdoc on the event's return value, captured via
    /// the `#[returns = "..."]` attribute syntax inside
    /// `declare_event!`.
    return_doc: Option<&'static str>,
    /// Declared return types as a multi-return shape.  `None` for
    /// untyped signatures; `Some(vec)` (possibly empty) for typed
    /// ones.
    #[cfg(feature = "shingetsu-backend")]
    return_types: Option<Vec<shingetsu::LuaType>>,
    _marker: PhantomData<fn(A) -> R>,
}

impl<A, R> EventSignature<A, R> {
    /// Construct a single-handler signature without typed
    /// parameter metadata.  Suitable for runtime-named events
    /// (the wezterm `emit_sync_callback` / `emit_async_callback`
    /// pattern) where compile-time handler validation isn't
    /// applicable.
    pub fn new_single(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            allow_multiple: false,
            params: Vec::new(),
            doc: None,
            return_doc: None,
            #[cfg(feature = "shingetsu-backend")]
            return_types: None,
            _marker: PhantomData,
        }
    }

    /// Construct a multi-handler signature without typed metadata.
    /// Handlers run in registration order until one returns a
    /// non-empty result.
    pub fn new_multiple(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            allow_multiple: true,
            params: Vec::new(),
            doc: None,
            return_doc: None,
            #[cfg(feature = "shingetsu-backend")]
            return_types: None,
            _marker: PhantomData,
        }
    }

    /// Construct a single-handler signature with typed parameter
    /// metadata and captured rustdoc.  Typically called from the
    /// [`crate::declare_event!`] macro expansion.  When such a
    /// signature is registered on a shingetsu engine, the compiler
    /// validates user-written handler lambdas against the captured
    /// types; mlua-only builds use the same constructor and read
    /// `doc` / `params[i].doc` / `return_doc` for kumomta-style
    /// per-event reference-page generation.
    #[cfg(feature = "shingetsu-backend")]
    pub fn new_single_typed(
        name: impl Into<String>,
        params: Vec<EventParam>,
        return_types: Vec<shingetsu::LuaType>,
        doc: Option<&'static str>,
        return_doc: Option<&'static str>,
    ) -> Self {
        Self {
            name: name.into(),
            allow_multiple: false,
            params,
            doc,
            return_doc,
            return_types: Some(return_types),
            _marker: PhantomData,
        }
    }

    /// Construct a multi-handler signature with typed metadata.
    #[cfg(feature = "shingetsu-backend")]
    pub fn new_multiple_typed(
        name: impl Into<String>,
        params: Vec<EventParam>,
        return_types: Vec<shingetsu::LuaType>,
        doc: Option<&'static str>,
        return_doc: Option<&'static str>,
    ) -> Self {
        Self {
            name: name.into(),
            allow_multiple: true,
            params,
            doc,
            return_doc,
            return_types: Some(return_types),
            _marker: PhantomData,
        }
    }

    /// mlua-only constructor for a single-handler typed
    /// signature.  Builds without `lua_type` metadata on each
    /// param (since shingetsu's `LuaType` isn't available without
    /// the shingetsu backend) but still captures `doc` /
    /// `return_doc` / param `doc` so kumomta's doc-build pipeline
    /// works in pure-mlua builds.
    #[cfg(not(feature = "shingetsu-backend"))]
    pub fn new_single_typed(
        name: impl Into<String>,
        params: Vec<EventParam>,
        doc: Option<&'static str>,
        return_doc: Option<&'static str>,
    ) -> Self {
        Self {
            name: name.into(),
            allow_multiple: false,
            params,
            doc,
            return_doc,
            _marker: PhantomData,
        }
    }

    /// mlua-only constructor for a multi-handler typed signature.
    #[cfg(not(feature = "shingetsu-backend"))]
    pub fn new_multiple_typed(
        name: impl Into<String>,
        params: Vec<EventParam>,
        doc: Option<&'static str>,
        return_doc: Option<&'static str>,
    ) -> Self {
        Self {
            name: name.into(),
            allow_multiple: true,
            params,
            doc,
            return_doc,
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

    /// Per-parameter metadata captured by the typed
    /// [`crate::declare_event!`] macro.  Always populated for
    /// macro-defined signatures regardless of backend; empty for
    /// runtime-constructed ones.
    pub fn params(&self) -> &[EventParam] {
        &self.params
    }

    /// Summary rustdoc captured from `///` comments on the
    /// `static` declaration inside `declare_event!`.
    pub fn doc(&self) -> Option<&'static str> {
        self.doc
    }

    /// Optional rustdoc describing the event's return value,
    /// captured from a `#[returns = "..."]` attribute inside
    /// `declare_event!`.
    pub fn return_doc(&self) -> Option<&'static str> {
        self.return_doc
    }

    /// Register this signature on the engine's registry so the
    /// active backend recognises the name.  On the shingetsu side
    /// this declares the static name on the env's
    /// `CallbackRegistry` and -- for typed signatures -- publishes
    /// the typed shape into the env's
    /// [`shingetsu::types::GlobalTypeMap`] so the compile-time
    /// event-handler checker can validate user-written handler
    /// lambdas.  On the mlua side this is a no-op.  Idempotent.
    /// Hosts call this once at engine construction for every
    /// statically declared signature.
    pub fn register<T: EventDispatchTarget>(&self, target: &T) {
        match target.engine_ref() {
            #[cfg(feature = "shingetsu-backend")]
            EngineRef::Shingetsu(env) => {
                shingetsu::callback::callback_registry(env).declare_static(
                    shingetsu::Bytes::from(self.name.as_str()),
                    self.allow_multiple,
                );
                if let Some(returns) = &self.return_types {
                    let ft = self.handler_function_type(returns.clone());
                    env.declare_event_handler_signature(
                        shingetsu::Bytes::from(self.name.as_str()),
                        ft,
                    );
                }
            }
            #[cfg(feature = "mlua-backend")]
            EngineRef::Mlua(_) => {}
        }
    }

    /// The mlua named-registry key used to store handlers for
    /// this signature.  Public so the `on()` registration entry
    /// point and any host-side callback walker can use the same
    /// key.
    #[cfg(feature = "mlua-backend")]
    pub fn mlua_registry_key(&self) -> String {
        format!("{MLUA_KEY_PREFIX}{}", self.name)
    }

    /// Build the [`shingetsu::types::FunctionLuaType`] a handler
    /// lambda must satisfy.  Internal helper for [`Self::register`].
    #[cfg(feature = "shingetsu-backend")]
    fn handler_function_type(
        &self,
        returns: Vec<shingetsu::LuaType>,
    ) -> shingetsu::types::FunctionLuaType {
        shingetsu::types::FunctionLuaType {
            type_params: Vec::new(),
            params: self
                .params
                .iter()
                .map(|p| {
                    shingetsu::types::TypedParam::new_with_doc(
                        Some(shingetsu::Bytes::from(p.name.as_str())),
                        p.lua_type.clone(),
                        p.doc.map(str::to_owned),
                    )
                })
                .collect(),
            variadic: None,
            returns,
            is_method: false,
            inferred_unannotated: false,
            deprecated: None,
            must_use: None,
        }
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
    let key = format!("{MLUA_KEY_PREFIX}{name}");
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
// install_on: lua-side `<host>.on(name, fn)` registration helper
// ---------------------------------------------------------------------------

/// Install a `<module_name>.on(name, fn)` callable in the engine's
/// globals so user scripts can register handlers in the same shape
/// kumomta and wezterm already provide today (`kumo.on`,
/// `wezterm.on`).  The host picks `module_name` to match its
/// existing convention.
///
/// On the shingetsu side this calls
/// [`shingetsu::callback::CallbackRegistry::register`] (which
/// honours the registry's name policy and reports duplicate
/// registrations on single-handler events).  On the mlua side
/// this stores the handler under the `host-event-<name>` named
/// registry slot, appending to a sequence-table for events
/// declared as multi-handler and rejecting duplicates on
/// single-handler events.
///
/// Idempotent across multiple calls -- mounting `kumo.on` and
/// then `wezterm.on` on the same engine is fine.
#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
pub fn install_on(engine: &Engine, module_name: &str) -> Result<(), EventError> {
    match engine {
        Engine::Shingetsu(env) => install_on_shingetsu(env, module_name),
        Engine::Mlua(lua) => install_on_mlua(lua, module_name),
    }
}

#[cfg(all(feature = "shingetsu-backend", feature = "mlua-backend"))]
fn install_on_shingetsu(env: &shingetsu::GlobalEnv, module_name: &str) -> Result<(), EventError> {
    use shingetsu::{Bytes, Function, Table, Value, VmError};

    let registry = shingetsu::callback::callback_registry(env).clone();
    let on_fn = Function::wrap(
        "on",
        move |name: Bytes, func: Function| -> Result<(), VmError> {
            // Registration errors ("unknown event name", "single
            // event already registered") are about the event name
            // arg; tag with position 1 so the caret lands on it.
            use shingetsu::VmResultExt;
            registry.register(name, func).with_arg_position(1)?;
            Ok(())
        },
    );

    // Set up the runtime module table FIRST.  `set_global`
    // re-infers the type from the value, so any
    // `register_global_type` call has to come after the table is
    // populated -- otherwise the set_global call clobbers the
    // typed registration with a fields-empty inferred shape.
    let module = match env.get_global(module_name) {
        Some(Value::Table(t)) => t,
        Some(other) => {
            return Err(EventError::ShingetsuVm(VmError::HostError {
                name: "install_on".to_owned(),
                source: format!(
                    "global '{module_name}' is a {} (expected table or nil)",
                    other.type_name(),
                )
                .into(),
            }));
        }
        None => {
            let t = Table::new();
            env.set_global(module_name, Value::Table(t.clone()));
            t
        }
    };
    module.raw_set(Value::string("on"), Value::Function(on_fn))?;

    // Publish a minimal `LuaType::Table` for the host module so the
    // compile-time type checker can resolve `<module>.on(...)`
    // calls and reach the event-handler validation pass.  Hosts
    // that want a richer module type (additional fields, doc
    // strings) call `register_global_type` after `install_on` to
    // overwrite this with a Table that still contains the `on`
    // entry.
    let on_field_type = shingetsu::types::FunctionLuaType {
        type_params: Vec::new(),
        params: vec![
            shingetsu::types::TypedParam::new(
                Some(shingetsu::Bytes::from("name")),
                shingetsu::LuaType::String,
            ),
            shingetsu::types::TypedParam::new(
                Some(shingetsu::Bytes::from("handler")),
                shingetsu::LuaType::Function(Box::new(shingetsu::types::FunctionLuaType {
                    type_params: Vec::new(),
                    params: Vec::new(),
                    // The handler signature is validated against
                    // the registered event signature at the call
                    // site; here we declare a generic
                    // (...any) -> () shape so the call-checker
                    // accepts arbitrary handler lambdas.
                    variadic: Some(Box::new(shingetsu::LuaType::Any)),
                    returns: Vec::new(),
                    is_method: false,
                    inferred_unannotated: true,
                    deprecated: None,
                    must_use: None,
                })),
            ),
        ],
        variadic: None,
        returns: Vec::new(),
        is_method: false,
        inferred_unannotated: false,
        deprecated: None,
        must_use: None,
    };
    let module_type = shingetsu::LuaType::Table(Box::new(shingetsu::types::TableLuaType {
        fields: vec![shingetsu::types::TableField::new(
            shingetsu::Bytes::from("on"),
            shingetsu::LuaType::Function(Box::new(on_field_type)),
        )],
        indexer: None,
    }));
    env.register_global_type(shingetsu::Bytes::from(module_name), module_type);

    // Mark `<module>.on` as an event-handler registrar so the
    // compile-time type checker recognises
    // `<module>.on('event-name', function(...) ... end)` calls,
    // emits unknown-event-name warnings (with did-you-mean
    // suggestions when applicable), and validates handler lambdas
    // against any typed signatures the host has published via
    // `EventSignature::register`.
    env.declare_event_registrar(format!("{module_name}.on"));
    Ok(())
}

#[cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]
fn install_on_mlua(lua: &mlua::Lua, module_name: &str) -> Result<(), EventError> {
    let on_fn = lua.create_function(
        |lua: &mlua::Lua, (name, func): (String, mlua::Function)| -> mlua::Result<()> {
            let key = format!("{MLUA_KEY_PREFIX}{name}");
            let stored: mlua::Value = lua.named_registry_value(&key)?;
            match stored {
                mlua::Value::Nil => {
                    // First registration: store the bare function.
                    // Subsequent registrations promote to a
                    // sequence-table.  EventSignature::call_mlua
                    // and emit_event already handle both shapes.
                    lua.set_named_registry_value(&key, func)?;
                    Ok(())
                }
                mlua::Value::Function(existing) => {
                    let tbl = lua.create_table()?;
                    tbl.push(existing)?;
                    tbl.push(func)?;
                    lua.set_named_registry_value(&key, tbl)?;
                    Ok(())
                }
                mlua::Value::Table(tbl) => {
                    tbl.push(func)?;
                    Ok(())
                }
                other => Err(mlua::Error::external(format!(
                    "on('{name}', ...): registry slot '{key}' holds a {} (expected nil, function, or table)",
                    other.type_name(),
                ))),
            }
        },
    )?;

    let globals = lua.globals();
    let module: mlua::Value = globals.get(module_name)?;
    let module = match module {
        mlua::Value::Table(t) => t,
        mlua::Value::Nil => {
            let t = lua.create_table()?;
            globals.set(module_name, t.clone())?;
            t
        }
        other => {
            return Err(EventError::Mlua(mlua::Error::external(format!(
                "global '{module_name}' is a {} (expected table or nil)",
                other.type_name(),
            ))));
        }
    };
    module.set("on", on_fn)?;
    Ok(())
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
/// Internal helper: combine `\n`-separated `#[doc = ...]`
/// fragments into one summary string, returning `None` when no
/// fragments were captured.  Used by [`declare_event!`] to fold
/// the per-line `///` rustdoc into a single `Option<&'static str>`
/// at expansion time.
#[doc(hidden)]
#[macro_export]
macro_rules! __event_join_docs {
    () => { ::std::option::Option::None };
    ( $($lit:literal),+ $(,)? ) => {
        ::std::option::Option::Some(
            ::std::concat!( $( ::std::concat!($lit, "\n") ),+ )
        )
    };
}

#[macro_export]
#[cfg(feature = "shingetsu-backend")]
macro_rules! declare_event {
    (
        $(#[doc = $sig_doc:literal])*
        $(#[returns = $ret_doc:literal])?
        $vis:vis static $sym:ident: Single(
            $name:literal
            $(,
                $(#[doc = $param_doc:literal])*
                $param:ident : $param_ty:ty
            )* $(,)?
        ) -> $ret:ty;
    ) => {
        $vis static $sym: ::std::sync::LazyLock<
            $crate::EventSignature<( $($param_ty,)* ), $ret>
        > = ::std::sync::LazyLock::new(|| {
            $crate::EventSignature::new_single_typed(
                $name,
                ::std::vec![
                    $( $crate::EventParam {
                        name: ::std::stringify!($param).to_owned(),
                        doc: $crate::__event_join_docs!( $($param_doc),* ),
                        lua_type: <$param_ty as $crate::shingetsu::LuaTyped>::lua_type(),
                    }, )*
                ],
                <$ret as $crate::shingetsu::LuaTypedMulti>::lua_types(),
                $crate::__event_join_docs!( $($sig_doc),* ),
                ::std::option::Option::None $(.or(::std::option::Option::Some($ret_doc)))?,
            )
        });
    };
    (
        $(#[doc = $sig_doc:literal])*
        $(#[returns = $ret_doc:literal])?
        $vis:vis static $sym:ident: Multiple(
            $name:literal
            $(,
                $(#[doc = $param_doc:literal])*
                $param:ident : $param_ty:ty
            )* $(,)?
        ) -> $ret:ty;
    ) => {
        $vis static $sym: ::std::sync::LazyLock<
            $crate::EventSignature<( $($param_ty,)* ), $ret>
        > = ::std::sync::LazyLock::new(|| {
            $crate::EventSignature::new_multiple_typed(
                $name,
                ::std::vec![
                    $( $crate::EventParam {
                        name: ::std::stringify!($param).to_owned(),
                        doc: $crate::__event_join_docs!( $($param_doc),* ),
                        lua_type: <$param_ty as $crate::shingetsu::LuaTyped>::lua_type(),
                    }, )*
                ],
                <$ret as $crate::shingetsu::LuaTypedMulti>::lua_types(),
                $crate::__event_join_docs!( $($sig_doc),* ),
                ::std::option::Option::None $(.or(::std::option::Option::Some($ret_doc)))?,
            )
        });
    };
}

/// mlua-only fallback when the migration facade is built without
/// the shingetsu backend.  Captures the same doc metadata that
/// the typed variant does (so kumomta-style mlua-only doc-build
/// pipelines work) but skips the shingetsu `LuaType` per param /
/// return-types vector that the compile-time event-handler
/// checker would consume.
#[macro_export]
#[cfg(not(feature = "shingetsu-backend"))]
macro_rules! declare_event {
    (
        $(#[doc = $sig_doc:literal])*
        $(#[returns = $ret_doc:literal])?
        $vis:vis static $sym:ident: Single(
            $name:literal
            $(,
                $(#[doc = $param_doc:literal])*
                $param:ident : $param_ty:ty
            )* $(,)?
        ) -> $ret:ty;
    ) => {
        $vis static $sym: ::std::sync::LazyLock<
            $crate::EventSignature<( $($param_ty,)* ), $ret>
        > = ::std::sync::LazyLock::new(|| {
            $crate::EventSignature::new_single_typed(
                $name,
                ::std::vec![
                    $( $crate::EventParam {
                        name: ::std::stringify!($param).to_owned(),
                        doc: $crate::__event_join_docs!( $($param_doc),* ),
                    }, )*
                ],
                $crate::__event_join_docs!( $($sig_doc),* ),
                ::std::option::Option::None $(.or(::std::option::Option::Some($ret_doc)))?,
            )
        });
    };
    (
        $(#[doc = $sig_doc:literal])*
        $(#[returns = $ret_doc:literal])?
        $vis:vis static $sym:ident: Multiple(
            $name:literal
            $(,
                $(#[doc = $param_doc:literal])*
                $param:ident : $param_ty:ty
            )* $(,)?
        ) -> $ret:ty;
    ) => {
        $vis static $sym: ::std::sync::LazyLock<
            $crate::EventSignature<( $($param_ty,)* ), $ret>
        > = ::std::sync::LazyLock::new(|| {
            $crate::EventSignature::new_multiple_typed(
                $name,
                ::std::vec![
                    $( $crate::EventParam {
                        name: ::std::stringify!($param).to_owned(),
                        doc: $crate::__event_join_docs!( $($param_doc),* ),
                    }, )*
                ],
                $crate::__event_join_docs!( $($sig_doc),* ),
                ::std::option::Option::None $(.or(::std::option::Option::Some($ret_doc)))?,
            )
        });
    };
}
