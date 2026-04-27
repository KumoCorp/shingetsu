use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use shingetsu_vm::ir::{ConstIdx, Instruction, NameIdx, Offset, Reg};
use shingetsu_vm::proto::{CallSiteInfo, SourceLocation};
use shingetsu_vm::value::Value;
use shingetsu_vm::Bytes;

/// Hashable key for interning constant-pool entries.  Constant pool
/// values are limited to the literal forms the lexer produces
/// (`Integer`, `Float`, `String`, `Boolean`, `Nil`); Floats are
/// keyed by their bit pattern so equal NaN payloads dedupe.  Other
/// `Value` variants (Table/Function/Userdata) are never interned
/// here; they fall through to the fallback insertion path.
#[derive(Clone, PartialEq, Eq, Hash)]
enum ConstKey {
    Nil,
    Boolean(bool),
    Integer(i64),
    Float(u64),
    String(Bytes),
}

fn const_key(v: &Value) -> Option<ConstKey> {
    Some(match v {
        Value::Nil => ConstKey::Nil,
        Value::Boolean(b) => ConstKey::Boolean(*b),
        Value::Integer(i) => ConstKey::Integer(*i),
        Value::Float(f) => ConstKey::Float(f.to_bits()),
        Value::String(s) => ConstKey::String(s.clone()),
        _ => return None,
    })
}

/// Mutable bytecode builder for a single `Proto` being compiled.
pub struct CodeGen {
    pub instructions: Vec<Instruction>,
    /// Constant pool (strings, global names, numbers, etc.).
    pub constants: Vec<Value>,
    /// Index from constant key to its position in `constants`, used to
    /// dedupe in O(1).
    constant_index: HashMap<ConstKey, ConstIdx>,
    /// Source location of the first `add_constant` call that would
    /// have exceeded the `ConstIdx` (`u16`) range, or `None` when the
    /// pool is still within range.
    pub constant_overflow: Option<SourceLocation>,
    /// Patch list: reserved for future use.
    #[allow(dead_code)]
    patches: Vec<usize>,
    /// Per-instruction source locations, parallel to `instructions`.
    /// Populated when debug info is enabled.
    pub source_locations: Vec<Option<SourceLocation>>,
    /// Sparse per-instruction call-site debug info, keyed by PC.
    /// Only populated for `Call` instructions when `debug_info` is true.
    pub call_site_info: BTreeMap<usize, CallSiteInfo>,
    /// Current source location stamped onto each emitted instruction.
    current_loc: Option<SourceLocation>,
    /// Whether to track per-instruction source locations.
    debug_info: bool,
}

impl CodeGen {
    pub fn new(debug_info: bool) -> Self {
        CodeGen {
            instructions: Vec::new(),
            constants: Vec::new(),
            constant_index: HashMap::new(),
            constant_overflow: None,
            patches: Vec::new(),
            source_locations: Vec::new(),
            call_site_info: BTreeMap::new(),
            current_loc: None,
            debug_info,
        }
    }

    /// Current instruction index (= next instruction's index).
    pub fn pc(&self) -> usize {
        self.instructions.len()
    }

    /// Set the current source location for subsequently emitted instructions.
    pub fn set_loc(&mut self, loc: Option<SourceLocation>) {
        self.current_loc = loc;
    }

    /// Return the current source location, if set.
    pub fn current_loc(&self) -> Option<&SourceLocation> {
        self.current_loc.as_ref()
    }

    /// Emit an instruction and return its index.
    pub fn emit(&mut self, instr: Instruction) -> usize {
        let idx = self.instructions.len();
        self.instructions.push(instr);
        if self.debug_info {
            self.source_locations.push(self.current_loc.clone());
        }
        idx
    }

    /// Record call-site debug info for a `Call` instruction at `pc`.
    pub fn set_call_site_info(&mut self, pc: usize, info: CallSiteInfo) {
        if self.debug_info {
            self.call_site_info.insert(pc, info);
        }
    }

    /// Intern a value in the constant pool; returns its index.
    ///
    /// On overflow (the pool has already reached the `ConstIdx`
    /// (`u16`) limit) records the current source location in
    /// `constant_overflow` and returns `0`.  Callers must check that
    /// field at finish time and surface a compile error.  This
    /// deferred-error scheme avoids polluting every caller with
    /// `Result` plumbing for an overflow that only triggers on
    /// >65k-constant programs.
    pub fn add_constant(&mut self, v: Value) -> ConstIdx {
        if let Some(key) = const_key(&v) {
            if let Some(&idx) = self.constant_index.get(&key) {
                return idx;
            }
            let len = self.constants.len();
            if len >= ConstIdx::MAX as usize {
                self.record_overflow();
                return 0;
            }
            let idx = len as ConstIdx;
            self.constants.push(v);
            self.constant_index.insert(key, idx);
            return idx;
        }
        let len = self.constants.len();
        if len >= ConstIdx::MAX as usize {
            self.record_overflow();
            return 0;
        }
        let idx = len as ConstIdx;
        self.constants.push(v);
        idx
    }

    /// Record the first overflow location.  Subsequent calls leave
    /// the original location in place — the user wants to know where
    /// the pool *first* ran out of room, not where the last constant
    /// after that was rejected.
    fn record_overflow(&mut self) {
        if self.constant_overflow.is_none() {
            // Best-effort: clone the current emit location.  If we
            // had no location set (e.g. during an emit that doesn't
            // call `set_loc`), fall back to a placeholder with an
            // empty source name; the lowerer fills the name in when
            // surfacing the error so the diagnostic still points at
            // the right file.
            let loc = self.current_loc.clone().unwrap_or(SourceLocation {
                source_name: Arc::new(String::new()),
                line: 0,
                column: 0,
                byte_offset: 0,
                byte_len: 0,
            });
            self.constant_overflow = Some(loc);
        }
    }

    /// Intern a constant string; returns its index in the constant pool.
    pub fn constant(&mut self, s: impl Into<Bytes>) -> ConstIdx {
        self.add_constant(Value::String(s.into()))
    }

    /// Alias: intern a name (global or label); same pool as `constant`.
    pub fn name(&mut self, s: impl Into<Bytes>) -> NameIdx {
        self.constant(s)
    }

    /// Emit a `Jump` with a placeholder offset; returns the instruction index
    /// so it can be patched later with `patch`.
    pub fn emit_jump(&mut self) -> usize {
        self.emit(Instruction::Jump { offset: 0 })
    }

    /// Emit a `BranchFalse` with a placeholder offset.
    pub fn emit_branch_false(&mut self, src: Reg) -> usize {
        self.emit(Instruction::BranchFalse { src, offset: 0 })
    }

    /// Emit a `BranchTrue` with a placeholder offset.
    pub fn emit_branch_true(&mut self, src: Reg) -> usize {
        self.emit(Instruction::BranchTrue { src, offset: 0 })
    }

    /// Patch a previously-emitted jump at `jump_idx` to target `target_pc`.
    /// The offset is relative: `target_pc - (jump_idx + 1)`.
    pub fn patch(&mut self, jump_idx: usize, target_pc: usize) {
        let offset = target_pc as i64 - (jump_idx as i64 + 1);
        let instr = &mut self.instructions[jump_idx];
        match instr {
            Instruction::Jump { offset: o } => *o = offset as Offset,
            Instruction::BranchFalse { offset: o, .. } => *o = offset as Offset,
            Instruction::BranchTrue { offset: o, .. } => *o = offset as Offset,
            Instruction::ForPrep { exit_offset: o, .. } => *o = offset as Offset,
            Instruction::ForStep { body_offset: o, .. } => *o = offset as Offset,
            Instruction::GenericForCheck { exit_offset: o, .. } => *o = offset as Offset,
            _ => panic!("patch called on non-jump instruction at {jump_idx}"),
        }
    }

    /// Set the `exit_offset` of a `ForPrep` instruction.
    pub fn patch_for_prep(&mut self, idx: usize, exit_pc: usize) {
        let offset = exit_pc as i64 - (idx as i64 + 1);
        if let Instruction::ForPrep { exit_offset, .. } = &mut self.instructions[idx] {
            *exit_offset = offset as Offset;
        }
    }

    /// Set the `body_offset` of a `ForStep` instruction.
    pub fn patch_for_step(&mut self, idx: usize, body_pc: usize) {
        let offset = body_pc as i64 - (idx as i64 + 1);
        if let Instruction::ForStep { body_offset, .. } = &mut self.instructions[idx] {
            *body_offset = offset as Offset;
        }
    }
}
