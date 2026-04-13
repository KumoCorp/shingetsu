use crate::Value;

#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error("attempt to perform arithmetic on a {type_name} value")]
    ArithmeticOnNonNumber { type_name: &'static str },

    #[error("attempt to concatenate a {type_name} value")]
    ConcatenationError { type_name: &'static str },

    #[error("attempt to compare {lhs} with {rhs}")]
    InvalidComparison {
        lhs: &'static str,
        rhs: &'static str,
    },

    #[error("attempt to call a {type_name} value")]
    CallNonFunction { type_name: &'static str },

    #[error("attempt to index a {type_name} value")]
    IndexNonTable { type_name: &'static str },

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
        expected: &'static str,
        got: &'static str,
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
}
