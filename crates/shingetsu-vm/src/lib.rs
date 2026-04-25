/// Maximum depth for chained metamethod lookups (`__index` table chains,
/// `__newindex` table chains).  Lua 5.4 uses `MAXTAGLOOP = 2000`; we use a
/// smaller limit because deep chains are almost always a bug.
pub const METAMETHOD_CHAIN_LIMIT: usize = 100;

pub mod byte_string;
pub mod bytecode;
pub mod call_context;
pub mod call_stack;
pub mod convert;
pub mod error;
pub mod file;
pub mod function;
pub(crate) mod gc;
pub mod global_env;
pub mod into_function;
pub mod ir;
pub mod meta_method;
pub mod module_loader;
pub mod proto;
pub mod table;
pub mod task;
pub mod traceback;
pub mod types;
pub mod userdata;
pub mod value;

pub use byte_string::Bytes;
pub use call_context::CallContext;
pub use call_stack::{CallStack, FrameLocals, StackFrame};
pub use convert::{
    FromLua, FromLuaBorrow, FromLuaMulti, IntoLua, IntoLuaMulti, LuaTyped, LuaTypedMulti, Never,
    Number, StdlibResult, TypedVariadic, Ud, Variadic,
};
pub use error::{Hint, RuntimeError, VarContext, VmError, VmResultExt};
pub use file::{BufferMode, CloseStatus, LuaFile, LuaFileOps, NumberAccumulator};
pub use function::{Function, NativeCall, NativeFunction, UpvalueCell};
pub use global_env::{value_to_error_string, GlobalEnv};
pub use into_function::{
    AsyncPlain, AsyncPlainVarargs, AsyncWithCtx, AsyncWithCtxVarargs, IntoIterResult,
    IntoNativeFunction, Plain, PlainVarargs, WithCtx, WithCtxVarargs,
};
pub use meta_method::MetaMethod;
pub use module_loader::{candidate_paths, LoadedModule, ModuleLoader};
pub use proto::{format_source_name, Proto, SourceLocation};
pub use table::Table;
pub use task::{value_matches_type, CallFrame, LuaFrame, NativeFrame, Task};
pub use types::{
    FieldDef, FieldKind, FunctionDef, FunctionLuaType, FunctionSignature, GenericTypeParam,
    GlobalTypeMap, LocalAttr, LuaType, LuaTypeArg, MetamethodDef, ModuleType, ParamSpec,
    TableLuaType, ValueType,
};
pub use userdata::{BinOpSide, Userdata};
pub use value::{Value, ValueVec};

// Re-export crates used by shingetsu-derive generated code so that
// `crate = "crate"` works from within this crate.
pub use async_trait;
pub use bytes;
pub use smallvec;
