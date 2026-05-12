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

/// Identifies the category of a diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LintId {
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
    /// Emitted when a directive references an unknown lint name.
    UnknownLint,
}

impl LintId {
    /// The snake_case string identifier for this lint.
    pub fn name(self) -> &'static str {
        match self {
            LintId::UnusedVariable => "unused_variable",
            LintId::Shadowing => "shadowing",
            LintId::UnreachableCode => "unreachable_code",
            LintId::EmptyLoop => "empty_loop",
            LintId::CallConvention => "call_convention",
            LintId::ArgCount => "arg_count",
            LintId::ArgType => "arg_type",
            LintId::ReturnType => "return_type",
            LintId::AssignType => "assign_type",
            LintId::FieldAccess => "field_access",
            LintId::MissingReturn => "missing_return",
            LintId::UndeclaredGlobal => "undeclared_global",
            LintId::EventHandlerArity => "event_handler_arity",
            LintId::EventHandlerTransposition => "event_handler_transposition",
            LintId::EventNameUnknown => "event_name_unknown",
            LintId::ModuleShape => "module_shape",
            LintId::UnknownLint => "unknown_lint",
        }
    }

    /// The compiled-in default severity for this lint.
    pub fn default_severity(self) -> Severity {
        match self {
            LintId::UnusedVariable => Severity::Warning,
            LintId::Shadowing => Severity::Warning,
            LintId::UnreachableCode => Severity::Warning,
            LintId::EmptyLoop => Severity::Warning,
            LintId::CallConvention => Severity::Warning,
            LintId::ArgCount => Severity::Error,
            LintId::ArgType => Severity::Error,
            LintId::ReturnType => Severity::Error,
            LintId::AssignType => Severity::Error,
            LintId::FieldAccess => Severity::Error,
            LintId::MissingReturn => Severity::Error,
            LintId::UndeclaredGlobal => Severity::Error,
            LintId::EventHandlerArity => Severity::Warning,
            LintId::EventHandlerTransposition => Severity::Warning,
            LintId::EventNameUnknown => Severity::Warning,
            LintId::ModuleShape => Severity::Warning,
            LintId::UnknownLint => Severity::Warning,
        }
    }

    /// Look up a lint by its string name.
    pub fn from_name(s: &str) -> Option<LintId> {
        match s {
            "unused_variable" => Some(LintId::UnusedVariable),
            "shadowing" => Some(LintId::Shadowing),
            "unreachable_code" => Some(LintId::UnreachableCode),
            "empty_loop" => Some(LintId::EmptyLoop),
            "call_convention" => Some(LintId::CallConvention),
            "arg_count" => Some(LintId::ArgCount),
            "arg_type" => Some(LintId::ArgType),
            "return_type" => Some(LintId::ReturnType),
            "assign_type" => Some(LintId::AssignType),
            "field_access" => Some(LintId::FieldAccess),
            "missing_return" => Some(LintId::MissingReturn),
            "undeclared_global" => Some(LintId::UndeclaredGlobal),
            "event_handler_arity" => Some(LintId::EventHandlerArity),
            "event_handler_transposition" => Some(LintId::EventHandlerTransposition),
            "event_name_unknown" => Some(LintId::EventNameUnknown),
            "module_shape" => Some(LintId::ModuleShape),
            _ => None,
        }
    }

    /// Returns all known lint identifiers.
    pub fn all() -> &'static [LintId] {
        static SORTED: std::sync::LazyLock<Vec<LintId>> = std::sync::LazyLock::new(|| {
            let mut all = vec![
                LintId::ArgCount,
                LintId::ArgType,
                LintId::AssignType,
                LintId::CallConvention,
                LintId::EventHandlerArity,
                LintId::EventHandlerTransposition,
                LintId::EventNameUnknown,
                LintId::FieldAccess,
                LintId::MissingReturn,
                LintId::ModuleShape,
                LintId::EmptyLoop,
                LintId::ReturnType,
                LintId::Shadowing,
                LintId::UndeclaredGlobal,
                LintId::UnreachableCode,
                LintId::UnusedVariable,
            ];
            all.sort_by_key(|l| l.name());
            all
        });
        &SORTED
    }
}

impl std::fmt::Display for LintId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl<'de> serde::Deserialize<'de> for LintId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        LintId::from_name(&s).ok_or_else(|| {
            static NAMES: std::sync::LazyLock<Vec<&str>> =
                std::sync::LazyLock::new(|| LintId::all().iter().map(|l| l.name()).collect());
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
