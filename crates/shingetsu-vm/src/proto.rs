use std::sync::Arc;

use bytes::Bytes;

use crate::ir::Instruction;
use crate::types::{FunctionSignature, LocalAttr};

/// Source location embedded in bytecode for stack traces.
#[derive(Debug, Clone)]
pub struct SourceLocation {
    pub source_name: String,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.source_name, self.line, self.column)
    }
}

/// Descriptor for a local variable in a `Proto`.
#[derive(Debug, Clone)]
pub struct LocalDesc {
    pub name: Bytes,
    pub attr: LocalAttr,
    /// Register slot.
    pub slot: u8,
    /// PC at which the local comes into scope (inclusive).
    pub start_pc: usize,
    /// PC at which the local goes out of scope (exclusive).
    pub end_pc: usize,
}

/// Descriptor for an upvalue captured by a `Proto`.
#[derive(Debug, Clone)]
pub struct UpvalueDesc {
    pub name: Bytes,
    /// If `true`, captured from the immediately enclosing function's register.
    /// If `false`, captured from that function's upvalue list.
    pub in_stack: bool,
    /// Register or upvalue index in the enclosing function.
    pub index: u8,
}

/// A compiled function prototype — the static, shareable unit of bytecode.
#[derive(Debug)]
pub struct Proto {
    pub signature: Arc<FunctionSignature>,
    pub instructions: Vec<Instruction>,
    /// String constants referenced by `LoadK`, `GetGlobal`, etc.
    pub constants: Vec<Bytes>,
    pub locals: Vec<LocalDesc>,
    pub upvalues: Vec<UpvalueDesc>,
    /// Nested function prototypes (closures defined inside this function).
    pub protos: Vec<Arc<Proto>>,
    /// Per-instruction source locations, parallel to `instructions`.
    /// Empty when `debug_info` is false.
    pub source_locations: Vec<Option<SourceLocation>>,
    /// `type Name = ...` aliases declared in this function scope.
    /// Compile-time metadata only — no runtime effect.
    pub type_aliases: std::collections::HashMap<Bytes, crate::types::TypeAlias>,
}
