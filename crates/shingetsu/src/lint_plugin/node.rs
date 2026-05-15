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
use crate::{Bytes, Ud, Value, VmError};
use shingetsu_compiler::lint_ir::{self, Expr, ExprKind, Span};
use shingetsu_compiler::{BuiltInLintId, Diagnostic, LintId, Severity, SourceLocation};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// LintContext: the `ctx` argument every event hands to its handler
// ---------------------------------------------------------------------------

/// Kinds of scoping ancestor a plugin can query with
/// `ctx:enclosing(kind)`.  The variant names map 1:1 to the string
/// vocabulary the plugin sees.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AncestorKind {
    Function,
    Loop,
    Branch,
    Chunk,
    DoBlock,
}

impl AncestorKind {
    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s {
            "function" => Some(Self::Function),
            "loop" => Some(Self::Loop),
            "branch" => Some(Self::Branch),
            "chunk" => Some(Self::Chunk),
            "do_block" => Some(Self::DoBlock),
            _ => None,
        }
    }

    pub(crate) fn valid_kinds() -> &'static str {
        "\"function\", \"loop\", \"branch\", \"chunk\", \"do_block\""
    }
}

/// Shared state for one dispatch session: which plugin's handler
/// is currently firing, the source name diagnostics anchor against,
/// and the collector that gathers every `ctx:warn` / `ctx:error`
/// call.
pub(crate) struct DispatchSession {
    pub plugin_name: Arc<str>,
    pub default_severity: Severity,
    pub source_name: Arc<String>,
    pub diagnostics: Mutex<Vec<Diagnostic>>,
    /// Stack of scoping ancestors maintained by the pre-order walker.
    /// Each entry is (kind, span-of-the-ancestor-node).  Pushed before
    /// recursing into a scope body, popped after.
    pub ancestors: Mutex<Vec<(AncestorKind, Span)>>,
}

impl DispatchSession {
    pub(crate) fn push_ancestor(&self, kind: AncestorKind, span: Span) {
        self.ancestors.lock().push((kind, span));
    }

    pub(crate) fn pop_ancestor(&self) {
        self.ancestors.lock().pop();
    }
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
    /// `default_severity` from `lint.declare`.  The optional
    /// trailing `help` argument fills the diagnostic's `help:`
    /// line -- use it to point users at how to fix the issue.
    #[lua_method]
    fn warn(
        self: Arc<Self>,
        span: Ud<lint_ir::Span>,
        message: Bytes,
        help: Option<Bytes>,
    ) -> Result<(), VmError> {
        self.emit(span, message, help, self.session.default_severity);
        Ok(())
    }

    /// Emit an error anchored at `span`.  Overrides the plugin's
    /// declared default severity for this specific diagnostic.
    #[lua_method]
    fn error(
        self: Arc<Self>,
        span: Ud<lint_ir::Span>,
        message: Bytes,
        help: Option<Bytes>,
    ) -> Result<(), VmError> {
        self.emit(span, message, help, Severity::Error);
        Ok(())
    }

    /// `true` when `span_a` and `span_b` start on the same source
    /// line.  Useful for style lints that care whether two nodes
    /// appear on the same line (e.g. inline `if`-then-else).
    #[lua_method]
    fn is_same_line(self: Arc<Self>, span_a: Ud<lint_ir::Span>, span_b: Ud<lint_ir::Span>) -> bool {
        span_a.start_line == span_b.start_line
    }

    /// The span of the nearest enclosing scope of the given `kind`,
    /// or `nil` if no such ancestor exists in the current traversal
    /// context.
    ///
    /// `kind` must be one of: `"function"`, `"loop"`, `"branch"`,
    /// `"chunk"`, `"do_block"`.  Any other value raises an error.
    ///
    /// The `node` argument is accepted for readability at call sites
    /// but is not used; the enclosing context is determined by the
    /// walker's traversal stack, not the node itself.
    #[lua_method]
    fn enclosing(self: Arc<Self>, _node: Value, kind: Bytes) -> Result<Option<Ud<Span>>, VmError> {
        let kind_str = std::str::from_utf8(kind.as_ref())
            .map_err(|_| super::runtime_error("ctx:enclosing: kind must be a UTF-8 string"))?;
        let target = AncestorKind::from_str(kind_str).ok_or_else(|| {
            super::runtime_error(format!(
                "ctx:enclosing: unknown kind '{kind_str}'; valid kinds are: {}",
                AncestorKind::valid_kinds(),
            ))
        })?;
        let found = self
            .session
            .ancestors
            .lock()
            .iter()
            .rev()
            .find(|(k, _)| *k == target)
            .map(|(_, s)| Ud(Arc::new(*s)));
        Ok(found)
    }

    /// The static value of `expr` if it is a compile-time constant,
    /// otherwise `nil`.  Currently handles string, number, and
    /// boolean literals directly; name references and expressions
    /// involving variables return `nil` (full constant-folding
    /// comes with `ctx.resolve` in a later phase).
    #[lua_method]
    fn constant_value(self: Arc<Self>, expr: Ud<Expr>) -> Value {
        match &expr.kind {
            ExprKind::StringLiteral { value, .. } => Value::string(value.clone()),
            ExprKind::NumberLiteral { value, .. } => Value::Float(*value),
            ExprKind::BoolLiteral(b) => Value::Boolean(*b),
            ExprKind::Nil => Value::Nil,
            _ => Value::Nil,
        }
    }
}

impl LintContext {
    fn emit(
        &self,
        span: Ud<lint_ir::Span>,
        message: Bytes,
        help: Option<Bytes>,
        severity: Severity,
    ) {
        let message = String::from_utf8_lossy(message.as_ref()).into_owned();
        let location = span.0.to_source_location(&self.session.source_name);
        // For the MVP every plugin diagnostic uses the same
        // `project:<plugin>` lint id.  A later turn surfaces
        // per-lint sub-ids if a plugin declares multiple.
        let lint = LintId::Plugin(Arc::clone(&self.session.plugin_name));
        let _ = BuiltInLintId::ArgCount; // keep the import wired
        let _: SourceLocation = location.clone();
        let help = help.map(|b| String::from_utf8_lossy(b.as_ref()).into_owned());
        self.session.diagnostics.lock().push(Diagnostic {
            lint,
            severity,
            location,
            message,
            help,
            primary_label: None,
            secondary_spans: vec![],
        });
    }
}
