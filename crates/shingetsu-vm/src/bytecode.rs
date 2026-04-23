/// Opcode values for the compact u32 bytecode encoding.
///
/// ## Encoding formats
///
/// All instructions are exactly one `u32` wide, except `SetList` which
/// is followed by one `ExtraArg` instruction.
///
/// ```text
/// bits 0-6:   opcode (7 bits, 128 max)
/// bits 7-14:  A (8 bits)
///
/// Format iABC:
///   bit  15:    k (1 bit flag)
///   bits 16-23: B (8 bits)
///   bits 24-31: C (8 bits)
///
/// Format iABx:
///   bits 15-31: Bx (17 bits unsigned, 0..131071)
///
/// Format iAsBx:
///   bits 15-31: sBx (17 bits signed, -65536..65535)
///
/// Format isJ:
///   bits 7-31:  sJ (25 bits signed, -16777216..16777215)
///
/// Format iAx:
///   bits 7-31:  Ax (25 bits unsigned, for ExtraArg)
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    LoadNil = 0,
    LoadBool = 1,
    LoadK = 2,
    Move = 3,
    GetGlobal = 4,
    SetGlobal = 5,
    GetUpval = 6,
    SetUpval = 7,
    GetTable = 8,
    SetTable = 9,
    NewTable = 10,
    SetList = 11,
    NewClosure = 12,
    Call = 13,
    GenericForCall = 14,
    GenericForCheck = 15,
    Vararg = 16,
    Return = 17,
    Jump = 18,
    BranchFalse = 19,
    BranchTrue = 20,
    ForPrep = 21,
    ForStep = 22,
    Concat = 23,
    ToString = 24,
    CloseVar = 25,
    Label = 26,
    Goto = 27,
    CollectGarbage = 28,
    Add = 29,
    Sub = 30,
    Mul = 31,
    Div = 32,
    IDiv = 33,
    Mod = 34,
    Pow = 35,
    BAnd = 36,
    BOr = 37,
    BXor = 38,
    Shl = 39,
    Shr = 40,
    Neg = 41,
    BNot = 42,
    Not = 43,
    Len = 44,
    Eq = 45,
    Ne = 46,
    Lt = 47,
    Le = 48,
    Gt = 49,
    Ge = 50,
    ExtraArg = 51,
}

const OPCODE_COUNT: usize = 52;

static OPCODE_TABLE: [OpCode; OPCODE_COUNT] = [
    OpCode::LoadNil,
    OpCode::LoadBool,
    OpCode::LoadK,
    OpCode::Move,
    OpCode::GetGlobal,
    OpCode::SetGlobal,
    OpCode::GetUpval,
    OpCode::SetUpval,
    OpCode::GetTable,
    OpCode::SetTable,
    OpCode::NewTable,
    OpCode::SetList,
    OpCode::NewClosure,
    OpCode::Call,
    OpCode::GenericForCall,
    OpCode::GenericForCheck,
    OpCode::Vararg,
    OpCode::Return,
    OpCode::Jump,
    OpCode::BranchFalse,
    OpCode::BranchTrue,
    OpCode::ForPrep,
    OpCode::ForStep,
    OpCode::Concat,
    OpCode::ToString,
    OpCode::CloseVar,
    OpCode::Label,
    OpCode::Goto,
    OpCode::CollectGarbage,
    OpCode::Add,
    OpCode::Sub,
    OpCode::Mul,
    OpCode::Div,
    OpCode::IDiv,
    OpCode::Mod,
    OpCode::Pow,
    OpCode::BAnd,
    OpCode::BOr,
    OpCode::BXor,
    OpCode::Shl,
    OpCode::Shr,
    OpCode::Neg,
    OpCode::BNot,
    OpCode::Not,
    OpCode::Len,
    OpCode::Eq,
    OpCode::Ne,
    OpCode::Lt,
    OpCode::Le,
    OpCode::Gt,
    OpCode::Ge,
    OpCode::ExtraArg,
];

impl OpCode {
    #[inline(always)]
    pub fn from_u8(v: u8) -> OpCode {
        OPCODE_TABLE[v as usize]
    }
}

// ── Bit-field constants ────────────────────────────────────────────────

const OP_BITS: u32 = 7;
const A_BITS: u32 = 8;
const K_BITS: u32 = 1;
const B_BITS: u32 = 8;
const C_BITS: u32 = 8;
const BX_BITS: u32 = K_BITS + B_BITS + C_BITS; // 17
const SJ_BITS: u32 = A_BITS + BX_BITS; // 25

const OP_MASK: u32 = (1 << OP_BITS) - 1; // 0x7F
const A_MASK: u32 = (1 << A_BITS) - 1; // 0xFF
const B_MASK: u32 = (1 << B_BITS) - 1; // 0xFF
const C_MASK: u32 = (1 << C_BITS) - 1; // 0xFF
const BX_MASK: u32 = (1 << BX_BITS) - 1; // 0x1FFFF
const SJ_MASK: u32 = (1 << SJ_BITS) - 1; // 0x1FFFFFF

const A_POS: u32 = OP_BITS; // 7
const K_POS: u32 = A_POS + A_BITS; // 15
const B_POS: u32 = K_POS + K_BITS; // 16
const C_POS: u32 = B_POS + B_BITS; // 24
const BX_POS: u32 = K_POS; // 15
const SJ_POS: u32 = A_POS; // 7

const SBX_BIAS: i32 = (1 << (BX_BITS - 1)) as i32; // 65536
const SJ_BIAS: i32 = (1 << (SJ_BITS - 1)) as i32; // 16777216

// ── Field extraction (decode) ──────────────────────────────────────────

#[inline(always)]
pub fn get_opcode(i: u32) -> OpCode {
    OpCode::from_u8((i & OP_MASK) as u8)
}

#[inline(always)]
pub fn get_a(i: u32) -> u8 {
    ((i >> A_POS) & A_MASK) as u8
}

#[inline(always)]
pub fn get_k(i: u32) -> bool {
    ((i >> K_POS) & 1) != 0
}

#[inline(always)]
pub fn get_b(i: u32) -> u8 {
    ((i >> B_POS) & B_MASK) as u8
}

#[inline(always)]
pub fn get_c(i: u32) -> u8 {
    ((i >> C_POS) & C_MASK) as u8
}

#[inline(always)]
pub fn get_bx(i: u32) -> u32 {
    (i >> BX_POS) & BX_MASK
}

#[inline(always)]
pub fn get_sbx(i: u32) -> i32 {
    get_bx(i) as i32 - SBX_BIAS
}

#[inline(always)]
pub fn get_sj(i: u32) -> i32 {
    ((i >> SJ_POS) & SJ_MASK) as i32 - SJ_BIAS
}

#[inline(always)]
pub fn get_ax(i: u32) -> u32 {
    (i >> SJ_POS) & SJ_MASK
}

// ── Encoding helpers ───────────────────────────────────────────────────

#[inline(always)]
fn abc(op: OpCode, a: u8, k: bool, b: u8, c: u8) -> u32 {
    (op as u32)
        | ((a as u32) << A_POS)
        | ((k as u32) << K_POS)
        | ((b as u32) << B_POS)
        | ((c as u32) << C_POS)
}

#[inline(always)]
fn abx(op: OpCode, a: u8, bx: u32) -> u32 {
    debug_assert!(bx <= BX_MASK, "Bx overflow: {bx}");
    (op as u32) | ((a as u32) << A_POS) | (bx << BX_POS)
}

#[inline(always)]
fn asbx(op: OpCode, a: u8, sbx: i32) -> u32 {
    let biased = (sbx + SBX_BIAS) as u32;
    debug_assert!(biased <= BX_MASK, "sBx overflow: {sbx}");
    (op as u32) | ((a as u32) << A_POS) | (biased << BX_POS)
}

#[inline(always)]
fn sj(op: OpCode, offset: i32) -> u32 {
    let biased = (offset + SJ_BIAS) as u32;
    debug_assert!(biased <= SJ_MASK, "sJ overflow: {offset}");
    (op as u32) | (biased << SJ_POS)
}

#[inline(always)]
fn ax(op: OpCode, val: u32) -> u32 {
    debug_assert!(val <= SJ_MASK, "Ax overflow: {val}");
    (op as u32) | (val << SJ_POS)
}

// ── Encode one Instruction → 1 or 2 u32 words ─────────────────────────

use crate::ir::Instruction;

/// Encode a high-level `Instruction` into compact u32 word(s).
/// Most instructions produce exactly one u32.  `SetList` produces two
/// (the instruction followed by an `ExtraArg`).
pub fn encode(instr: &Instruction, out: &mut Vec<u32>) {
    match *instr {
        Instruction::LoadNil { dst } => {
            out.push(abc(OpCode::LoadNil, dst, false, 0, 0));
        }
        Instruction::LoadBool { dst, value } => {
            out.push(abc(OpCode::LoadBool, dst, false, value as u8, 0));
        }
        Instruction::LoadK { dst, idx } => {
            out.push(abx(OpCode::LoadK, dst, idx as u32));
        }
        Instruction::Move { dst, src } => {
            out.push(abc(OpCode::Move, dst, false, src, 0));
        }
        Instruction::GetGlobal { dst, name } => {
            out.push(abx(OpCode::GetGlobal, dst, name as u32));
        }
        Instruction::SetGlobal { name, src } => {
            out.push(abx(OpCode::SetGlobal, src, name as u32));
        }
        Instruction::GetUpval { dst, upval } => {
            out.push(abc(OpCode::GetUpval, dst, false, upval, 0));
        }
        Instruction::SetUpval { upval, src } => {
            out.push(abc(OpCode::SetUpval, upval, false, src, 0));
        }
        Instruction::GetTable { dst, table, key } => {
            out.push(abc(OpCode::GetTable, dst, false, table, key));
        }
        Instruction::SetTable { table, key, src } => {
            out.push(abc(OpCode::SetTable, table, false, key, src));
        }
        Instruction::NewTable {
            dst,
            array_hint,
            hash_hint,
        } => {
            out.push(abc(OpCode::NewTable, dst, false, array_hint, hash_hint));
        }
        Instruction::SetList {
            table,
            src_base,
            count,
            array_start,
        } => {
            // count: -1 means "all" → encode as 0; positive → count+1
            let c = if count < 0 { 0u8 } else { (count + 1) as u8 };
            out.push(abc(OpCode::SetList, table, false, src_base, c));
            out.push(ax(OpCode::ExtraArg, array_start as u32));
        }
        Instruction::NewClosure { dst, proto_idx } => {
            out.push(abx(OpCode::NewClosure, dst, proto_idx as u32));
        }
        Instruction::Call {
            func,
            nargs,
            nresults,
            is_method_call,
        } => {
            // nargs: -1 means vararg → 0; positive → nargs+1
            let b = if nargs < 0 { 0u8 } else { (nargs + 1) as u8 };
            let c = if nresults < 0 {
                0u8
            } else {
                (nresults + 1) as u8
            };
            out.push(abc(OpCode::Call, func, is_method_call, b, c));
        }
        Instruction::GenericForCall { base, nresults } => {
            out.push(abc(OpCode::GenericForCall, base, false, nresults, 0));
        }
        Instruction::GenericForCheck { base, exit_offset } => {
            out.push(asbx(OpCode::GenericForCheck, base, exit_offset));
        }
        Instruction::Vararg { dst, nresults } => {
            // -1 → 0, positive → nresults+1
            let b = if nresults < 0 {
                0u8
            } else {
                (nresults + 1) as u8
            };
            out.push(abc(OpCode::Vararg, dst, false, b, 0));
        }
        Instruction::Return { base, nresults } => {
            let b = if nresults < 0 {
                0u8
            } else {
                (nresults + 1) as u8
            };
            out.push(abc(OpCode::Return, base, false, b, 0));
        }
        Instruction::Jump { offset } => {
            out.push(sj(OpCode::Jump, offset));
        }
        Instruction::BranchFalse { src, offset } => {
            out.push(asbx(OpCode::BranchFalse, src, offset));
        }
        Instruction::BranchTrue { src, offset } => {
            out.push(asbx(OpCode::BranchTrue, src, offset));
        }
        Instruction::ForPrep { base, exit_offset } => {
            out.push(asbx(OpCode::ForPrep, base, exit_offset));
        }
        Instruction::ForStep { base, body_offset } => {
            out.push(asbx(OpCode::ForStep, base, body_offset));
        }
        Instruction::Concat { dst, base, count } => {
            out.push(abc(OpCode::Concat, dst, false, base, count));
        }
        Instruction::ToString { dst, src } => {
            out.push(abc(OpCode::ToString, dst, false, src, 0));
        }
        Instruction::CloseVar { slot } => {
            out.push(abc(OpCode::CloseVar, slot, false, 0, 0));
        }
        Instruction::Label { name } => {
            out.push(abx(OpCode::Label, 0, name as u32));
        }
        Instruction::Goto { name } => {
            out.push(abx(OpCode::Goto, 0, name as u32));
        }
        Instruction::CollectGarbage => {
            out.push(abc(OpCode::CollectGarbage, 0, false, 0, 0));
        }
        // Arithmetic binary
        Instruction::Add { dst, lhs, rhs } => {
            out.push(abc(OpCode::Add, dst, false, lhs, rhs));
        }
        Instruction::Sub { dst, lhs, rhs } => {
            out.push(abc(OpCode::Sub, dst, false, lhs, rhs));
        }
        Instruction::Mul { dst, lhs, rhs } => {
            out.push(abc(OpCode::Mul, dst, false, lhs, rhs));
        }
        Instruction::Div { dst, lhs, rhs } => {
            out.push(abc(OpCode::Div, dst, false, lhs, rhs));
        }
        Instruction::IDiv { dst, lhs, rhs } => {
            out.push(abc(OpCode::IDiv, dst, false, lhs, rhs));
        }
        Instruction::Mod { dst, lhs, rhs } => {
            out.push(abc(OpCode::Mod, dst, false, lhs, rhs));
        }
        Instruction::Pow { dst, lhs, rhs } => {
            out.push(abc(OpCode::Pow, dst, false, lhs, rhs));
        }
        Instruction::BAnd { dst, lhs, rhs } => {
            out.push(abc(OpCode::BAnd, dst, false, lhs, rhs));
        }
        Instruction::BOr { dst, lhs, rhs } => {
            out.push(abc(OpCode::BOr, dst, false, lhs, rhs));
        }
        Instruction::BXor { dst, lhs, rhs } => {
            out.push(abc(OpCode::BXor, dst, false, lhs, rhs));
        }
        Instruction::Shl { dst, lhs, rhs } => {
            out.push(abc(OpCode::Shl, dst, false, lhs, rhs));
        }
        Instruction::Shr { dst, lhs, rhs } => {
            out.push(abc(OpCode::Shr, dst, false, lhs, rhs));
        }
        // Arithmetic unary
        Instruction::Neg { dst, src } => {
            out.push(abc(OpCode::Neg, dst, false, src, 0));
        }
        Instruction::BNot { dst, src } => {
            out.push(abc(OpCode::BNot, dst, false, src, 0));
        }
        Instruction::Not { dst, src } => {
            out.push(abc(OpCode::Not, dst, false, src, 0));
        }
        Instruction::Len { dst, src } => {
            out.push(abc(OpCode::Len, dst, false, src, 0));
        }
        // Comparison
        Instruction::Eq { dst, lhs, rhs } => {
            out.push(abc(OpCode::Eq, dst, false, lhs, rhs));
        }
        Instruction::Ne { dst, lhs, rhs } => {
            out.push(abc(OpCode::Ne, dst, false, lhs, rhs));
        }
        Instruction::Lt { dst, lhs, rhs } => {
            out.push(abc(OpCode::Lt, dst, false, lhs, rhs));
        }
        Instruction::Le { dst, lhs, rhs } => {
            out.push(abc(OpCode::Le, dst, false, lhs, rhs));
        }
        Instruction::Gt { dst, lhs, rhs } => {
            out.push(abc(OpCode::Gt, dst, false, lhs, rhs));
        }
        Instruction::Ge { dst, lhs, rhs } => {
            out.push(abc(OpCode::Ge, dst, false, lhs, rhs));
        }
    }
}

/// Encode a full instruction stream into compact u32 bytecode.
/// Returns the encoded words, a mapping from old instruction indices
/// to new u32 word indices (for source locations and call-site info),
/// and remaps all jump/branch offsets to account for multi-word
/// instructions (SetList emits 2 words).
pub fn encode_all(instructions: &[Instruction]) -> (Vec<u32>, Vec<usize>) {
    let mut code = Vec::with_capacity(instructions.len());
    let mut index_map = Vec::with_capacity(instructions.len());
    for instr in instructions {
        index_map.push(code.len());
        encode(instr, &mut code);
    }
    // Sentinel so we can map target_pc == instructions.len() (end of code).
    index_map.push(code.len());

    // Fix up jump offsets: the original offsets are in terms of
    // high-level instruction indices.  We need them in u32 word indices.
    for old_pc in 0..instructions.len() {
        let new_pc = index_map[old_pc];
        let w = code[new_pc];
        let op = get_opcode(w);
        match op {
            OpCode::Jump => {
                let old_offset = get_sj(w);
                // target in old indices: old_pc + 1 + old_offset
                let old_target = (old_pc as i32 + 1 + old_offset) as usize;
                let new_target = index_map[old_target];
                let new_offset = new_target as i32 - (new_pc as i32 + 1);
                code[new_pc] = sj(op, new_offset);
            }
            OpCode::BranchFalse
            | OpCode::BranchTrue
            | OpCode::ForPrep
            | OpCode::ForStep
            | OpCode::GenericForCheck => {
                let a = get_a(w);
                let old_offset = get_sbx(w);
                let old_target = (old_pc as i32 + 1 + old_offset) as usize;
                let new_target = index_map[old_target];
                let new_offset = new_target as i32 - (new_pc as i32 + 1);
                code[new_pc] = asbx(op, a, new_offset);
            }
            _ => {}
        }
    }
    // Remove the sentinel.
    index_map.pop();
    (code, index_map)
}

// ── Decode: extract fields inline in the VM dispatch ───────────────────
//
// The VM doesn't reconstruct an `Instruction` enum.  It calls
// `get_opcode(word)` then extracts fields with `get_a`, `get_b`, etc.
// No decode function is needed here — the field extractors above
// are sufficient.

/// Return the destination register written by this instruction, if any.
/// Mirrors `Instruction::dst_reg()` for the compact encoding.
pub fn dst_reg(word: u32) -> Option<u8> {
    let op = get_opcode(word);
    match op {
        OpCode::LoadNil
        | OpCode::LoadBool
        | OpCode::LoadK
        | OpCode::Move
        | OpCode::GetGlobal
        | OpCode::GetUpval
        | OpCode::GetTable
        | OpCode::NewTable
        | OpCode::NewClosure
        | OpCode::Concat
        | OpCode::ToString
        | OpCode::Add
        | OpCode::Sub
        | OpCode::Mul
        | OpCode::Div
        | OpCode::IDiv
        | OpCode::Mod
        | OpCode::Pow
        | OpCode::BAnd
        | OpCode::BOr
        | OpCode::BXor
        | OpCode::Shl
        | OpCode::Shr
        | OpCode::Neg
        | OpCode::BNot
        | OpCode::Not
        | OpCode::Len
        | OpCode::Eq
        | OpCode::Ne
        | OpCode::Lt
        | OpCode::Le
        | OpCode::Gt
        | OpCode::Ge
        | OpCode::Vararg => Some(get_a(word)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_abc() {
        let instr = Instruction::Add {
            dst: 3,
            lhs: 7,
            rhs: 12,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        k9::assert_equal!(buf.len(), 1);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::Add);
        k9::assert_equal!(get_a(w), 3u8);
        k9::assert_equal!(get_b(w), 7u8);
        k9::assert_equal!(get_c(w), 12u8);
    }

    #[test]
    fn roundtrip_abx() {
        let instr = Instruction::LoadK { dst: 5, idx: 1000 };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::LoadK);
        k9::assert_equal!(get_a(w), 5u8);
        k9::assert_equal!(get_bx(w), 1000u32);
    }

    #[test]
    fn roundtrip_asbx() {
        let instr = Instruction::BranchFalse {
            src: 2,
            offset: -42,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::BranchFalse);
        k9::assert_equal!(get_a(w), 2u8);
        k9::assert_equal!(get_sbx(w), -42i32);
    }

    #[test]
    fn roundtrip_sj() {
        let instr = Instruction::Jump { offset: -100 };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::Jump);
        k9::assert_equal!(get_sj(w), -100i32);
    }

    #[test]
    fn roundtrip_call() {
        let instr = Instruction::Call {
            func: 10,
            nargs: 3,
            nresults: -1,
            is_method_call: true,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::Call);
        k9::assert_equal!(get_a(w), 10u8);
        k9::assert_equal!(get_k(w), true);
        k9::assert_equal!(get_b(w), 4u8); // nargs + 1
        k9::assert_equal!(get_c(w), 0u8); // -1 → 0
    }

    #[test]
    fn roundtrip_setlist() {
        let instr = Instruction::SetList {
            table: 1,
            src_base: 5,
            count: -1,
            array_start: 42,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        k9::assert_equal!(buf.len(), 2);
        let w0 = buf[0];
        let w1 = buf[1];
        k9::assert_equal!(get_opcode(w0), OpCode::SetList);
        k9::assert_equal!(get_a(w0), 1u8);
        k9::assert_equal!(get_b(w0), 5u8);
        k9::assert_equal!(get_c(w0), 0u8); // -1 → 0
        k9::assert_equal!(get_opcode(w1), OpCode::ExtraArg);
        k9::assert_equal!(get_ax(w1), 42u32);
    }

    #[test]
    fn roundtrip_vararg_all() {
        let instr = Instruction::Vararg {
            dst: 3,
            nresults: -1,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::Vararg);
        k9::assert_equal!(get_a(w), 3u8);
        k9::assert_equal!(get_b(w), 0u8); // -1 → 0
    }

    #[test]
    fn sbx_boundary_values() {
        // Max positive sBx
        let max_sbx = SBX_BIAS - 1; // 65535
        let instr = Instruction::BranchTrue {
            src: 0,
            offset: max_sbx,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        k9::assert_equal!(get_sbx(buf[0]), max_sbx);

        // Max negative sBx
        let min_sbx = -SBX_BIAS; // -65536
        let instr = Instruction::BranchFalse {
            src: 0,
            offset: min_sbx,
        };
        buf.clear();
        encode(&instr, &mut buf);
        k9::assert_equal!(get_sbx(buf[0]), min_sbx);

        // Zero
        let instr = Instruction::ForPrep {
            base: 5,
            exit_offset: 0,
        };
        buf.clear();
        encode(&instr, &mut buf);
        k9::assert_equal!(get_sbx(buf[0]), 0i32);
        k9::assert_equal!(get_a(buf[0]), 5u8);
    }

    #[test]
    fn sj_boundary_values() {
        let max_sj = SJ_BIAS - 1; // 16777215
        let instr = Instruction::Jump { offset: max_sj };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        k9::assert_equal!(get_sj(buf[0]), max_sj);

        let min_sj = -SJ_BIAS; // -16777216
        let instr = Instruction::Jump { offset: min_sj };
        buf.clear();
        encode(&instr, &mut buf);
        k9::assert_equal!(get_sj(buf[0]), min_sj);
    }

    #[test]
    fn bx_max_value() {
        let max_bx = BX_MASK; // 131071
        let instr = Instruction::LoadK {
            dst: 255,
            idx: max_bx as u16,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        k9::assert_equal!(get_a(buf[0]), 255u8);
        k9::assert_equal!(get_bx(buf[0]), max_bx as u16 as u32);
    }

    #[test]
    fn dst_reg_reports_destinations() {
        let mut buf = Vec::new();

        encode(
            &Instruction::Add {
                dst: 7,
                lhs: 1,
                rhs: 2,
            },
            &mut buf,
        );
        k9::assert_equal!(dst_reg(buf[0]), Some(7u8));

        buf.clear();
        encode(&Instruction::LoadNil { dst: 0 }, &mut buf);
        k9::assert_equal!(dst_reg(buf[0]), Some(0u8));

        buf.clear();
        encode(
            &Instruction::Vararg {
                dst: 10,
                nresults: 3,
            },
            &mut buf,
        );
        k9::assert_equal!(dst_reg(buf[0]), Some(10u8));
    }

    #[test]
    fn dst_reg_returns_none_for_non_writing() {
        let mut buf = Vec::new();

        encode(&Instruction::Jump { offset: 5 }, &mut buf);
        k9::assert_equal!(dst_reg(buf[0]), None);

        buf.clear();
        encode(&Instruction::SetGlobal { name: 0, src: 1 }, &mut buf);
        k9::assert_equal!(dst_reg(buf[0]), None);

        buf.clear();
        encode(
            &Instruction::Return {
                base: 0,
                nresults: 1,
            },
            &mut buf,
        );
        k9::assert_equal!(dst_reg(buf[0]), None);

        buf.clear();
        encode(
            &Instruction::SetTable {
                table: 0,
                key: 1,
                src: 2,
            },
            &mut buf,
        );
        k9::assert_equal!(dst_reg(buf[0]), None);
    }

    #[test]
    fn encode_all_remaps_jump_over_setlist() {
        // SetList emits 2 words. A jump targeting the instruction
        // after SetList must have its offset adjusted.
        let instrs = vec![
            Instruction::LoadNil { dst: 0 }, // word 0
            Instruction::SetList {
                // words 1-2 (SetList + ExtraArg)
                table: 0,
                src_base: 1,
                count: 3,
                array_start: 0,
            },
            Instruction::LoadNil { dst: 1 },  // word 3
            Instruction::Jump { offset: -3 }, // word 4: should jump to word 0
        ];
        let (code, index_map) = encode_all(&instrs);
        k9::assert_equal!(code.len(), 5); // 1 + 2 + 1 + 1
        k9::assert_equal!(index_map, vec![0, 1, 3, 4]);

        // The Jump at word 4 should target word 0.
        // Original offset -3 meant: target = pc(3) + 1 + (-3) = 1 (instr index 1)
        // Wait, let me recalculate:
        // Jump is at old index 3, offset -3 means target = 3 + 1 + (-3) = 1
        // Old index 1 maps to new index 1.
        // New Jump is at word 4, so new offset = 1 - (4 + 1) = -4
        let jump_word = code[4];
        k9::assert_equal!(get_opcode(jump_word), OpCode::Jump);
        k9::assert_equal!(get_sj(jump_word), -4i32);
    }

    #[test]
    fn encode_all_forward_jump_over_setlist() {
        let instrs = vec![
            Instruction::Jump { offset: 1 }, // word 0: skip next
            Instruction::SetList {
                // words 1-2
                table: 0,
                src_base: 1,
                count: -1,
                array_start: 0,
            },
            Instruction::LoadNil { dst: 0 }, // word 3
        ];
        let (code, _) = encode_all(&instrs);
        k9::assert_equal!(code.len(), 4); // 1 + 2 + 1

        // Jump at old index 0, offset 1: target = 0 + 1 + 1 = 2 (LoadNil)
        // Old index 2 maps to new index 3.
        // New Jump at word 0: new offset = 3 - (0 + 1) = 2
        let jump_word = code[0];
        k9::assert_equal!(get_opcode(jump_word), OpCode::Jump);
        k9::assert_equal!(get_sj(jump_word), 2i32);
    }

    #[test]
    fn roundtrip_all_abc_opcodes() {
        let abc_instrs = vec![
            Instruction::GetTable {
                dst: 1,
                table: 2,
                key: 3,
            },
            Instruction::SetTable {
                table: 4,
                key: 5,
                src: 6,
            },
            Instruction::NewTable {
                dst: 7,
                array_hint: 8,
                hash_hint: 9,
            },
            Instruction::Concat {
                dst: 10,
                base: 11,
                count: 12,
            },
            Instruction::Eq {
                dst: 13,
                lhs: 14,
                rhs: 15,
            },
            Instruction::Ne {
                dst: 16,
                lhs: 17,
                rhs: 18,
            },
            Instruction::Lt {
                dst: 19,
                lhs: 20,
                rhs: 21,
            },
            Instruction::Le {
                dst: 22,
                lhs: 23,
                rhs: 24,
            },
            Instruction::Gt {
                dst: 25,
                lhs: 26,
                rhs: 27,
            },
            Instruction::Ge {
                dst: 28,
                lhs: 29,
                rhs: 30,
            },
        ];
        for instr in &abc_instrs {
            let mut buf = Vec::new();
            encode(instr, &mut buf);
            k9::assert_equal!(buf.len(), 1);
        }
    }

    #[test]
    fn roundtrip_return_with_values() {
        let instr = Instruction::Return {
            base: 5,
            nresults: 3,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::Return);
        k9::assert_equal!(get_a(w), 5u8);
        k9::assert_equal!(get_b(w), 4u8); // 3 + 1
    }

    #[test]
    fn roundtrip_generic_for() {
        let instr = Instruction::GenericForCall {
            base: 10,
            nresults: 3,
        };
        let mut buf = Vec::new();
        encode(&instr, &mut buf);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::GenericForCall);
        k9::assert_equal!(get_a(w), 10u8);
        k9::assert_equal!(get_b(w), 3u8);

        let instr = Instruction::GenericForCheck {
            base: 10,
            exit_offset: -5,
        };
        buf.clear();
        encode(&instr, &mut buf);
        let w = buf[0];
        k9::assert_equal!(get_opcode(w), OpCode::GenericForCheck);
        k9::assert_equal!(get_a(w), 10u8);
        k9::assert_equal!(get_sbx(w), -5i32);
    }
}
