use std::sync::Arc;

/// Source location, used in error messages.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub source_name: Arc<String>,
    pub line: u32,
    pub column: u32,
    /// Byte offset from the start of the source text.
    pub byte_offset: u32,
    /// Length in bytes of the span (0 = point / unknown).
    pub byte_len: u32,
}

impl SourceLocation {
    /// Create from a `full_moon` position (point location, no span).
    pub fn from_pos(source_name: &Arc<String>, pos: full_moon::tokenizer::Position) -> Self {
        Self {
            source_name: Arc::clone(source_name),
            line: pos.line() as u32,
            column: pos.character() as u32,
            byte_offset: pos.bytes() as u32,
            byte_len: 0,
        }
    }

    /// Create from a start and end `full_moon` position (span).
    pub fn from_span(
        source_name: &Arc<String>,
        start: full_moon::tokenizer::Position,
        end: full_moon::tokenizer::Position,
    ) -> Self {
        let start_bytes = start.bytes() as u32;
        let end_bytes = end.bytes() as u32;
        Self {
            source_name: Arc::clone(source_name),
            line: start.line() as u32,
            column: start.character() as u32,
            byte_offset: start_bytes,
            byte_len: end_bytes.saturating_sub(start_bytes),
        }
    }

    /// Create a zero/unknown location.
    pub fn unknown(source_name: &Arc<String>) -> Self {
        Self {
            source_name: Arc::clone(source_name),
            line: 0,
            column: 0,
            byte_offset: 0,
            byte_len: 0,
        }
    }
}

impl From<SourceLocation> for shingetsu_vm::proto::SourceLocation {
    fn from(loc: SourceLocation) -> Self {
        Self {
            source_name: loc.source_name,
            line: loc.line,
            column: loc.column,
            byte_offset: loc.byte_offset,
            byte_len: loc.byte_len,
        }
    }
}

impl From<shingetsu_vm::proto::SourceLocation> for SourceLocation {
    fn from(loc: shingetsu_vm::proto::SourceLocation) -> Self {
        Self {
            source_name: loc.source_name,
            line: loc.line,
            column: loc.column,
            byte_offset: loc.byte_offset,
            byte_len: loc.byte_len,
        }
    }
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            shingetsu_vm::format_source_name(&self.source_name),
            self.line,
            self.column
        )
    }
}

/// Severity level for compiler diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    /// Suppressed entirely — not displayed.
    Allow,
    Warning,
    Error,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Allow => write!(f, "allow"),
            Severity::Warning => write!(f, "warn"),
            Severity::Error => write!(f, "deny"),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Severity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.as_str() {
            "allow" => Ok(Severity::Allow),
            "warn" => Ok(Severity::Warning),
            "deny" => Ok(Severity::Error),
            _ => Err(serde::de::Error::unknown_variant(
                &s,
                &["allow", "warn", "deny"],
            )),
        }
    }
}

/// Identifies the category of a built-in diagnostic check.
///
/// This is the closed set of lint identifiers shipped with shingetsu.
/// Plugin-defined lints live in the [`LintId::Plugin`] variant of the
/// outer [`LintId`] wrapper -- callers should typically interact with
/// [`LintId`] rather than this enum directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltInLintId {
    UnusedVariable,
    Shadowing,
    UnreachableCode,
    EmptyLoop,
    CallConvention,
    ArgCount,
    ArgType,
    ReturnType,
    AssignType,
    FieldAccess,
    MissingReturn,
    /// Emitted when a chunk uses Lua 5.5 `global` declarations and a free
    /// name is read or written without having been declared.
    UndeclaredGlobal,
    /// Emitted when an event handler lambda accepts more parameters than
    /// the registered signature declares; the extras would always be `nil`.
    EventHandlerArity,
    /// Emitted when an event handler lambda's parameter names look
    /// transposed relative to the registered signature — e.g. the user
    /// wrote `function(domain, message)` but the signature declares
    /// `(message, domain)`.
    EventHandlerTransposition,
    /// Emitted when `host.on('NAME', ...)` is called with an event
    /// name the type checker has not seen declared.  Default
    /// severity is Warning so registries with dynamic name policies
    /// (where names are added at runtime) are not falsely failed at
    /// compile time.  Promote to Error via project lint config to
    /// require every event name to be statically declared.
    EventNameUnknown,
    /// Emitted when a chunk being treated as a Lua module (e.g. by
    /// `shingetsu doc extract-lua`) does not return a single table
    /// value.  The extractor can only see a module's surface when
    /// the chunk ends with `return <table-shaped-value>`.
    ModuleShape,
    /// Emitted when a `---` doc-comment block is separated from
    /// the declaration it would document by a plain `--` comment.
    /// The `---` block is silently dropped today; this lint surfaces
    /// the mistake so the author can convert the interleaving `--`
    /// line to `---` (or move it inside the body).
    InterruptedDocComment,
    /// Emitted when a call references a function or accesses a
    /// field marked `@deprecated` (in DocModel or by `#[deprecated]`
    /// on the Rust-side declaration).  Carries the deprecation
    /// message when one was supplied.
    Deprecated,
    /// Emitted when a function marked `@nodiscard` is called in
    /// statement position (its return value is discarded).  The
    /// callee opts into this by setting `must_use` in its
    /// signature; pure-Lua sources declare it via the `@nodiscard`
    /// EmmyLua tag.
    MustUse,
    /// Emitted when a directive references an unknown lint name.
    UnknownLint,
}

impl BuiltInLintId {
    /// The snake_case string identifier for this lint.
    pub fn name(self) -> &'static str {
        match self {
            BuiltInLintId::UnusedVariable => "unused_variable",
            BuiltInLintId::Shadowing => "shadowing",
            BuiltInLintId::UnreachableCode => "unreachable_code",
            BuiltInLintId::EmptyLoop => "empty_loop",
            BuiltInLintId::CallConvention => "call_convention",
            BuiltInLintId::ArgCount => "arg_count",
            BuiltInLintId::ArgType => "arg_type",
            BuiltInLintId::ReturnType => "return_type",
            BuiltInLintId::AssignType => "assign_type",
            BuiltInLintId::FieldAccess => "field_access",
            BuiltInLintId::MissingReturn => "missing_return",
            BuiltInLintId::UndeclaredGlobal => "undeclared_global",
            BuiltInLintId::EventHandlerArity => "event_handler_arity",
            BuiltInLintId::EventHandlerTransposition => "event_handler_transposition",
            BuiltInLintId::EventNameUnknown => "event_name_unknown",
            BuiltInLintId::ModuleShape => "module_shape",
            BuiltInLintId::InterruptedDocComment => "interrupted_doc_comment",
            BuiltInLintId::Deprecated => "deprecated",
            BuiltInLintId::MustUse => "must_use",
            BuiltInLintId::UnknownLint => "unknown_lint",
        }
    }

    /// The compiled-in default severity for this lint.
    pub fn default_severity(self) -> Severity {
        match self {
            BuiltInLintId::UnusedVariable => Severity::Warning,
            BuiltInLintId::Shadowing => Severity::Warning,
            BuiltInLintId::UnreachableCode => Severity::Warning,
            BuiltInLintId::EmptyLoop => Severity::Warning,
            BuiltInLintId::CallConvention => Severity::Warning,
            BuiltInLintId::ArgCount => Severity::Error,
            BuiltInLintId::ArgType => Severity::Error,
            BuiltInLintId::ReturnType => Severity::Error,
            BuiltInLintId::AssignType => Severity::Error,
            BuiltInLintId::FieldAccess => Severity::Error,
            BuiltInLintId::MissingReturn => Severity::Error,
            BuiltInLintId::UndeclaredGlobal => Severity::Error,
            BuiltInLintId::EventHandlerArity => Severity::Warning,
            BuiltInLintId::EventHandlerTransposition => Severity::Warning,
            BuiltInLintId::EventNameUnknown => Severity::Warning,
            BuiltInLintId::ModuleShape => Severity::Warning,
            BuiltInLintId::InterruptedDocComment => Severity::Warning,
            BuiltInLintId::Deprecated => Severity::Warning,
            BuiltInLintId::MustUse => Severity::Warning,
            BuiltInLintId::UnknownLint => Severity::Warning,
        }
    }

    /// Look up a built-in lint by its string name.
    pub fn from_name(s: &str) -> Option<BuiltInLintId> {
        match s {
            "unused_variable" => Some(BuiltInLintId::UnusedVariable),
            "shadowing" => Some(BuiltInLintId::Shadowing),
            "unreachable_code" => Some(BuiltInLintId::UnreachableCode),
            "empty_loop" => Some(BuiltInLintId::EmptyLoop),
            "call_convention" => Some(BuiltInLintId::CallConvention),
            "arg_count" => Some(BuiltInLintId::ArgCount),
            "arg_type" => Some(BuiltInLintId::ArgType),
            "return_type" => Some(BuiltInLintId::ReturnType),
            "assign_type" => Some(BuiltInLintId::AssignType),
            "field_access" => Some(BuiltInLintId::FieldAccess),
            "missing_return" => Some(BuiltInLintId::MissingReturn),
            "undeclared_global" => Some(BuiltInLintId::UndeclaredGlobal),
            "event_handler_arity" => Some(BuiltInLintId::EventHandlerArity),
            "event_handler_transposition" => Some(BuiltInLintId::EventHandlerTransposition),
            "event_name_unknown" => Some(BuiltInLintId::EventNameUnknown),
            "module_shape" => Some(BuiltInLintId::ModuleShape),
            "interrupted_doc_comment" => Some(BuiltInLintId::InterruptedDocComment),
            "deprecated" => Some(BuiltInLintId::Deprecated),
            "must_use" => Some(BuiltInLintId::MustUse),
            _ => None,
        }
    }

    /// Returns all known built-in lint identifiers, sorted by name.
    pub fn all() -> &'static [BuiltInLintId] {
        static SORTED: std::sync::LazyLock<Vec<BuiltInLintId>> = std::sync::LazyLock::new(|| {
            let mut all = vec![
                BuiltInLintId::ArgCount,
                BuiltInLintId::ArgType,
                BuiltInLintId::AssignType,
                BuiltInLintId::CallConvention,
                BuiltInLintId::EventHandlerArity,
                BuiltInLintId::EventHandlerTransposition,
                BuiltInLintId::EventNameUnknown,
                BuiltInLintId::Deprecated,
                BuiltInLintId::FieldAccess,
                BuiltInLintId::InterruptedDocComment,
                BuiltInLintId::MissingReturn,
                BuiltInLintId::MustUse,
                BuiltInLintId::ModuleShape,
                BuiltInLintId::EmptyLoop,
                BuiltInLintId::ReturnType,
                BuiltInLintId::Shadowing,
                BuiltInLintId::UndeclaredGlobal,
                BuiltInLintId::UnreachableCode,
                BuiltInLintId::UnusedVariable,
            ];
            all.sort_by_key(|l| l.name());
            all
        });
        &SORTED
    }
}

impl std::fmt::Display for BuiltInLintId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl From<BuiltInLintId> for LintId {
    fn from(b: BuiltInLintId) -> LintId {
        LintId::BuiltIn(b)
    }
}

/// Identifies a lint -- either a shingetsu built-in or a project-loaded
/// plugin lint.
///
/// Plugin lint names are written `project:<name>` in source-level
/// directives, `shingetsu.toml`, and rendered diagnostics.  Built-in
/// names appear unprefixed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LintId {
    /// A shingetsu-defined built-in lint.
    BuiltIn(BuiltInLintId),
    /// A project-loaded plugin lint, identified by its declared
    /// name (without the `project:` prefix; the prefix is added by
    /// the renderer / directive parser).
    Plugin(std::sync::Arc<str>),
}

impl LintId {
    /// The user-visible identifier for this lint.  Built-in lints
    /// return their bare snake_case name; plugin lints return
    /// `project:<name>`.
    pub fn display_name(&self) -> std::borrow::Cow<'_, str> {
        match self {
            LintId::BuiltIn(b) => std::borrow::Cow::Borrowed(b.name()),
            LintId::Plugin(name) => std::borrow::Cow::Owned(format!("project:{name}")),
        }
    }

    /// The compiled-in default severity.  Built-in lints have their
    /// fixed defaults; plugin lints default to `Warning` until the
    /// plugin loader registers a per-plugin override.
    pub fn default_severity(&self) -> Severity {
        match self {
            LintId::BuiltIn(b) => b.default_severity(),
            LintId::Plugin(_) => Severity::Warning,
        }
    }

    /// Look up a lint by its user-visible name.  Resolves `project:`
    /// prefixed names to [`LintId::Plugin`] without validation
    /// (plugin existence is checked at load time, not here).
    /// Unprefixed names must resolve to a built-in or return `None`.
    pub fn from_name(s: &str) -> Option<LintId> {
        if let Some(plugin_name) = s.strip_prefix("project:") {
            if plugin_name.is_empty() {
                return None;
            }
            return Some(LintId::Plugin(std::sync::Arc::from(plugin_name)));
        }
        BuiltInLintId::from_name(s).map(LintId::BuiltIn)
    }

    /// Returns all known built-in lint identifiers wrapped in
    /// `LintId`.  Plugin lints are not enumerable through this
    /// path -- the plugin registry exposes them separately.
    pub fn all_builtins() -> impl Iterator<Item = LintId> {
        BuiltInLintId::all().iter().copied().map(LintId::BuiltIn)
    }
}

impl std::fmt::Display for LintId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.display_name())
    }
}

impl<'de> serde::Deserialize<'de> for LintId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        LintId::from_name(&s).ok_or_else(|| {
            static NAMES: std::sync::LazyLock<Vec<&str>> = std::sync::LazyLock::new(|| {
                BuiltInLintId::all().iter().map(|l| l.name()).collect()
            });
            serde::de::Error::unknown_variant(&s, &NAMES)
        })
    }
}

/// A non-fatal diagnostic emitted during compilation.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub lint: LintId,
    pub severity: Severity,
    pub location: SourceLocation,
    /// Full diagnostic message, used as the rendered title and as
    /// the default primary-annotation label.  Override the
    /// annotation label via [`Self::primary_label`] when the title
    /// should be verbose but the label at the carets should be
    /// short.
    pub message: String,
    pub help: Option<String>,
    /// Optional override for the label rendered next to the primary
    /// annotation's carets.  When `None`, the renderer reuses
    /// [`Self::message`].  Useful when [`Self::message`] is verbose
    /// (suitable for a title) but the carets sit on a short
    /// expression where a tighter label reads better.
    pub primary_label: Option<String>,
    /// Additional contextual spans surfaced alongside the primary
    /// `location`.  The renderer emits each as a non-primary
    /// annotation labelled with its accompanying message, so a
    /// diagnostic about a problem at one site can also point at a
    /// related site (e.g. the registration site plus the function
    /// definition that drives the validation).
    pub secondary_spans: Vec<(SourceLocation, String)>,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self.severity {
            Severity::Allow => "note",
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(f, "{}: {}: {}", self.location, label, self.message)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("{location}: {message}")]
    Parse {
        location: SourceLocation,
        message: String,
    },

    #[error("{location}: unsupported feature: {feature}")]
    UnsupportedFeature {
        location: SourceLocation,
        feature: String,
        /// Optional `help:` text rendered alongside the diagnostic.
        help: Option<String>,
    },

    #[error("{location}: {message}")]
    Semantic {
        location: SourceLocation,
        message: String,
        /// Optional `help:` text rendered alongside the diagnostic
        /// (e.g. an actionable suggestion).  `None` means no hint.
        help: Option<String>,
    },
}
