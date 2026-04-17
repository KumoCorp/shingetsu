pub mod call_context;
pub mod convert;
pub mod error;
pub mod file;
pub mod function;
pub(crate) mod gc;
pub mod global_env;
pub mod into_function;
pub mod ir;
pub mod meta_method;
pub mod proto;
pub mod table;
pub mod task;
pub mod traceback;
pub mod types;
pub mod userdata;
pub mod value;

pub use call_context::{CallContext, StackFrame};
pub use convert::{CoerceInt, FromLua, FromLuaMulti, IntoLua, IntoLuaMulti, LuaTyped, Variadic};
pub use error::{VmError, VmResultExt};
pub use file::{BufferMode, CloseStatus, LuaFile, LuaFileOps, NumberAccumulator};
pub use function::{Function, NativeFunction, UpvalueCell};
pub use global_env::{value_to_error_string, GlobalEnv};
pub use into_function::{
    AsyncPlain, AsyncPlainVarargs, AsyncWithCtx, AsyncWithCtxVarargs, IntoIterResult,
    IntoNativeFunction, Plain, PlainVarargs, WithCtx, WithCtxVarargs,
};
pub use meta_method::MetaMethod;
pub use proto::{Proto, SourceLocation};
pub use table::Table;
pub use task::{value_matches_type, CallFrame, LuaFrame, NativeFrame, Task};
pub use types::{
    FieldDef, FieldKind, FunctionDef, FunctionLuaType, FunctionSignature, GenericTypeParam,
    LocalAttr, LuaType, LuaTypeArg, MetamethodDef, ModuleType, ParamSpec, TableLuaType, ValueType,
};
pub use userdata::Userdata;
pub use value::Value;

// Re-export crates used by shingetsu-derive generated code so that
// `crate = "crate"` works from within this crate.
pub use async_trait;
pub use bytes;
