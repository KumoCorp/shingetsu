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
    LoadInt {
        dst: Reg,
        value: i64,
    },
    LoadFloat {
        dst: Reg,
        value: f64,
    },
    /// Load a string constant.
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
        array_hint: u32,
        hash_hint: u32,
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
        count: i32,
        array_start: i64,
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
        nargs: i32,
        nresults: i32,
        /// Whether the call used `:` syntax (`obj:method()`).  Used
        /// to produce better error messages when `.` and `:` are
        /// confused.
        is_method_call: bool,
    },

    /// Generic `for … in` iterator call.
    ///
    /// Calls `frame[iter](frame[state], frame[control])` and writes
    /// `nresults` values starting at `frame[vars]`.  The caller's
    /// `return_dst` / `pending_nresults` are patched to `(vars, nresults)`
    /// before the call so the existing return-value machinery works unchanged.
    GenericForCall {
        iter: Reg,
        state: Reg,
        control: Reg,
        vars: Reg,
        nresults: u8,
    },

    /// Generic `for … in` loop check (emitted immediately after
    /// `GenericForCall` returns).
    ///
    /// If `frame[vars]` is nil: jump forward by `exit_offset` (exit loop).
    /// Otherwise copy `frame[vars]` into `frame[control]` and fall through
    /// into the loop body.
    GenericForCheck {
        control: Reg,
        vars: Reg,
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
        nresults: i32,
    },

    Return {
        base: Reg,
        nresults: i32,
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
    /// execute.
    ForPrep {
        counter: Reg,
        limit: Reg,
        step: Reg,
        exit_offset: Offset,
    },
    /// Numeric `for` step.  Increments counter; jumps back to body if
    /// still in range.
    ForStep {
        counter: Reg,
        limit: Reg,
        step: Reg,
        body_offset: Offset,
    },

    /// Concatenate `count` values starting at `base` into `dst`.
    Concat {
        dst: Reg,
        base: Reg,
        count: u8,
    },

    /// Invoke `__close` metamethod on register `slot`, then set it to nil.
    CloseVar {
        slot: Reg,
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
            | Self::LoadInt { dst, .. }
            | Self::LoadFloat { dst, .. }
            | Self::LoadK { dst, .. }
            | Self::Move { dst, .. }
            | Self::GetGlobal { dst, .. }
            | Self::GetUpval { dst, .. }
            | Self::GetTable { dst, .. }
            | Self::NewTable { dst, .. }
            | Self::NewClosure { dst, .. }
            | Self::Concat { dst, .. }
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
