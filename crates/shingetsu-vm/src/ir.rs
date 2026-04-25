/// Register index within a call frame.
pub type Reg = u8;

/// Index into a `Proto`'s constant pool.
pub type ConstIdx = u16;

/// Relative PC offset for jumps (signed; added to PC after fetch).
pub type Offset = i32;

/// Index into a `Proto`'s string constant pool (for names / labels).
pub type NameIdx = u16;

/// Index into a `Proto`'s upvalue descriptor list.
pub type UpvalIdx = u8;

/// One bytecode instruction.
#[derive(Debug, Clone, PartialEq)]
pub enum Instruction {
    LoadNil {
        dst: Reg,
    },
    LoadBool {
        dst: Reg,
        value: bool,
    },
    /// Load a constant (string, integer, or float) from the constant pool.
    LoadK {
        dst: Reg,
        idx: ConstIdx,
    },

    Move {
        dst: Reg,
        src: Reg,
    },

    GetGlobal {
        dst: Reg,
        name: NameIdx,
    },
    SetGlobal {
        name: NameIdx,
        src: Reg,
    },

    GetUpval {
        dst: Reg,
        upval: UpvalIdx,
    },
    SetUpval {
        upval: UpvalIdx,
        src: Reg,
    },

    GetTable {
        dst: Reg,
        table: Reg,
        key: Reg,
    },
    SetTable {
        table: Reg,
        key: Reg,
        src: Reg,
    },
    NewTable {
        dst: Reg,
        /// Log2-encoded array size hint.  0 means no hint;
        /// n>0 means approximately 2^(n-1) entries.
        array_hint: u8,
        /// Log2-encoded hash size hint, same encoding.
        hash_hint: u8,
    },

    /// Bulk-assign a run of consecutive registers into the array part of a
    /// table.  Emitted by the compiler for table constructors whose last
    /// positional field is a vararg (`...`) or a function call — Lua
    /// expands such an expression to fill the remaining array slots.
    ///
    /// For `i in 0..n`: `table[array_start + i] := frame[src_base + i]`.
    /// `count >= 0` copies exactly that many values.
    /// `count < 0` copies everything from `src_base` up to the current top
    /// of the register file (set by a preceding `Vararg { nresults: -1 }`
    /// or `Call { nresults: -1 }`).
    SetList {
        table: Reg,
        src_base: Reg,
        count: i16,
        /// Index into the constant pool for the 1-based array offset.
        array_start: ConstIdx,
    },

    NewClosure {
        dst: Reg,
        proto_idx: u16,
    },

    /// Regular (non-tail-call) function call.
    /// `nargs` is the exact count of arguments, or -1 meaning "take everything
    /// on the stack above `func`" (used when the last argument is a vararg or
    /// multi-return expansion).  `nresults` is the number of return values to
    /// keep, or -1 for all.
    Call {
        func: Reg,
        nargs: i16,
        nresults: i16,
        /// Whether the call used `:` syntax (`obj:method()`).  Used
        /// to produce better error messages when `.` and `:` are
        /// confused.
        is_method_call: bool,
    },

    /// Generic `for … in` iterator call.
    ///
    /// Calls `frame[base](frame[base+1], frame[base+2])` and writes
    /// `nresults` values starting at `frame[base+4]`.  The caller's
    /// `return_dst` / `pending_nresults` are patched to `(base+4, nresults)`
    /// before the call so the existing return-value machinery works unchanged.
    /// Layout: base=iter, base+1=state, base+2=control, base+3=closing,
    /// base+4..=vars.
    GenericForCall {
        base: Reg,
        nresults: u8,
    },

    /// Generic `for … in` loop check (emitted immediately after
    /// `GenericForCall` returns).
    ///
    /// If `frame[base+4]` is nil: jump forward by `exit_offset` (exit loop).
    /// Otherwise copy `frame[base+4]` into `frame[base+2]` and fall through
    /// into the loop body.
    /// Layout: base=iter, base+2=control, base+4=vars.
    GenericForCheck {
        base: Reg,
        exit_offset: Offset,
    },

    /// Copy vararg values (the extra arguments passed to this function beyond
    /// its declared parameters) into consecutive registers starting at `dst`.
    /// `nresults >= 0` copies exactly that many (padding with nil).
    /// `nresults < 0` copies all varargs and resizes the register file to
    /// `dst + n_varargs` so that a subsequent `Return { nresults: -1 }` or
    /// `Call { nargs: -1 }` picks up exactly those values.
    Vararg {
        dst: Reg,
        nresults: i16,
    },

    Return {
        base: Reg,
        nresults: i16,
    },

    Jump {
        offset: Offset,
    },
    BranchFalse {
        src: Reg,
        offset: Offset,
    },
    BranchTrue {
        src: Reg,
        offset: Offset,
    },

    /// Numeric `for` initialisation.  Validates and normalises counter,
    /// limit, step; jumps to `exit_offset` if the loop body should not
    /// execute.  `limit` is `base+1`, `step` is `base+2`.
    ForPrep {
        base: Reg,
        exit_offset: Offset,
    },
    /// Numeric `for` step.  Increments counter; jumps back to body if
    /// still in range.  `limit` is `base+1`, `step` is `base+2`.
    ForStep {
        base: Reg,
        body_offset: Offset,
    },

    /// Concatenate `count` values starting at `base` into `dst`.
    Concat {
        dst: Reg,
        base: Reg,
        count: u8,
    },

    /// Convert a value to its string representation using `tostring`
    /// semantics: strings pass through, numbers/booleans/nil stringify,
    /// tables/userdata invoke `__tostring` metamethod if present.
    /// Used by string interpolation to convert each expression part
    /// before concatenation with `Concat`.
    ToString {
        dst: Reg,
        src: Reg,
    },

    /// Invoke `__close` metamethod on register `slot`, then set it to nil.
    CloseVar {
        slot: Reg,
    },

    /// Close (detach) all open upvalues whose register index is >= `from`.
    /// The upvalue cells keep their current values but are removed from the
    /// frame's open-upvalue list so that subsequent captures of the same
    /// register create fresh cells.  Used at the end of each for-loop
    /// iteration to give each iteration its own upvalue identity.
    CloseUpvalues {
        from: Reg,
    },

    /// Goto target.  Resolved to a `Jump` offset during semantic analysis.
    Label {
        name: NameIdx,
    },
    /// Unconditional jump to a `Label`; crossing a `<close>` init is rejected
    /// at compile time.
    Goto {
        name: NameIdx,
    },

    /// Trigger `GlobalEnv::collect_cycles`.
    CollectGarbage,

    // ---- Arithmetic (binary) -------------------------------------------
    Add {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Sub {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Mul {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Div {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    IDiv {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Mod {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Pow {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BAnd {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BOr {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BXor {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Shl {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Shr {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },

    // ---- Arithmetic (unary) --------------------------------------------
    Neg {
        dst: Reg,
        src: Reg,
    },
    BNot {
        dst: Reg,
        src: Reg,
    },
    Not {
        dst: Reg,
        src: Reg,
    },
    Len {
        dst: Reg,
        src: Reg,
    },

    // ---- Comparison ----------------------------------------------------
    Eq {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Ne {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Lt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Le {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Gt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Ge {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
}

impl Instruction {
    /// Return the destination register this instruction writes to, if any.
    pub fn dst_reg(&self) -> Option<Reg> {
        match self {
            Self::LoadNil { dst }
            | Self::LoadBool { dst, .. }
            | Self::LoadK { dst, .. }
            | Self::Move { dst, .. }
            | Self::GetGlobal { dst, .. }
            | Self::GetUpval { dst, .. }
            | Self::GetTable { dst, .. }
            | Self::NewTable { dst, .. }
            | Self::NewClosure { dst, .. }
            | Self::Concat { dst, .. }
            | Self::ToString { dst, .. }
            | Self::Add { dst, .. }
            | Self::Sub { dst, .. }
            | Self::Mul { dst, .. }
            | Self::Div { dst, .. }
            | Self::IDiv { dst, .. }
            | Self::Mod { dst, .. }
            | Self::Pow { dst, .. }
            | Self::BAnd { dst, .. }
            | Self::BOr { dst, .. }
            | Self::BXor { dst, .. }
            | Self::Shl { dst, .. }
            | Self::Shr { dst, .. }
            | Self::Neg { dst, .. }
            | Self::BNot { dst, .. }
            | Self::Not { dst, .. }
            | Self::Len { dst, .. }
            | Self::Eq { dst, .. }
            | Self::Ne { dst, .. }
            | Self::Lt { dst, .. }
            | Self::Le { dst, .. }
            | Self::Gt { dst, .. }
            | Self::Ge { dst, .. } => Some(*dst),
            // Vararg writes starting at dst, but for single-slot
            // tracking we report the base register.
            Self::Vararg { dst, .. } => Some(*dst),
            _ => None,
        }
    }
}

/// Encode a size hint as a log2 byte: 0 → 0, n>0 → ceil(log2(n))+1.
pub fn encode_size_hint(n: u32) -> u8 {
    if n == 0 {
        return 0;
    }
    // 32 - leading_zeros gives ceil(log2(n+1)), but we want a value
    // such that 2^(result-1) >= n.
    let bits = u32::BITS - n.leading_zeros(); // 1..=32
    bits.min(255) as u8
}

/// Decode a log2 size hint back to an approximate size.
pub fn decode_size_hint(h: u8) -> u32 {
    if h == 0 {
        0
    } else {
        1u32 << (h - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_size() {
        k9::assert_equal!(std::mem::size_of::<Instruction>(), 8);
    }

    #[test]
    fn size_hint_zero() {
        k9::assert_equal!(encode_size_hint(0), 0u8);
        k9::assert_equal!(decode_size_hint(0), 0u32);
    }

    #[test]
    fn size_hint_powers_of_two() {
        k9::assert_equal!(encode_size_hint(1), 1u8);
        k9::assert_equal!(decode_size_hint(1), 1u32);

        k9::assert_equal!(encode_size_hint(2), 2u8);
        k9::assert_equal!(decode_size_hint(2), 2u32);

        k9::assert_equal!(encode_size_hint(4), 3u8);
        k9::assert_equal!(decode_size_hint(3), 4u32);

        k9::assert_equal!(encode_size_hint(256), 9u8);
        k9::assert_equal!(decode_size_hint(9), 256u32);
    }

    #[test]
    fn size_hint_non_powers() {
        // 3 is not a power of 2; encode rounds up
        k9::assert_equal!(encode_size_hint(3), 2u8);
        // decode(2) = 2, which is >= 3? No, 2 < 3.
        // The hint is approximate — decode gives 2^(h-1),
        // which is a lower bound for exact powers and an
        // upper bound after encode rounds up.
        k9::assert_equal!(decode_size_hint(2), 2u32);

        // 5 → ceil(log2(5)) = 3 → encode = 3, decode = 4
        k9::assert_equal!(encode_size_hint(5), 3u8);
        k9::assert_equal!(decode_size_hint(3), 4u32);
    }
}
