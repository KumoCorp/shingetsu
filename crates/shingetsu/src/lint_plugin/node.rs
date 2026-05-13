//! Userdata types exposed to lint plugins.
//!
//! Plugin-side userdata: the shared `LintContext` (`ctx`).
//!
//! Event payload userdata (`MethodCall`, `FunctionCall`, ...) live
//! on their respective IR structs in shingetsu-compiler's
//! `lint_ir` module -- the `Userdata` derive sits on the IR type
//! directly so plugins see the same shape the rest of the
//! compiler does.  Only the `ctx` userdata is plugin-host concern
//! and lives here.

use crate::sync::Mutex;
use crate::{Bytes, Ud, VmError};
use shingetsu_compiler::{lint_ir, BuiltInLintId, Diagnostic, LintId, Severity, SourceLocation};
use std::sync::Arc;

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
