use std::collections::BTreeMap;

use bytes::Bytes;
use shingetsu_vm::ir::{ConstIdx, Instruction, NameIdx, Offset, Reg};
use shingetsu_vm::proto::{CallSiteInfo, SourceLocation};

/// Mutable bytecode builder for a single `Proto` being compiled.
pub struct CodeGen {
    pub instructions: Vec<Instruction>,
    /// String constant pool (strings, global names, label names).
    pub constants: Vec<Bytes>,
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

    /// Intern a constant string; returns its index in the constant pool.
    pub fn constant(&mut self, s: impl Into<Bytes>) -> ConstIdx {
        let s = s.into();
        if let Some(pos) = self.constants.iter().position(|c| c == &s) {
            return pos as ConstIdx;
        }
        let idx = self.constants.len() as ConstIdx;
        self.constants.push(s);
        idx
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
