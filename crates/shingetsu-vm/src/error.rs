use std::io::ErrorKind;

use crate::Value;

/// Whether a variable reference is local or global, for use in error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarKind {
    Local,
    Global,
}

/// A variable name paired with its scope kind, for contextual error messages.
#[derive(Debug, Clone)]
pub struct VarName {
    pub name: String,
    pub kind: VarKind,
}

impl VarName {
    pub fn local(name: impl Into<String>) -> Self {
        VarName {
            name: name.into(),
            kind: VarKind::Local,
        }
    }

    pub fn global(name: impl Into<String>) -> Self {
        VarName {
            name: name.into(),
            kind: VarKind::Global,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error("{}", format_arithmetic_error(*.type_name, name.as_ref()))]
    ArithmeticOnNonNumber {
        type_name: &'static str,
        /// Source-level variable name and kind, if known from debug info.
        name: Option<VarName>,
    },

    #[error("{}", format_concat_error(*.type_name, name.as_ref()))]
    ConcatenationError {
        type_name: &'static str,
        name: Option<VarName>,
    },

    #[error("{}", format_comparison_error(*.lhs, lhs_name.as_ref(), *.rhs, rhs_name.as_ref()))]
    InvalidComparison {
        lhs: &'static str,
        /// Source-level name of the left-hand operand, if known.
        lhs_name: Option<VarName>,
        rhs: &'static str,
        /// Source-level name of the right-hand operand, if known.
        rhs_name: Option<VarName>,
    },

    #[error("{}", format_call_error(*.type_name, name.as_ref()))]
    CallNonFunction {
        type_name: &'static str,
        name: Option<VarName>,
    },

    #[error("{}", format_index_error(*.type_name, name.as_ref()))]
    IndexNonTable {
        type_name: &'static str,
        name: Option<VarName>,
    },

    #[error("stack overflow")]
    StackOverflow,

    #[error("table index is nil")]
    TableKeyIsNil,

    #[error("table index is NaN")]
    TableKeyIsNaN,

    #[error("bad argument #{position} to '{function}' ({expected} expected, got {got})")]
    BadArgument {
        position: usize,
        function: String,
        expected: String,
        got: String,
    },

    /// Error thrown by Lua's `error()` / `assert()` functions.
    /// Preserves the original `Value` for `pcall` handlers.
    #[error("{display}")]
    LuaError { display: String, value: Value },

    /// Error propagated from a `Userdata::dispatch` or `NativeFunction::call`.
    #[error("error in '{name}': {source}")]
    HostError {
        name: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// I/O error with associated filesystem path context.
    #[error("{source}")]
    IoError {
        #[from]
        source: PathIoError,
    },
}

/// Map an [`std::io::ErrorKind`] to a stable, platform-independent
/// description string.  Useful for producing deterministic error
/// messages in tests and user-facing output.
pub fn io_error_description(kind: ErrorKind) -> Option<&'static str> {
    match kind {
        ErrorKind::NotFound => Some("No such file or directory"),
        ErrorKind::PermissionDenied => Some("Permission denied"),
        ErrorKind::AlreadyExists => Some("File already exists"),
        ErrorKind::InvalidInput => Some("Invalid argument"),
        ErrorKind::InvalidData => Some("Invalid data"),
        ErrorKind::TimedOut => Some("Operation timed out"),
        ErrorKind::WriteZero => Some("Write zero"),
        ErrorKind::Interrupted => Some("Operation interrupted"),
        ErrorKind::Unsupported => Some("Operation not supported"),
        ErrorKind::UnexpectedEof => Some("Unexpected end of file"),
        ErrorKind::OutOfMemory => Some("Out of memory"),
        ErrorKind::ConnectionRefused => Some("Connection refused"),
        ErrorKind::ConnectionReset => Some("Connection reset"),
        ErrorKind::ConnectionAborted => Some("Connection aborted"),
        ErrorKind::NotConnected => Some("Not connected"),
        ErrorKind::AddrInUse => Some("Address already in use"),
        ErrorKind::AddrNotAvailable => Some("Address not available"),
        ErrorKind::BrokenPipe => Some("Broken pipe"),
        ErrorKind::WouldBlock => Some("Operation would block"),
        _ => None,
    }
}

/// An I/O error paired with the filesystem path that caused it.
///
/// The [`Display`](std::fmt::Display) implementation formats as
/// `"{path}: {description}"` where `description` is a stable,
/// platform-independent string derived from the error kind.
/// The path is stored as raw bytes and converted to a lossy UTF-8
/// string only at display time.
#[derive(Debug)]
pub struct PathIoError {
    /// The raw path bytes that were being operated on.
    pub path: bytes::Bytes,
    /// The underlying I/O error.
    pub source: std::io::Error,
}

impl std::fmt::Display for PathIoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let path = String::from_utf8_lossy(&self.path);
        match io_error_description(self.source.kind()) {
            Some(desc) => write!(f, "{}: {}", path, desc),
            None => write!(f, "{}: {}", path, self.source),
        }
    }
}

impl std::error::Error for PathIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl VmError {
    /// Enrich an error with a source-level variable name.
    /// Only modifies `ArithmeticOnNonNumber`, `ConcatenationError`,
    /// and `InvalidComparison`; other variants pass through unchanged.
    pub fn with_name(mut self, var_name: Option<VarName>) -> Self {
        if var_name.is_none() {
            return self;
        }
        match &mut self {
            VmError::ArithmeticOnNonNumber { name, .. } => *name = var_name,
            VmError::ConcatenationError { name, .. } => *name = var_name,
            VmError::InvalidComparison { lhs_name, .. } => *lhs_name = var_name,
            _ => {}
        }
        self
    }

    /// Enrich an `InvalidComparison` error with both operand names.
    pub fn with_comparison_names(
        mut self,
        lhs_var: Option<VarName>,
        rhs_var: Option<VarName>,
    ) -> Self {
        match &mut self {
            VmError::InvalidComparison {
                lhs_name, rhs_name, ..
            } => {
                *lhs_name = lhs_var;
                *rhs_name = rhs_var;
            }
            _ => {}
        }
        self
    }

    /// Patch a `BadArgument` error with the correct 1-based argument
    /// position and the function name from a [`CallContext`].
    ///
    /// `FromLua` impls produce placeholder values (`position: 0`, empty
    /// function name); this fills them in at the call site where the
    /// context and argument index are known.
    ///
    /// Non-`BadArgument` errors pass through unchanged.
    pub fn with_arg_and_call_context(
        self,
        position: usize,
        ctx: &crate::call_context::CallContext,
    ) -> Self {
        match self {
            VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
                position,
                function: ctx
                    .native_name
                    .as_ref()
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default(),
                expected,
                got,
            },
            other => other,
        }
    }
}

/// Extension trait for `Result<T, VmError>` that provides convenient
/// error-context helpers without requiring a closure + `map_err`.
pub trait VmResultExt<T> {
    /// Patch any `BadArgument` error with the given position and call context.
    fn with_call_context(
        self,
        position: usize,
        ctx: &crate::call_context::CallContext,
    ) -> Result<T, VmError>;

    /// Patch the argument position on any `BadArgument` error, leaving
    /// the function name unchanged.
    fn with_arg_position(self, position: usize) -> Result<T, VmError>;
}

impl<T> VmResultExt<T> for Result<T, VmError> {
    fn with_call_context(
        self,
        position: usize,
        ctx: &crate::call_context::CallContext,
    ) -> Result<T, VmError> {
        self.map_err(|e| e.with_arg_and_call_context(position, ctx))
    }

    fn with_arg_position(self, position: usize) -> Result<T, VmError> {
        self.map_err(|e| match e {
            VmError::BadArgument {
                function,
                expected,
                got,
                ..
            } => VmError::BadArgument {
                position,
                function,
                expected,
                got,
            },
            other => other,
        })
    }
}

fn format_var(var: &VarName) -> String {
    let kind = match var.kind {
        VarKind::Local => "local ",
        VarKind::Global => "global ",
    };
    format!("{}'{}'", kind, var.name)
}

fn format_index_error(type_name: &str, name: Option<&VarName>) -> String {
    match name {
        Some(v) => format!("attempt to index {} (a {} value)", format_var(v), type_name),
        None => format!("attempt to index a {} value", type_name),
    }
}

fn format_call_error(type_name: &str, name: Option<&VarName>) -> String {
    match name {
        Some(v) => format!("attempt to call {} (a {} value)", format_var(v), type_name),
        None => format!("attempt to call a {} value", type_name),
    }
}

fn format_arithmetic_error(type_name: &str, name: Option<&VarName>) -> String {
    match name {
        Some(v) => format!(
            "attempt to perform arithmetic on {} (a {} value)",
            format_var(v),
            type_name
        ),
        None => format!("attempt to perform arithmetic on a {} value", type_name),
    }
}

fn format_concat_error(type_name: &str, name: Option<&VarName>) -> String {
    match name {
        Some(v) => format!(
            "attempt to concatenate {} (a {} value)",
            format_var(v),
            type_name
        ),
        None => format!("attempt to concatenate a {} value", type_name),
    }
}

fn format_comparison_error(
    lhs: &str,
    lhs_name: Option<&VarName>,
    rhs: &str,
    rhs_name: Option<&VarName>,
) -> String {
    // When both types are the same, use "two <type> values".
    // When different, use "<lhs> with <rhs>".
    let type_part = if lhs == rhs {
        format!("two {} values", lhs)
    } else {
        format!("{} with {}", lhs, rhs)
    };
    // Pick the first named operand to annotate the message.
    match lhs_name.or(rhs_name) {
        Some(v) => format!("attempt to compare {} ({})", type_part, format_var(v)),
        None => format!("attempt to compare {}", type_part),
    }
}
