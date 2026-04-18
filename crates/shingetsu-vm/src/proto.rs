use std::collections::BTreeMap;
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
    /// Byte offset from the start of the source text.
    pub byte_offset: u32,
    /// Length in bytes of the span (0 = point / unknown).
    pub byte_len: u32,
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

/// Debug info for a `Call` instruction's call site, recording the
/// position of the `.` or `:` token so that diagnostic hints can
/// point at the exact token.
#[derive(Debug, Clone)]
pub struct CallSiteInfo {
    /// Byte offset of the `.` or `:` token from the start of the source.
    pub dot_colon_offset: u32,
    /// Byte length of the `.` or `:` token (always 1, but stored for
    /// consistency with `SourceLocation`).
    pub dot_colon_len: u32,
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
    /// Sparse per-instruction call-site debug info, keyed by PC.
    /// Only populated for `Call` instructions when `debug_info` is true.
    pub call_site_info: BTreeMap<usize, CallSiteInfo>,
    /// Original source text, shared across all `Proto`s from the same
    /// compilation.  Used by diagnostic rendering to show annotated
    /// source snippets.
    pub source_text: Bytes,
    /// `type Name = ...` aliases declared in this function scope.
    /// Compile-time metadata only — no runtime effect.
    pub type_aliases: std::collections::HashMap<Bytes, crate::types::TypeAlias>,
}

impl Proto {
    /// Set source text on this proto and all nested child protos.
    /// Uses `Bytes` cheap cloning so all protos share one allocation.
    /// Set source text on this proto and all nested child protos.
    /// Uses `Bytes` cheap cloning so all protos share one allocation.
    ///
    /// Must be called before any `Arc<Proto>` is shared (i.e. while
    /// each child proto has a unique reference).
    pub fn set_source_text(&mut self, source: Bytes) {
        self.source_text = source.clone();
        for child in &mut self.protos {
            Arc::get_mut(child)
                .expect("Proto already shared before set_source_text")
                .set_source_text(source.clone());
        }
    }
}
