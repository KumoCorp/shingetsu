use std::io::ErrorKind;

use crate::call_context::StackFrame;
use crate::proto::SourceLocation;
use crate::Value;

/// Supplemental source locations for the variable involved in an error.
///
/// Populated at the error site by scanning debug info in the `Proto`.
/// Zero runtime cost during normal execution — only computed on error.
#[derive(Debug, Clone, Default)]
pub struct VarContext {
    /// Where the variable was declared (`local x = ...`).
    pub definition: Option<SourceLocation>,
    /// Where the variable was last assigned before the error.
    /// `None` if the variable was never reassigned after its declaration,
    /// or if the assignment site could not be determined.
    pub last_assignment: Option<SourceLocation>,
}

/// A runtime error paired with the call stack at the point of failure.
///
/// The stack trace is captured before unwinding, so it reflects the
/// exact state when the error occurred.
#[derive(Debug)]
pub struct RuntimeError {
    /// The underlying VM error.
    pub error: VmError,
    /// Call stack snapshot, outermost frame first.
    pub call_stack: Vec<StackFrame>,
    /// Source-location context for the variable referenced in the error
    /// (definition site, last assignment site).  Only populated when the
    /// error carries a `VarName` and debug info is available.
    pub var_context: Option<VarContext>,
    /// Source text from the innermost Lua frame's proto, if available.
    /// Used by diagnostic rendering to show annotated source snippets.
    pub source_text: bytes::Bytes,
    /// Structured hints attached to the error (e.g. `.` vs `:` suggestions).
    /// Rendered as additional annotated-snippet groups by the diagnostic
    /// renderer.
    pub hints: Vec<Hint>,
}

/// A structured hint attached to a runtime error, rendered as a
/// `help:` annotation by the diagnostic renderer.
#[derive(Debug, Clone)]
pub struct Hint {
    /// Source location to highlight in the hint snippet.  `None` for
    /// hints without a specific source location.
    pub location: Option<SourceLocation>,
    /// Human-readable hint message.
    pub message: String,
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, f)
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

impl RuntimeError {
    /// Access the inner `VmError`.
    pub fn vm_error(&self) -> &VmError {
        &self.error
    }
}

impl From<RuntimeError> for VmError {
    /// Extract the inner `VmError`, discarding the call stack.
    ///
    /// Used by native functions that propagate errors from nested
    /// `call_function` via `?` — the outer task captures its own
    /// full stack trace at the error boundary.
    fn from(re: RuntimeError) -> Self {
        re.error
    }
}

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

    #[error("{}", format_index_error(*.type_name, name.as_ref(), key.as_deref()))]
    IndexNonTable {
        type_name: &'static str,
        name: Option<VarName>,
        /// The key being indexed, if it is a short string suitable for display.
        key: Option<String>,
    },

    #[error("{}", format_length_error(*.type_name, name.as_ref()))]
    LengthNonTableOrString {
        type_name: &'static str,
        name: Option<VarName>,
    },

    #[error("stack overflow")]
    StackOverflow,

    #[error("{}", format_table_key_error("nil", name.as_ref()))]
    TableKeyIsNil {
        /// Source-level variable name of the table, if known.
        name: Option<VarName>,
    },

    #[error("{}", format_table_key_error("NaN", name.as_ref()))]
    TableKeyIsNaN {
        /// Source-level variable name of the table, if known.
        name: Option<VarName>,
    },

    #[error("bad argument #{position} to '{function}' ({expected} expected, got {got})")]
    BadArgument {
        position: usize,
        function: String,
        expected: String,
        got: String,
    },

    /// Argument-related error where the message is free-form, not a
    /// simple "expected X, got Y" — e.g. `integer overflow` or
    /// `initial position out of string`.  Formats as
    /// `bad argument #N to 'funcname' (msg)` matching Lua's
    /// `luaL_argerror` output.
    #[error("bad argument #{position} to '{function}' ({msg})")]
    ArgError {
        position: usize,
        function: String,
        msg: String,
    },

    /// Error thrown by Lua's `error()` / `assert()` functions.
    /// Preserves the original `Value` for `pcall` handlers.
    #[error("{display}")]
    LuaError { display: String, value: Value },

    /// Raised by `os.exit` to request process termination.  Carries the
    /// resolved `i32` exit code (Lua's `true`/`false`/integer argument
    /// already normalized) and the `close` flag from the `os.exit(code,
    /// close)` call.
    ///
    /// Propagates through the VM like any other error — closing `<close>`
    /// locals on the unwind path — but is deliberately **not** caught by
    /// `pcall`/`xpcall`, matching the "one-way" semantics of reference
    /// Lua's `os.exit` (which is a C `exit()` call that never returns).
    ///
    /// The embedder receives this variant from the top-level `Task`
    /// future and decides how to act on it (terminate the host process,
    /// log and continue, capture in a test, …).
    #[error("exit requested")]
    ExitRequested { code: i32, close: bool },

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

/// Return a platform-portable description for an `io::Error`.
///
/// Tries [`io_error_description`] (by `ErrorKind`) first, then
/// [`raw_os_error_description`] (by raw errno) for OS-originated
/// errors that Rust classifies as `Uncategorized`.  Falls back to
/// the error's own `Display` output if neither matches.
pub fn portable_io_error_description(e: &std::io::Error) -> String {
    if let Some(desc) = io_error_description(e.kind()) {
        return desc.to_owned();
    }
    if let Some(raw) = e.raw_os_error() {
        if let Some(desc) = raw_os_error_description(raw) {
            return desc.to_owned();
        }
    }
    e.to_string()
}

/// Map a raw OS errno value to a stable, platform-independent
/// description string.
///
/// This covers POSIX errno values that [`io_error_description`] cannot
/// handle because Rust's `ErrorKind` classifies them as
/// `Uncategorized`.  The strings are hardcoded (not derived from the
/// OS `strerror`) so they are identical on every platform.
pub fn raw_os_error_description(errno: i32) -> Option<&'static str> {
    match errno {
        libc::EBADF => Some("Bad file descriptor"),
        libc::ESPIPE => Some("Illegal seek"),
        libc::EISDIR => Some("Is a directory"),
        libc::ENOTDIR => Some("Not a directory"),
        libc::ENXIO => Some("No such device or address"),
        libc::ENODEV => Some("No such device"),
        libc::ETXTBSY => Some("Text file busy"),
        libc::EFBIG => Some("File too large"),
        libc::ENOSPC => Some("No space left on device"),
        libc::EROFS => Some("Read-only file system"),
        libc::EMLINK => Some("Too many links"),
        libc::ENAMETOOLONG => Some("File name too long"),
        libc::ENOTEMPTY => Some("Directory not empty"),
        libc::ELOOP => Some("Too many symbolic links"),
        libc::EMFILE => Some("Too many open files"),
        libc::ENFILE => Some("File table overflow"),
        libc::ENOSYS => Some("Function not implemented"),
        libc::EIO => Some("I/O error"),
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
        let desc = portable_io_error_description(&self.source);
        write!(f, "{}: {}", path, desc)
    }
}

impl std::error::Error for PathIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl VmError {
    /// Extract the first `VarName` from the error, if any.
    pub fn var_name(&self) -> Option<&VarName> {
        match self {
            VmError::ArithmeticOnNonNumber { name, .. } => name.as_ref(),
            VmError::ConcatenationError { name, .. } => name.as_ref(),
            VmError::CallNonFunction { name, .. } => name.as_ref(),
            VmError::IndexNonTable { name, .. } => name.as_ref(),
            VmError::LengthNonTableOrString { name, .. } => name.as_ref(),
            VmError::TableKeyIsNil { name, .. } => name.as_ref(),
            VmError::TableKeyIsNaN { name, .. } => name.as_ref(),
            VmError::InvalidComparison { lhs_name, .. } => lhs_name.as_ref(),
            _ => None,
        }
    }

    /// Attach a table variable name to `TableKeyIsNil` / `TableKeyIsNaN`.
    /// Other variants pass through unchanged.
    pub fn with_table_name(mut self, var_name: Option<VarName>) -> Self {
        match &mut self {
            VmError::TableKeyIsNil { name, .. } => *name = var_name,
            VmError::TableKeyIsNaN { name, .. } => *name = var_name,
            _ => {}
        }
        self
    }

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

    /// Patch the function name from a [`CallContext`] on any
    /// `BadArgument` or `ArgError`, preserving the existing position.
    /// `LuaError` is wrapped as `bad argument to 'name' (msg)`.
    pub fn with_function_context(self, ctx: &crate::call_context::CallContext) -> Self {
        let func_name = || {
            ctx.native_name
                .as_ref()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default()
        };
        match self {
            VmError::BadArgument {
                position,
                expected,
                got,
                ..
            } => VmError::BadArgument {
                position,
                function: func_name(),
                expected,
                got,
            },
            VmError::ArgError { position, msg, .. } => VmError::ArgError {
                position,
                function: func_name(),
                msg,
            },
            VmError::LuaError { display, value } => {
                let name = func_name();
                VmError::LuaError {
                    display: format!("bad argument to '{name}' ({display})"),
                    value,
                }
            }
            other => other,
        }
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
        let func_name = || {
            ctx.native_name
                .as_ref()
                .map(|n| String::from_utf8_lossy(n).into_owned())
                .unwrap_or_default()
        };
        match self {
            VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
                position,
                function: func_name(),
                expected,
                got,
            },
            VmError::ArgError { msg, .. } => VmError::ArgError {
                position,
                function: func_name(),
                msg,
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

fn format_index_error(type_name: &str, name: Option<&VarName>, key: Option<&str>) -> String {
    let base = match name {
        Some(v) => format!("attempt to index {} (a {} value)", format_var(v), type_name),
        None => format!("attempt to index a {} value", type_name),
    };
    match key {
        Some(k) => format!("{base} with key '{k}'"),
        None => base,
    }
}

fn format_table_key_error(key_desc: &str, name: Option<&VarName>) -> String {
    match name {
        Some(v) => format!("table index is {} (table is {})", key_desc, format_var(v)),
        None => format!("table index is {}", key_desc),
    }
}

fn format_length_error(type_name: &str, name: Option<&VarName>) -> String {
    match name {
        Some(v) => format!(
            "attempt to get length of {} (a {} value)",
            format_var(v),
            type_name
        ),
        None => format!("attempt to get length of a {} value", type_name),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_os_error_description_ebadf() {
        k9::assert_equal!(
            raw_os_error_description(libc::EBADF),
            Some("Bad file descriptor")
        );
    }

    #[test]
    fn raw_os_error_description_espipe() {
        k9::assert_equal!(raw_os_error_description(libc::ESPIPE), Some("Illegal seek"));
    }

    #[test]
    fn raw_os_error_description_unknown() {
        // Errno 0 or some unlikely value should return None.
        k9::assert_equal!(raw_os_error_description(0), None);
    }

    #[test]
    fn portable_description_known_kind() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "os msg");
        k9::assert_equal!(
            portable_io_error_description(&e),
            "No such file or directory"
        );
    }

    #[test]
    fn portable_description_raw_os_error() {
        let e = std::io::Error::from_raw_os_error(libc::EBADF);
        k9::assert_equal!(portable_io_error_description(&e), "Bad file descriptor");
    }

    #[test]
    fn portable_description_custom_message_preserved() {
        // Errors constructed with io::Error::new and an unrecognized
        // kind should keep their custom message.
        let e = std::io::Error::new(std::io::ErrorKind::Other, "custom msg");
        k9::assert_equal!(portable_io_error_description(&e), "custom msg");
    }
}
