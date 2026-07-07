//! Typed event callback registry.
//!
//! Hosts can expose a `name -> handler` registration entry point
//! (e.g. lua-side `host.on(name, fn)`) and dispatch into registered
//! handlers from Rust through a typed [`CallbackSignature<A, R>`].
//! Names may be statically declared via [`crate::declare_event!`] or added
//! dynamically at runtime via [`CallbackRegistry::declare_dynamic`].
//!
//! ## Storage
//!
//! The registry is attached to a [`GlobalEnv`] as an extension; one
//! call to [`callback_registry`] from anywhere with access to the env
//! returns the (lazily initialised) shared instance.  Multiple
//! `GlobalEnv`s, multiple registries — there is no global state.
//!
//! ## Name policies
//!
//! Event names are not a closed set in general — hosts often allow
//! arbitrary user-defined names alongside well-known ones.  The
//! registry's [`NamePolicy`] controls how unknown names are treated:
//!
//! - [`NamePolicy::Closed`] — every name must be statically declared;
//!   misspells are hard errors.
//! - [`NamePolicy::OpenWithSuggestions`] (default) — unknown names are
//!   accepted, but a name close to a known one yields a
//!   [`RegisterOutcome::Novel`] whose `suggestion` field carries a
//!   "did you mean" hint.
//! - [`NamePolicy::Open`] — unknown names are accepted silently.
//!
//! ## User-defined opt-out
//!
//! When registering a handler for a name that the host cannot
//! pre-declare (an n-th-order dependency known only at runtime), use
//! [`CallbackRegistry::register_user_defined`] to skip the suggestion
//! check for that specific call.  The name is added to the dynamic
//! set so subsequent typo registrations still flag close-but-distinct
//! sibling names.

use crate::byte_string::Bytes;
use crate::convert::{FromLuaMulti, IntoLuaMulti};
use crate::diagnostics::render_field_suggestion;
use crate::error::{RuntimeError, VmError};
use crate::function::Function;
use crate::global_env::GlobalEnv;
use crate::task::ReturnSite;
use crate::types::{FunctionLuaType, LuaType, TypedParam};

use crate::sync::Mutex;
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;

/// Per-environment callback registry.  Look up via [`callback_registry`].
pub struct CallbackRegistry {
    inner: Mutex<RegistryInner>,
}

struct RegistryInner {
    policy: NamePolicy,
    /// Names declared at startup, e.g. via `declare_event!`.
    static_names: HashSet<Bytes>,
    /// Names added at runtime via [`CallbackRegistry::declare_dynamic`]
    /// or as a side effect of any registration.
    dynamic_names: HashSet<Bytes>,
    /// Names whose registration permits multiple handlers.
    multi_names: HashSet<Bytes>,
    /// Currently registered handlers.
    handlers: FxHashMap<Bytes, HandlerEntry>,
}

/// Policy for how unknown event names are handled.  See module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamePolicy {
    /// Every event name must be statically declared.  Unknown names
    /// produce errors at registration and lookup time.
    Closed,
    /// Unknown names are accepted; close-but-distinct names yield a
    /// suggestion in the registration outcome.  Default.
    OpenWithSuggestions,
    /// Unknown names are accepted silently.
    Open,
}

impl Default for NamePolicy {
    fn default() -> Self {
        NamePolicy::OpenWithSuggestions
    }
}

/// One handler function plus optional registration metadata.
#[derive(Clone)]
struct HandlerWithSource {
    func: Function,
    source: Option<RegistrationSource>,
}

/// One or many handlers for a given event name.
#[derive(Clone)]
enum HandlerEntry {
    Single(HandlerWithSource),
    Multiple(Vec<HandlerWithSource>),
}

/// Source location at which an event handler was registered.
///
/// Hosts that wrap the registry behind a lua-facing `on(name, fn)`
/// API are expected to populate this from the lua call stack at the
/// time of registration so error messages can point back to the
/// offending source line.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RegistrationSource {
    /// Source name as it appeared in the registering chunk — a file
    /// path, `=stdin`, or whatever lua's debug machinery surfaced.
    pub source: Bytes,
    /// 1-based line number within [`Self::source`].
    pub line: u32,
    /// Optional name of the function that contained the registration
    /// call, when the host can resolve it from the call stack.
    pub function_name: Option<Bytes>,
}

impl std::fmt::Display for RegistrationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use bstr::BStr;
        write!(f, "{}:{}", BStr::new(&self.source), self.line)?;
        if let Some(fn_name) = &self.function_name {
            write!(f, " (in {})", BStr::new(fn_name))?;
        }
        Ok(())
    }
}

/// Snapshot record returned by [`CallbackRegistry::registered_handlers`].
///
/// One entry per event *name*.  Multi-handler events accumulate one
/// element per registration in [`Self::sources`] (in registration
/// order), each `Some(...)` if the host plumbed a source location and
/// `None` if it did not.  Single-handler events always carry exactly
/// one element.
///
/// Hosts use this for validation passes: "does event X have a
/// handler?" answers from the presence of a record, "how many
/// handlers does Y have?" from `sources.len()`, and "where are they
/// registered?" from the individual `Some` values.
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredHandler {
    pub name: Bytes,
    pub allow_multiple: bool,
    pub sources: Vec<Option<RegistrationSource>>,
}

/// Outcome of a [`CallbackRegistry::register`] call.
#[derive(Debug, Clone, PartialEq)]
pub enum RegisterOutcome {
    /// Registration succeeded; the name was already known
    /// (statically or dynamically declared).
    Known,
    /// Registration succeeded; the name was not previously known and
    /// has been added to the dynamic set.  Under
    /// [`NamePolicy::OpenWithSuggestions`], `suggestion` carries a
    /// human-readable "did you mean ...?" hint when a similar name
    /// exists; under [`NamePolicy::Open`] it is empty.
    Novel { suggestion: String },
}

/// Disposition of a [`CallbackSignature::call`] dispatch.
pub struct CallbackDisposition<R> {
    /// True when at least one handler was registered for this name.
    pub handler_was_defined: bool,
    /// Result returned by the (last) handler, if any.  In the multi
    /// case, the first handler that returned a non-empty result wins.
    pub result: Option<R>,
    /// The event name that was looked up.
    pub event_name: Bytes,
}

impl<R> CallbackDisposition<R> {
    /// Require a value, error otherwise.
    pub fn require_value(self) -> Result<R, VmError> {
        if !self.handler_was_defined {
            return Err(host_err(
                "callback",
                format!(
                    "no event handler is defined for '{}'",
                    bstr::BStr::new(self.event_name.as_ref())
                ),
            ));
        }
        self.result.ok_or_else(|| {
            host_err(
                "callback",
                format!(
                    "the event handler for '{}' did not return a value",
                    bstr::BStr::new(self.event_name.as_ref())
                ),
            )
        })
    }
}

impl<R: Default> CallbackDisposition<R> {
    /// Unwrap the result, falling back to `R::default()` when the
    /// handler was undefined or returned no value.
    pub fn or_default(self) -> R {
        self.result.unwrap_or_default()
    }
}

impl CallbackRegistry {
    /// Create a registry with the default [`NamePolicy::OpenWithSuggestions`].
    pub fn new() -> Self {
        Self::with_policy(NamePolicy::default())
    }

    pub fn with_policy(policy: NamePolicy) -> Self {
        Self {
            inner: Mutex::new(RegistryInner {
                policy,
                static_names: HashSet::new(),
                dynamic_names: HashSet::new(),
                multi_names: HashSet::new(),
                handlers: FxHashMap::default(),
            }),
        }
    }

    pub fn policy(&self) -> NamePolicy {
        self.inner.lock().policy
    }

    pub fn set_policy(&self, policy: NamePolicy) {
        self.inner.lock().policy = policy;
    }

    /// Declare a statically-known event name (typically via
    /// [`CallbackSignature::register`] driven by [`crate::declare_event!`]).
    ///
    /// Idempotent; calling multiple times for the same name is fine.
    pub fn declare_static(&self, name: impl Into<Bytes>, allow_multiple: bool) {
        let name = name.into();
        let mut inner = self.inner.lock();
        inner.static_names.insert(name.clone());
        if allow_multiple {
            inner.multi_names.insert(name);
        }
    }

    /// Add a name to the dynamic set without registering a handler.
    /// Used by hosts that learn about expected event names from
    /// configuration before the script that registers handlers runs.
    pub fn declare_dynamic(&self, name: impl Into<Bytes>) {
        self.inner.lock().dynamic_names.insert(name.into());
    }

    /// Register a handler.  Honors the active [`NamePolicy`] when the
    /// name is unknown.
    pub fn register(
        &self,
        name: impl Into<Bytes>,
        func: Function,
    ) -> Result<RegisterOutcome, VmError> {
        self.register_with_suggestion(name.into(), func, false, None)
    }

    /// Register a handler with a captured source location.  Hosts
    /// wrapping a lua `on(...)` API should call this with location
    /// info derived from the calling chunk so duplicate-registration
    /// errors can name the offending source line.
    pub fn register_with_source(
        &self,
        name: impl Into<Bytes>,
        func: Function,
        source: RegistrationSource,
    ) -> Result<RegisterOutcome, VmError> {
        self.register_with_suggestion(name.into(), func, false, Some(source))
    }

    /// Register a handler asserting the name is intentionally novel
    /// (skips the suggestion check for this call).  The name is still
    /// added to the dynamic set so future typo registrations against
    /// the same name *do* trigger suggestions.
    pub fn register_user_defined(
        &self,
        name: impl Into<Bytes>,
        func: Function,
    ) -> Result<(), VmError> {
        self.register_with_suggestion(name.into(), func, true, None)?;
        Ok(())
    }

    /// Like [`Self::register_user_defined`] but with a captured source
    /// location for downstream diagnostics.
    pub fn register_user_defined_with_source(
        &self,
        name: impl Into<Bytes>,
        func: Function,
        source: RegistrationSource,
    ) -> Result<(), VmError> {
        self.register_with_suggestion(name.into(), func, true, Some(source))?;
        Ok(())
    }

    fn register_with_suggestion(
        &self,
        name: Bytes,
        func: Function,
        user_defined: bool,
        source: Option<RegistrationSource>,
    ) -> Result<RegisterOutcome, VmError> {
        let mut inner = self.inner.lock();
        let known = inner.static_names.contains(&name) || inner.dynamic_names.contains(&name);

        let outcome = if known {
            RegisterOutcome::Known
        } else {
            // Unknown name; consult the policy.
            let suggestion = if user_defined {
                String::new()
            } else {
                match inner.policy {
                    NamePolicy::Closed => {
                        let known_names: Vec<&[u8]> = inner
                            .static_names
                            .iter()
                            .chain(inner.dynamic_names.iter())
                            .map(|n| n.as_ref())
                            .collect();
                        let used = String::from_utf8_lossy(name.as_ref());
                        let hint = render_field_suggestion(&used, &known_names);
                        let mut msg = format!(
                            "'{}' is not a recognised event name",
                            bstr::BStr::new(name.as_ref())
                        );
                        if !hint.is_empty() {
                            msg.push_str(". ");
                            msg.push_str(&hint);
                        }
                        return Err(host_err("callback", msg));
                    }
                    NamePolicy::OpenWithSuggestions => {
                        let known_names: Vec<&[u8]> = inner
                            .static_names
                            .iter()
                            .chain(inner.dynamic_names.iter())
                            .map(|n| n.as_ref())
                            .collect();
                        let used = String::from_utf8_lossy(name.as_ref());
                        render_field_suggestion(&used, &known_names)
                    }
                    NamePolicy::Open => String::new(),
                }
            };
            inner.dynamic_names.insert(name.clone());
            RegisterOutcome::Novel { suggestion }
        };

        // Insert the handler.  The shape (single vs multiple) follows
        // the declaration recorded in `multi_names`; both mismatch
        // directions surface as end-user-readable errors that describe
        // the constraint without exposing internal-variant names.
        let multi = inner.multi_names.contains(&name);
        match inner.handlers.entry(name.clone()) {
            std::collections::hash_map::Entry::Occupied(mut e) => match e.get_mut() {
                HandlerEntry::Single(_) if multi => {
                    // The name was registered earlier as single, then
                    // later declared as supporting multiple handlers.
                    return Err(host_err(
                        "callback",
                        format!(
                            "event '{}' allows multiple event handlers, \
                             but a previously registered handler is recorded \
                             under an incompatible single-handler shape",
                            bstr::BStr::new(name.as_ref())
                        ),
                    ));
                }
                HandlerEntry::Single(existing) => {
                    let mut msg = format!(
                        "event '{}' allows only a single event handler to be defined; \
                         another handler has already been registered for this name",
                        bstr::BStr::new(name.as_ref())
                    );
                    if let Some(prev) = &existing.source {
                        msg.push_str(&format!(" at {prev}"));
                    }
                    return Err(host_err("callback", msg));
                }
                HandlerEntry::Multiple(_) if !multi => {
                    // Symmetric inverse of the case above.
                    return Err(host_err(
                        "callback",
                        format!(
                            "event '{}' allows only a single event handler to be defined, \
                             but multiple handlers are recorded for this name",
                            bstr::BStr::new(name.as_ref())
                        ),
                    ));
                }
                HandlerEntry::Multiple(v) => v.push(HandlerWithSource { func, source }),
            },
            std::collections::hash_map::Entry::Vacant(e) => {
                let entry = HandlerWithSource { func, source };
                if multi {
                    e.insert(HandlerEntry::Multiple(vec![entry]));
                } else {
                    e.insert(HandlerEntry::Single(entry));
                }
            }
        }

        Ok(outcome)
    }

    /// Snapshot the registered handler(s) for an event name.
    fn lookup(&self, name: &Bytes) -> Option<HandlerEntry> {
        self.inner.lock().handlers.get(name).cloned()
    }

    /// Snapshot the [`Function`] handlers registered for `name` in
    /// registration order.  Returns an empty `Vec` when the name is
    /// unregistered.  Single-handler events return a one-element
    /// vector; multi-handler events return all handlers.
    ///
    /// Used by hosts that need custom dispatch semantics on top of
    /// the registry (e.g. wezterm's broadcast "all run, first false
    /// short-circuits" through the migration facade's `emit_event`).
    /// Built-in dispatch (single-result / first-non-empty) is
    /// already handled by [`CallbackSignature::call`]; reach for
    /// this only when those don't fit.
    pub fn handlers(&self, name: &[u8]) -> Vec<Function> {
        let bytes = Bytes::from(name);
        match self.inner.lock().handlers.get(&bytes) {
            None => Vec::new(),
            Some(HandlerEntry::Single(h)) => vec![h.func.clone()],
            Some(HandlerEntry::Multiple(hs)) => hs.iter().map(|h| h.func.clone()).collect(),
        }
    }

    /// Snapshot all registered handlers, one entry per event name,
    /// sorted by name.  See [`RegisteredHandler`] for the shape.
    ///
    /// Hosts drive validation passes off this list: "every required
    /// event has at least one handler", "no two registered names are
    /// typos of each other", "every multi-handler event has at most
    /// N handlers", and so on.
    pub fn registered_handlers(&self) -> Vec<RegisteredHandler> {
        let inner = self.inner.lock();
        let mut entries: Vec<RegisteredHandler> = Vec::new();
        let mut keys: Vec<&Bytes> = inner.handlers.keys().collect();
        keys.sort();
        for name in keys {
            let multi = inner.multi_names.contains(name);
            let sources = match inner.handlers.get(name).expect("key from same map") {
                HandlerEntry::Single(h) => vec![h.source.clone()],
                HandlerEntry::Multiple(v) => v.iter().map(|h| h.source.clone()).collect(),
            };
            entries.push(RegisteredHandler {
                name: name.clone(),
                allow_multiple: multi,
                sources,
            });
        }
        entries
    }

    /// Snapshot the static + dynamic name sets.  Test/inspection helper.
    pub fn known_names(&self) -> Vec<Bytes> {
        let inner = self.inner.lock();
        let mut names: Vec<Bytes> = inner
            .static_names
            .iter()
            .chain(inner.dynamic_names.iter())
            .cloned()
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

impl Default for CallbackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Look up the [`CallbackRegistry`] attached to `env`, initialising
/// it lazily with [`NamePolicy::OpenWithSuggestions`] on first access.
pub fn callback_registry(env: &GlobalEnv) -> Arc<CallbackRegistry> {
    env.extension_or_init::<CallbackRegistry, _>(CallbackRegistry::new)
}

// ---------------------------------------------------------------------------
// Typed signature
// ---------------------------------------------------------------------------

/// Per-parameter info captured at signature declaration time.  Used
/// by the compile-time event-handler checker to validate user-written
/// handler lambdas against the declared signature, and by docgen to
/// surface the rustdoc summary captured on the `declare_event!` site.
#[derive(Debug, Clone, PartialEq)]
pub struct CallbackParam {
    pub name: Bytes,
    pub lua_type: LuaType,
    /// Rustdoc captured on the parameter inside the `declare_event!`
    /// invocation, joined with newlines when multiple `///` lines
    /// were present.  `None` when no `#[doc = ...]` attributes were
    /// applied to this parameter.
    pub doc: Option<String>,
}

/// Typed callback signature.  Decouples the rust call site from the
/// untyped `Function` registry by encoding the argument tuple `A` and
/// return type `R`.
///
/// Construct via [`crate::declare_event!`] or one of the `new` / `new_typed`
/// constructors.  The typed variants additionally capture parameter
/// names and types so the compiler can validate handler lambdas at
/// load time — see [`Self::param_info`] / [`Self::return_types`].
pub struct CallbackSignature<A, R> {
    name: Bytes,
    allow_multiple: bool,
    /// Per-parameter metadata, populated by [`crate::declare_event!`] and
    /// accessible to compile-time tooling.  Empty when the signature
    /// was constructed via [`Self::new`] / [`Self::new_multiple`]
    /// (which intentionally do not require typed param info).
    params: Vec<CallbackParam>,
    /// Return type metadata, expressed as a (possibly empty) list of
    /// types matching `LuaTypedMulti`.  `None` when the signature was
    /// constructed without typed param info.  An empty `Some(vec)`
    /// represents a unit / no-return signature.
    return_types: Option<Vec<LuaType>>,
    /// Rustdoc captured on the `static` declaration inside
    /// [`crate::declare_event!`].  Surfaced by docgen as the event-level
    /// summary; `None` for hand-rolled signatures or for callers
    /// that did not attach a `///` block.
    event_doc: Option<String>,
    /// Return-value rustdoc captured via the `#[returns = "..."]`
    /// attribute inside [`crate::declare_event!`].  `None` when no such
    /// attribute was supplied.
    return_doc: Option<String>,
    _marker: PhantomData<fn(A) -> R>,
}

impl<A, R> CallbackSignature<A, R> {
    /// A signature where only one handler may be registered, without
    /// captured parameter info.  Suitable for hand-rolled signatures
    /// where compile-time handler checking is not required.
    pub fn new(name: impl Into<Bytes>) -> Self {
        Self {
            name: name.into(),
            allow_multiple: false,
            params: Vec::new(),
            return_types: None,
            event_doc: None,
            return_doc: None,
            _marker: PhantomData,
        }
    }

    /// A signature where multiple handlers may be registered.  When
    /// invoked, handlers run in registration order until one returns
    /// a non-empty result.
    pub fn new_multiple(name: impl Into<Bytes>) -> Self {
        Self {
            name: name.into(),
            allow_multiple: true,
            params: Vec::new(),
            return_types: None,
            event_doc: None,
            return_doc: None,
            _marker: PhantomData,
        }
    }

    /// Construct a single-handler signature with explicit param + return
    /// metadata.  Typically called by the [`crate::declare_event!`] macro
    /// expansion.  `return_types` is the multi-return shape from
    /// `LuaTypedMulti::lua_types()` -- use an empty vec for `()`.
    /// `event_doc` and `return_doc` carry the rustdoc captured on
    /// the `static` declaration and via the `#[returns = ...]`
    /// attribute respectively; pass `None` when not capturing docs.
    pub fn new_typed(
        name: impl Into<Bytes>,
        params: Vec<CallbackParam>,
        return_types: Vec<LuaType>,
        event_doc: Option<String>,
        return_doc: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            allow_multiple: false,
            params,
            return_types: Some(return_types),
            event_doc,
            return_doc,
            _marker: PhantomData,
        }
    }

    /// Construct a multi-handler signature with explicit param + return
    /// metadata.  See [`Self::new_typed`] for the doc-argument shape.
    pub fn new_multiple_typed(
        name: impl Into<Bytes>,
        params: Vec<CallbackParam>,
        return_types: Vec<LuaType>,
        event_doc: Option<String>,
        return_doc: Option<String>,
    ) -> Self {
        Self {
            name: name.into(),
            allow_multiple: true,
            params,
            return_types: Some(return_types),
            event_doc,
            return_doc,
            _marker: PhantomData,
        }
    }

    pub fn name(&self) -> &[u8] {
        self.name.as_ref()
    }

    pub fn allow_multiple(&self) -> bool {
        self.allow_multiple
    }

    /// Per-parameter metadata, in declaration order.  Empty when the
    /// signature was constructed without typed info.
    pub fn param_info(&self) -> &[CallbackParam] {
        &self.params
    }

    /// Declared return types, if known.  An empty slice means the
    /// signature returns nothing (unit / `()`).
    pub fn return_types(&self) -> Option<&[LuaType]> {
        self.return_types.as_deref()
    }

    /// Event-level rustdoc captured on the `static` declaration
    /// inside [`crate::declare_event!`].
    pub fn event_doc(&self) -> Option<&str> {
        self.event_doc.as_deref()
    }

    /// Return-value rustdoc captured via the `#[returns = ...]`
    /// attribute inside [`crate::declare_event!`].
    pub fn return_doc(&self) -> Option<&str> {
        self.return_doc.as_deref()
    }

    /// Build the [`FunctionLuaType`] a handler lambda must satisfy.
    /// Returns `None` for signatures constructed without typed info.
    pub fn handler_function_type(&self) -> Option<FunctionLuaType> {
        let returns = self.return_types.clone()?;
        Some(FunctionLuaType {
            type_params: Vec::new(),
            params: self
                .params
                .iter()
                .map(|p| {
                    TypedParam::new_with_doc(
                        Some(p.name.clone()),
                        p.lua_type.clone(),
                        p.doc.clone(),
                    )
                })
                .collect(),
            variadic: None,
            returns,
            is_method: false,
            inferred_unannotated: false,
            deprecated: None,
            must_use: None,
        })
    }

    /// Register this signature on the env's registry.  Idempotent.
    /// Hosts typically call this for every `declare_event!`-defined
    /// signature at env construction time.
    pub fn register(&self, env: &GlobalEnv) {
        callback_registry(env).declare_static(self.name.clone(), self.allow_multiple);
    }

    /// Publish this signature into a [`crate::types::GlobalTypeMap`]
    /// so the compile-time event-handler checker can validate user-
    /// written handler lambdas against the declared shape.
    ///
    /// No-op for signatures constructed without typed param info
    /// (via [`Self::new`] / [`Self::new_multiple`]) — those don't
    /// have a typed shape to publish.
    pub fn register_compile_type(&self, types: &mut crate::types::GlobalTypeMap) {
        if let Some(ft) = self.handler_function_type() {
            types.declare_event_handler_signature(
                self.name.clone(),
                crate::types::EventHandlerSignature {
                    function_type: ft,
                    doc: self.event_doc.clone(),
                    return_doc: self.return_doc.clone(),
                },
            );
        }
    }
}

/// Turn a return-value conversion failure into a `RuntimeError`,
/// anchoring it at the handler's `return` when the task captured one.
fn convert_error(err: VmError, site: &Arc<Mutex<Option<ReturnSite>>>) -> RuntimeError {
    RuntimeError::from_return_conversion(err, site.lock().take())
}

impl<A, R> CallbackSignature<A, R>
where
    A: IntoLuaMulti + Clone,
    R: FromLuaMulti,
{
    /// Invoke the registered handler(s) for this event.
    ///
    /// For single signatures, returns the handler's first result if
    /// defined.  For multi signatures, runs handlers in registration
    /// order and returns the first non-empty result; if every handler
    /// returns nothing the disposition reports `result = None`.
    ///
    /// A handler that raises surfaces as the full [`RuntimeError`],
    /// preserving the call stack and source context so callers can
    /// render an annotated diagnostic.  A return-value type mismatch
    /// is anchored at the handler's `return`.
    pub async fn call(
        &self,
        env: &GlobalEnv,
        args: A,
    ) -> Result<CallbackDisposition<R>, RuntimeError> {
        use crate::task::Task;
        let registry = callback_registry(env);
        let entry = registry.lookup(&self.name);
        let event_name = self.name.clone();

        match entry {
            None => Ok(CallbackDisposition {
                handler_was_defined: false,
                result: None,
                event_name,
            }),
            Some(HandlerEntry::Single(h)) => {
                let argv = args.into_lua_multi();
                let site = Arc::new(Mutex::new(None));
                let mut task = Task::new(env.clone(), h.func, argv);
                task.set_capture_return_site(Arc::clone(&site));
                let raw = task.await?;
                let r = R::from_lua_multi(raw, env).map_err(|vm| convert_error(vm, &site))?;
                Ok(CallbackDisposition {
                    handler_was_defined: true,
                    result: Some(r),
                    event_name,
                })
            }
            Some(HandlerEntry::Multiple(handlers)) => {
                for h in handlers {
                    let argv = args.clone().into_lua_multi();
                    let site = Arc::new(Mutex::new(None));
                    let mut task = Task::new(env.clone(), h.func, argv);
                    task.set_capture_return_site(Arc::clone(&site));
                    let raw = task.await?;
                    if !raw.is_empty() {
                        let r =
                            R::from_lua_multi(raw, env).map_err(|vm| convert_error(vm, &site))?;
                        return Ok(CallbackDisposition {
                            handler_was_defined: true,
                            result: Some(r),
                            event_name,
                        });
                    }
                }
                Ok(CallbackDisposition {
                    handler_was_defined: true,
                    result: None,
                    event_name,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// declare_event! macro
// ---------------------------------------------------------------------------

/// Declare a typed callback signature in a static slot.
///
/// Two variants:
///
/// ```ignore
/// declare_event! {
///     pub static GET_CONFIG: Single("get_config", domain: String) -> Config;
/// }
///
/// declare_event! {
///     pub static ON_REQUEST: Multiple("on_request", req: Request) -> bool;
/// }
/// ```
///
/// Expands to a `LazyLock<CallbackSignature<A, R>>` where the parameter
/// list `(name: Type, ...)` becomes the tuple `A` and the return type
/// becomes `R`.  The parameter names are not yet propagated to the
/// type checker (compile-time validation of registered handler
/// lambdas).
///
/// Hosts typically iterate registered signatures and call
/// `signature.register(&env)` once at env construction so the
/// registry knows which names are statically declared.
///
/// Parameter names and per-param `LuaType`s are captured into the
/// expanded signature so the compile-time event-handler checker can
/// validate user-written lambdas against them.  Rustdoc is captured
/// at three positions: on the `static` itself (event-level summary),
/// on each parameter (per-parameter summary), and via an optional
/// `#[returns = "..."]` attribute (return-value description).
#[macro_export]
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
        $(#[doc = $sig_doc])*
        $vis static $sym: ::std::sync::LazyLock<
            $crate::callback::CallbackSignature<( $($param_ty,)* ), $ret>
        > = ::std::sync::LazyLock::new(|| {
            $crate::callback::CallbackSignature::new_typed(
                $name,
                ::std::vec![
                    $( $crate::callback::CallbackParam {
                        name: $crate::Bytes::from(::std::stringify!($param)),
                        lua_type: <$param_ty as $crate::LuaTyped>::lua_type(),
                        doc: $crate::__event_join_docs!( $($param_doc),* ),
                    }, )*
                ],
                <$ret as $crate::convert::LuaTypedMulti>::lua_types(),
                $crate::__event_join_docs!( $($sig_doc),* ),
                ::std::option::Option::None
                    $(.or(::std::option::Option::Some(($ret_doc).to_owned())))?,
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
        $(#[doc = $sig_doc])*
        $vis static $sym: ::std::sync::LazyLock<
            $crate::callback::CallbackSignature<( $($param_ty,)* ), $ret>
        > = ::std::sync::LazyLock::new(|| {
            $crate::callback::CallbackSignature::new_multiple_typed(
                $name,
                ::std::vec![
                    $( $crate::callback::CallbackParam {
                        name: $crate::Bytes::from(::std::stringify!($param)),
                        lua_type: <$param_ty as $crate::LuaTyped>::lua_type(),
                        doc: $crate::__event_join_docs!( $($param_doc),* ),
                    }, )*
                ],
                <$ret as $crate::convert::LuaTypedMulti>::lua_types(),
                $crate::__event_join_docs!( $($sig_doc),* ),
                ::std::option::Option::None
                    $(.or(::std::option::Option::Some(($ret_doc).to_owned())))?,
            )
        });
    };
}

/// Internal helper: combine `\n`-separated `#[doc = ...]` fragments
/// into a single `Option<String>` summary.  `None` when no fragments
/// were supplied.  Used by [`crate::declare_event!`].
#[doc(hidden)]
#[macro_export]
macro_rules! __event_join_docs {
    () => { ::std::option::Option::None };
    ( $($lit:literal),+ $(,)? ) => {
        ::std::option::Option::Some(
            ::std::concat!( $( ::std::concat!($lit, "\n") ),+ ).to_owned()
        )
    };
}

fn host_err(name: &'static str, msg: String) -> VmError {
    VmError::HostError {
        name: name.to_owned(),
        source: msg.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_func(name: &'static str, retval: i64) -> Function {
        Function::wrap(name, move || -> Result<i64, VmError> { Ok(retval) })
    }

    #[test]
    fn registry_stored_per_env() {
        let env = GlobalEnv::new();
        let r1 = callback_registry(&env);
        let r2 = callback_registry(&env);
        assert!(Arc::ptr_eq(&r1, &r2));
    }

    #[test]
    fn closed_policy_rejects_unknown_name() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.set_policy(NamePolicy::Closed);
        // No declared names — nothing to suggest, the message is
        // the bare "not recognised" form.
        let err = reg
            .register("ghost", make_func("h", 0))
            .expect_err("should reject");
        k9::assert_equal!(
            format!("{err}"),
            "error in 'callback': 'ghost' is not a recognised event name"
        );
    }

    #[test]
    fn closed_policy_includes_did_you_mean_when_close_match_exists() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("on_request", false);
        reg.set_policy(NamePolicy::Closed);
        let err = reg
            .register("on_requst", make_func("h", 0))
            .expect_err("should reject");
        k9::assert_equal!(
            format!("{err}"),
            "error in 'callback': 'on_requst' is not a recognised event name. \
             Did you mean `on_request`?"
        );
    }

    #[test]
    fn open_with_suggestions_returns_hint() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("on_request", false);
        let outcome = reg
            .register("on_requst", make_func("h", 0))
            .expect("should accept");
        match outcome {
            RegisterOutcome::Novel { suggestion } => {
                k9::assert_equal!(suggestion, "Did you mean `on_request`?");
            }
            other => panic!("expected Novel suggestion, got {other:?}"),
        }
    }

    #[test]
    fn user_defined_extends_dynamic_set() {
        // A user-defined registration succeeds without a suggestion
        // for itself, and seeds the dynamic name set so future typo
        // registrations against the same neighborhood are flagged.
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("on_request", false);
        reg.register_user_defined("on_requst", make_func("h", 0))
            .expect("should accept");

        // The typo'd name is now in the registry's known set.
        let names: Vec<Vec<u8>> = reg
            .known_names()
            .into_iter()
            .map(|b| b.as_ref().to_vec())
            .collect();
        k9::assert_equal!(names, vec![b"on_request".to_vec(), b"on_requst".to_vec()]);
    }

    #[test]
    fn single_signature_double_registration_errors() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("ev", false);
        reg.register("ev", make_func("h1", 0)).expect("first");
        let err = reg.register("ev", make_func("h2", 0)).expect_err("dup");
        k9::assert_equal!(
            format!("{err}"),
            "error in 'callback': event 'ev' allows only a single event handler to be \
             defined; another handler has already been registered for this name"
        );
    }

    #[test]
    fn multi_signature_collects_handlers_in_order() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("multi", true);
        reg.register("multi", make_func("a", 1)).expect("a");
        reg.register("multi", make_func("b", 2)).expect("b");
        // Inspect via the public introspection API rather than the
        // private lookup, so we can compare the full record shape
        // (Function values themselves are opaque to PartialEq).
        k9::assert_equal!(
            reg.registered_handlers(),
            vec![RegisteredHandler {
                name: Bytes::from("multi"),
                allow_multiple: true,
                sources: vec![None, None],
            }]
        );
    }

    #[test]
    fn declare_event_captures_param_names_and_types() {
        declare_event! {
            pub static SIG: Single("deliver", domain: String, port: i64) -> bool;
        }
        k9::assert_equal!(
            SIG.param_info(),
            &[
                CallbackParam {
                    name: Bytes::from("domain"),
                    lua_type: LuaType::String,
                    doc: None,
                },
                CallbackParam {
                    name: Bytes::from("port"),
                    lua_type: LuaType::Number,
                    doc: None,
                },
            ]
        );
        k9::assert_equal!(
            SIG.return_types().map(|s| s.to_vec()),
            Some(vec![LuaType::Boolean])
        );
    }

    #[test]
    fn declare_event_handler_function_type_for_unit_return() {
        declare_event! {
            pub static SIG: Single("check", msg: String) -> ();
        }
        k9::assert_equal!(
            SIG.handler_function_type(),
            Some(FunctionLuaType {
                type_params: vec![],
                params: vec![TypedParam::new(Some("msg"), LuaType::String)],
                variadic: None,
                returns: vec![],
                is_method: false,
                inferred_unannotated: false,
                deprecated: None,
                must_use: None,
            })
        );
    }

    #[test]
    fn untyped_signature_has_no_handler_type() {
        let sig: CallbackSignature<(i64,), bool> = CallbackSignature::new("x");
        k9::assert_equal!(sig.handler_function_type(), None);
        k9::assert_equal!(sig.param_info(), &[]);
        k9::assert_equal!(sig.return_types().map(|s| s.to_vec()), None);
    }

    #[test]
    fn declare_event_multiple_captures_typing_too() {
        declare_event! {
            pub static M: Multiple("m", n: i64) -> bool;
        }
        k9::assert_equal!(M.allow_multiple(), true);
        k9::assert_equal!(
            M.param_info(),
            &[CallbackParam {
                name: Bytes::from("n"),
                lua_type: LuaType::Number,
                doc: None,
            }]
        );
        k9::assert_equal!(
            M.return_types().map(|s| s.to_vec()),
            Some(vec![LuaType::Boolean])
        );
    }

    #[test]
    fn declare_event_captures_rustdoc_metadata() {
        declare_event! {
            /// Fired when a queue is reset.
            /// Handlers may decide whether the reset proceeds.
            #[returns = "`true` to allow the reset; `false` to veto."]
            pub static SIG: Single(
                "on_reset",
                /// Identifier of the queue being reset.
                queue: String,
                /// Whether this reset was triggered manually.
                manual: bool,
            ) -> bool;
        }
        k9::assert_equal!(
            SIG.event_doc(),
            Some(
                " Fired when a queue is reset.\n Handlers may decide whether the reset proceeds.\n"
            )
        );
        k9::assert_equal!(
            SIG.return_doc(),
            Some("`true` to allow the reset; `false` to veto.")
        );
        k9::assert_equal!(
            SIG.param_info(),
            &[
                CallbackParam {
                    name: Bytes::from("queue"),
                    lua_type: LuaType::String,
                    doc: Some(" Identifier of the queue being reset.\n".to_owned()),
                },
                CallbackParam {
                    name: Bytes::from("manual"),
                    lua_type: LuaType::Boolean,
                    doc: Some(" Whether this reset was triggered manually.\n".to_owned()),
                },
            ]
        );
        // The same docs flow through to the FunctionLuaType the
        // type checker validates handler lambdas against.
        let ft = SIG.handler_function_type().expect("typed sig");
        k9::assert_equal!(
            ft.params,
            vec![
                TypedParam::new_with_doc(
                    Some("queue"),
                    LuaType::String,
                    Some(" Identifier of the queue being reset.\n".to_owned()),
                ),
                TypedParam::new_with_doc(
                    Some("manual"),
                    LuaType::Boolean,
                    Some(" Whether this reset was triggered manually.\n".to_owned()),
                ),
            ]
        );
    }

    #[test]
    fn register_compile_type_publishes_doc_metadata() {
        declare_event! {
            /// Looks up tenant config.
            #[returns = "The resolved config struct."]
            pub static GET_CONFIG: Single(
                "get_config",
                /// Tenant identifier.
                tenant: String,
            ) -> String;
        }
        let mut tm = crate::types::GlobalTypeMap::default();
        GET_CONFIG.register_compile_type(&mut tm);
        let sig = tm
            .event_handler_signature(b"get_config")
            .expect("event registered");
        k9::assert_equal!(sig.doc.as_deref(), Some(" Looks up tenant config.\n"));
        k9::assert_equal!(
            sig.return_doc.as_deref(),
            Some("The resolved config struct.")
        );
        k9::assert_equal!(
            sig.function_type.params[0].doc.as_deref(),
            Some(" Tenant identifier.\n")
        );
    }

    #[tokio::test]
    async fn dispatch_via_typed_signature() {
        declare_event! {
            pub static GREET: Single("greet", n: i64) -> i64;
        }

        let env = GlobalEnv::new();
        GREET.register(&env);

        let reg = callback_registry(&env);
        reg.register(
            "greet",
            Function::wrap("greet_handler", |n: i64| -> Result<i64, VmError> {
                Ok(n * 2)
            }),
        )
        .expect("register");

        let disp = GREET.call(&env, (21i64,)).await.expect("call");
        k9::assert_equal!(
            (disp.handler_was_defined, disp.result, disp.event_name),
            (true, Some(42), Bytes::from("greet"))
        );
    }

    #[tokio::test]
    async fn dispatch_undefined_returns_no_handler() {
        declare_event! {
            pub static MISSING: Single("missing", n: i64) -> i64;
        }
        let env = GlobalEnv::new();
        MISSING.register(&env);
        let disp = MISSING.call(&env, (1i64,)).await.expect("call");
        k9::assert_equal!(
            (disp.handler_was_defined, disp.result, disp.event_name),
            (false, None, Bytes::from("missing"))
        );
    }

    #[tokio::test]
    async fn multi_first_non_empty_wins() {
        declare_event! {
            pub static M: Multiple("m", n: i64) -> i64;
        }
        let env = GlobalEnv::new();
        M.register(&env);

        let reg = callback_registry(&env);
        // First handler returns nothing; second returns the value.
        reg.register(
            "m",
            Function::wrap("h1", |_n: i64| -> Result<(), VmError> { Ok(()) }),
        )
        .expect("h1");
        reg.register(
            "m",
            Function::wrap("h2", |n: i64| -> Result<i64, VmError> { Ok(n + 100) }),
        )
        .expect("h2");

        let disp = M.call(&env, (5i64,)).await.expect("call");
        k9::assert_equal!(
            (disp.handler_was_defined, disp.result, disp.event_name),
            (true, Some(105), Bytes::from("m"))
        );
    }

    #[test]
    fn known_names_includes_static_and_dynamic() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("a", false);
        reg.declare_dynamic("b");
        let names = reg.known_names();
        k9::assert_equal!(
            names.iter().map(|b| b.as_ref()).collect::<Vec<_>>(),
            vec![&b"a"[..], &b"b"[..]]
        );
    }

    fn src(file: &str, line: u32) -> RegistrationSource {
        RegistrationSource {
            source: Bytes::from(file),
            line,
            function_name: None,
        }
    }

    #[test]
    fn duplicate_single_registration_names_previous_source() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("ev", false);
        reg.register_with_source("ev", make_func("h1", 0), src("config.lua", 17))
            .expect("first");
        let err = reg
            .register_with_source("ev", make_func("h2", 0), src("config.lua", 42))
            .expect_err("dup");
        k9::assert_equal!(
            format!("{err}"),
            "error in 'callback': event 'ev' allows only a single event handler to be \
             defined; another handler has already been registered for this name \
             at config.lua:17"
        );
    }

    #[test]
    fn duplicate_single_without_source_omits_location_clause() {
        // When the host did not capture a source location for the
        // first registration, the duplicate-error message keeps its
        // original shape — no trailing "at ..." fragment.
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("ev", false);
        reg.register("ev", make_func("h1", 0)).expect("first");
        let err = reg.register("ev", make_func("h2", 0)).expect_err("dup");
        k9::assert_equal!(
            format!("{err}"),
            "error in 'callback': event 'ev' allows only a single event handler to be \
             defined; another handler has already been registered for this name"
        );
    }

    #[test]
    fn registration_source_display_with_function_name() {
        let s = RegistrationSource {
            source: Bytes::from("config.lua"),
            line: 9,
            function_name: Some(Bytes::from("setup")),
        };
        k9::assert_equal!(format!("{s}"), "config.lua:9 (in setup)");
    }

    #[test]
    fn registered_handlers_groups_by_name_with_per_registration_sources() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("single", false);
        reg.declare_static("multi", true);
        reg.register_with_source("single", make_func("a", 0), src("a.lua", 1))
            .expect("a");
        reg.register_with_source("multi", make_func("b", 0), src("b.lua", 2))
            .expect("b");
        reg.register_with_source("multi", make_func("c", 0), src("c.lua", 3))
            .expect("c");

        let recorded = reg.registered_handlers();
        k9::assert_equal!(
            recorded,
            vec![
                RegisteredHandler {
                    name: Bytes::from("multi"),
                    allow_multiple: true,
                    sources: vec![Some(src("b.lua", 2)), Some(src("c.lua", 3))],
                },
                RegisteredHandler {
                    name: Bytes::from("single"),
                    allow_multiple: false,
                    sources: vec![Some(src("a.lua", 1))],
                },
            ]
        );
    }

    #[test]
    fn registered_handlers_records_mixed_some_and_none_sources() {
        // A multi-handler event where the host plumbed source for one
        // registration but not the other carries the per-call distinction.
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("m", true);
        reg.register_with_source("m", make_func("a", 0), src("a.lua", 5))
            .expect("a");
        reg.register("m", make_func("b", 0)).expect("b");

        let recorded = reg.registered_handlers();
        k9::assert_equal!(
            recorded,
            vec![RegisteredHandler {
                name: Bytes::from("m"),
                allow_multiple: true,
                sources: vec![Some(src("a.lua", 5)), None],
            }]
        );
    }

    #[test]
    fn registered_handlers_without_source_carries_single_none_slot() {
        let env = GlobalEnv::new();
        let reg = callback_registry(&env);
        reg.declare_static("ev", false);
        reg.register("ev", make_func("h", 0)).expect("register");
        let recorded = reg.registered_handlers();
        k9::assert_equal!(
            recorded,
            vec![RegisteredHandler {
                name: Bytes::from("ev"),
                allow_multiple: false,
                sources: vec![None],
            }]
        );
    }

    #[test]
    fn require_value_errors_when_undefined() {
        let disp: CallbackDisposition<i64> = CallbackDisposition {
            handler_was_defined: false,
            result: None,
            event_name: Bytes::from("x"),
        };
        let err = disp.require_value().expect_err("err");
        k9::assert_equal!(
            format!("{err}"),
            "error in 'callback': no event handler is defined for 'x'"
        );
    }

    #[test]
    fn require_value_errors_when_handler_returned_nothing() {
        let disp: CallbackDisposition<i64> = CallbackDisposition {
            handler_was_defined: true,
            result: None,
            event_name: Bytes::from("x"),
        };
        let err = disp.require_value().expect_err("err");
        k9::assert_equal!(
            format!("{err}"),
            "error in 'callback': the event handler for 'x' did not return a value"
        );
    }

    #[test]
    fn or_default_yields_default_when_empty() {
        let disp: CallbackDisposition<i64> = CallbackDisposition {
            handler_was_defined: true,
            result: None,
            event_name: Bytes::from("x"),
        };
        k9::assert_equal!(disp.or_default(), 0);
    }
}
