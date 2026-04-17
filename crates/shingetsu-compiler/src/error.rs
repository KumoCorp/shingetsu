/// Source location, used in error messages.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub source_name: String,
    pub line: u32,
    pub column: u32,
    /// Byte offset from the start of the source text.
    pub byte_offset: u32,
    /// Length in bytes of the span (0 = point / unknown).
    pub byte_len: u32,
}

impl SourceLocation {
    /// Create from a `full_moon` position (point location, no span).
    pub fn from_pos(source_name: &str, pos: full_moon::tokenizer::Position) -> Self {
        Self {
            source_name: source_name.to_string(),
            line: pos.line() as u32,
            column: pos.character() as u32,
            byte_offset: pos.bytes() as u32,
            byte_len: 0,
        }
    }

    /// Create from a start and end `full_moon` position (span).
    pub fn from_span(
        source_name: &str,
        start: full_moon::tokenizer::Position,
        end: full_moon::tokenizer::Position,
    ) -> Self {
        let start_bytes = start.bytes() as u32;
        let end_bytes = end.bytes() as u32;
        Self {
            source_name: source_name.to_string(),
            line: start.line() as u32,
            column: start.character() as u32,
            byte_offset: start_bytes,
            byte_len: end_bytes.saturating_sub(start_bytes),
        }
    }

    /// Create a zero/unknown location.
    pub fn unknown(source_name: &str) -> Self {
        Self {
            source_name: source_name.to_string(),
            line: 0,
            column: 0,
            byte_offset: 0,
            byte_len: 0,
        }
    }
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.source_name, self.line, self.column)
    }
}

/// Severity level for compiler diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
}

/// A non-fatal diagnostic emitted during compilation.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub location: SourceLocation,
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self.severity {
            Severity::Warning => "warning",
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
        feature: &'static str,
    },

    #[error("{location}: {message}")]
    Semantic {
        location: SourceLocation,
        message: String,
    },
}
