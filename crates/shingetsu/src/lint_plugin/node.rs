//! Userdata types exposed to lint plugins.
//!
//! Per-kind userdata for the event payloads (`MethodCallNode` for
//! now, with `FunctionCallNode` and `AssignNode` to follow when
//! their events are wired) plus the shared `LintContext` `ctx`.
//! The `Span` userdata lives on [`lint_ir::Span`] itself -- the
//! derive is right on the IR struct in shingetsu-compiler, so this
//! module references it directly without a local wrapper.
//!
//! This file deliberately starts narrow.  The MVP exposes the
//! minimum fields the two integration tests need; more fields land
//! as real plugins call for them.

use crate::sync::Mutex;
use crate::{Bytes, Ud, VmError};
use shingetsu_compiler::{lint_ir, BuiltInLintId, Diagnostic, LintId, Severity, SourceLocation};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// MethodCallNode: payload for the "method_call" event
// ---------------------------------------------------------------------------

/// Userdata wrapping a [`lint_ir::ExprKind::MethodCall`].  Exposes
/// the method name, the call's overall span, and the
/// `has_trailing_multret` flag.  Receiver and args are deferred:
/// the integration tests this MVP targets don't need them yet.
#[derive(Clone)]
pub struct MethodCallNode {
    pub(crate) method: Bytes,
    pub(crate) method_span: lint_ir::Span,
    pub(crate) span: lint_ir::Span,
    pub(crate) has_trailing_multret: bool,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "MethodCallNode", index_fallback = "nil")]
impl MethodCallNode {
    /// Discriminant tag, useful to write plugins that handle
    /// multiple event kinds with a shared `ctx.walk` callback.
    #[lua_field]
    fn kind(&self) -> Bytes {
        Bytes::from(&b"method_call"[..])
    }

    /// The method name as written at the call site.
    #[lua_field]
    fn method(&self) -> Bytes {
        self.method.clone()
    }

    /// Span covering just the method-name token.  Use this to
    /// anchor a diagnostic on the method itself rather than the
    /// whole call expression.
    #[lua_field]
    fn method_span(&self) -> Ud<lint_ir::Span> {
        Ud(Arc::new(self.method_span))
    }

    /// Span covering the entire call expression.
    #[lua_field]
    fn span(&self) -> Ud<lint_ir::Span> {
        Ud(Arc::new(self.span))
    }

    /// `true` when the last argument is itself a call or `...`,
    /// so the runtime will pass through its full multi-value
    /// result rather than truncating to one value.
    #[lua_field]
    fn has_trailing_multret(&self) -> bool {
        self.has_trailing_multret
    }
}

// ---------------------------------------------------------------------------
// LintContext: the `ctx` argument every event hands to its handler
// ---------------------------------------------------------------------------

/// Shared state for one dispatch session: which plugin's handler
/// is currently firing, the source name diagnostics anchor against,
/// and the collector that gathers every `ctx:warn` / `ctx:error`
/// call.
pub(crate) struct DispatchSession {
    pub plugin_name: Arc<str>,
    pub default_severity: Severity,
    pub source_name: Arc<String>,
    pub diagnostics: Mutex<Vec<Diagnostic>>,
}

/// The `ctx` userdata.  Holds an `Arc` of the session so every
/// callback in the same dispatch sees the same diagnostic
/// collector.
#[derive(Clone)]
pub struct LintContext {
    pub(crate) session: Arc<DispatchSession>,
}

#[shingetsu_derive::userdata(crate = "crate", rename = "LintContext", index_fallback = "nil")]
impl LintContext {
    /// Emit a warning anchored at `span`.  The diagnostic is tagged
    /// with the plugin's `project:<name>` lint id and the plugin's
    /// `default_severity` from `lint.declare`.
    #[lua_method]
    fn warn(self: Arc<Self>, span: Ud<lint_ir::Span>, message: Bytes) -> Result<(), VmError> {
        self.emit(span, message, self.session.default_severity);
        Ok(())
    }

    /// Emit an error anchored at `span`.  Overrides the plugin's
    /// declared default severity for this specific diagnostic.
    #[lua_method]
    fn error(self: Arc<Self>, span: Ud<lint_ir::Span>, message: Bytes) -> Result<(), VmError> {
        self.emit(span, message, Severity::Error);
        Ok(())
    }
}

impl LintContext {
    fn emit(&self, span: Ud<lint_ir::Span>, message: Bytes, severity: Severity) {
        let message = String::from_utf8_lossy(message.as_ref()).into_owned();
        let location = span.0.to_source_location(&self.session.source_name);
        // For the MVP every plugin diagnostic uses the same
        // `project:<plugin>` lint id.  A later turn surfaces
        // per-lint sub-ids if a plugin declares multiple.
        let lint = LintId::Plugin(Arc::clone(&self.session.plugin_name));
        let _ = BuiltInLintId::ArgCount; // keep the import wired
        let _: SourceLocation = location.clone();
        self.session.diagnostics.lock().push(Diagnostic {
            lint,
            severity,
            location,
            message,
            help: None,
            primary_label: None,
            secondary_spans: vec![],
        });
    }
}
