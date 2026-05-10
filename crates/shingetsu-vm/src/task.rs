use crate::valuevec;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::future::BoxFuture;

use crate::bytecode::{self, OpCode};
use crate::call_context::CallContext;
use crate::call_stack::{CallStack, FrameLocals, StackFrame};
use crate::error::{RuntimeError, VmError};
use crate::function::{Function, FunctionState};
use crate::global_env::GlobalEnv;
use crate::proto::{Proto, SourceLocation};
use crate::table::Table;
use crate::types::{FunctionSignature, LocalAttr, ValueType};
use crate::upvalue::{UpvalueCell, UpvalueInner};
use crate::userdata::Userdata;
use crate::value::{Value, ValueVec};

// ---------------------------------------------------------------------------
// Call frames
// ---------------------------------------------------------------------------

pub struct LuaFrame {
    pub proto: Arc<Proto>,
    pub pc: usize,
    /// Fixed-capacity register array, allocated at `max_stack_size`.
    /// Must not be replaced while open upvalues point into it.
    pub registers: Box<[Value]>,
    /// Logical number of active registers.  Always <= registers.len().
    /// Variable-length ops (Vararg, Return with -1) use this to know
    /// where the active region ends.
    pub reg_count: usize,
    /// Upvalue cells captured by this closure (one per `Proto::upvalues` entry).
    pub upvalues: Vec<UpvalueCell>,
    /// Open upvalue cells for locals in this frame that have been captured by
    /// nested closures.  Each entry is `(slot, cell)`.  While the cell is open,
    /// both `get` and `set` route through it so inner closures see mutations.
    pub open_upvalues: Vec<(u8, UpvalueCell)>,
    pub call_site: Option<SourceLocation>,
    /// Register slot where call results should be written when this frame
    /// returns (set by the parent frame's `Call` handler).
    pub return_dst: usize,
    /// Number of results the caller expects (-1 = all).
    pub pending_nresults: i32,
    /// Extra arguments passed beyond the function's declared parameter count.
    /// Only populated when `proto.signature.variadic` is true.
    pub varargs: Vec<Value>,
    /// When true, the first result of this frame's return is converted to a
    /// `Value::Boolean` before being written back to the caller.  Used by
    /// comparison metamethods (`__eq`, `__lt`, `__le`) so that `==` / `<` /
    /// `<=` always produce a boolean regardless of what the metamethod returns.
    pub coerce_result_to_bool: bool,
    /// Whether the most recent `Call` instruction used `:` syntax.
    /// Used by `detect_hints` to suggest `.` vs `:` corrections.
    pub last_call_is_method: bool,
    /// Byte offset and length of the `.` or `:` token at the most recent
    /// call site.  Used by `detect_hints` to point the hint annotation
    /// at the exact token.
    pub last_call_dot_colon: Option<(u32, u32)>,
    /// Byte offset of the start of the receiver expression at the most
    /// recent call site.  Combined with `last_call_dot_colon` to extract
    /// the receiver name from source text.
    pub last_call_receiver_offset: Option<u32>,
    /// Signature of the callee for the most recent `Call` instruction.
    /// Used by `detect_hints` when the callee frame is not on the stack
    /// (native functions, validate_args errors).
    pub last_call_callee_sig: Option<Arc<FunctionSignature>>,
}

impl LuaFrame {
    /// Read a register value by cloning it.
    #[inline]
    pub fn get(&self, slot: u8) -> Value {
        self.registers[slot as usize].clone()
    }

    /// Borrow a register value without cloning.
    #[inline]
    pub fn get_ref(&self, slot: u8) -> &Value {
        &self.registers[slot as usize]
    }

    /// Write a value into a register.
    #[inline]
    pub fn set(&mut self, slot: u8, val: Value) {
        let i = slot as usize;
        self.registers[i] = val;
        if i >= self.reg_count {
            self.reg_count = i + 1;
        }
    }

    /// Grow the register array if `needed` exceeds its current capacity.
    ///
    /// Because open upvalue cells may hold raw pointers into the
    /// register array, all open upvalues must be closed and then
    /// re-opened with pointers into the new array so they continue
    /// to track the live register values.
    fn ensure_registers(&mut self, needed: usize) {
        if needed > self.registers.len() {
            // Close all open upvalues, reallocate, then re-open them.
            // Step 1: Close — copies current register values into the
            // cells so no dangling pointers remain.
            for (_slot, cell) in &self.open_upvalues {
                // Safety: the old register array is still alive at this
                // point, so any Open pointers are valid for closing.
                unsafe { cell.close() };
            }

            // Step 2: Reallocate the register array.
            let mut new_regs = vec![Value::Nil; needed].into_boxed_slice();
            new_regs[..self.registers.len()].clone_from_slice(&self.registers);
            self.registers = new_regs;

            // Step 3: Re-open upvalues with pointers into the new array.
            // This keeps the bidirectional sync between registers and
            // cells alive across the reallocation.
            for (slot, cell) in &self.open_upvalues {
                let ptr = &mut self.registers[*slot as usize] as *mut Value;
                // Safety: the new register array was just allocated and
                // the slot index is valid.
                unsafe { cell.reopen(ptr) };
            }
        }
    }

    /// Close all open upvalues on this frame, copying each pointed-to
    /// value into the cell itself (Open → Closed).
    fn close_upvalues(&mut self) {
        for (_slot, cell) in self.open_upvalues.drain(..) {
            // Safety: the frame owns the register array that any open
            // upvalue pointers refer to. Closing copies the pointed-to
            // value into the cell, converting Open(*mut Value) →
            // Closed(Value). After this the raw pointer is no longer
            // stored or used.
            unsafe { cell.close() };
        }
    }

    /// Close all open upvalues and return the register array for recycling.
    fn take_registers(&mut self) -> Box<[Value]> {
        self.close_upvalues();
        self.reg_count = 0;
        std::mem::replace(&mut self.registers, Box::new([]))
    }

    /// Look up the variable name for a register slot using debug info.
    /// Follows Move chains and GetGlobal instructions to find the
    /// source variable name.  Returns both the name and whether it is
    /// a local or global, so error messages can distinguish the two.
    pub fn register_name(&self, slot: u8) -> Option<crate::error::VarName> {
        self.register_name_inner(slot, 5)
    }

    /// For a binary arithmetic error, return the name of the first
    /// operand that is not coercible to a number.
    pub fn arith_error_name(&self, lhs: u8, rhs: u8) -> Option<crate::error::VarName> {
        let l = self.get(lhs);
        if l.to_float().is_none() {
            return self.register_name(lhs);
        }
        self.register_name(rhs)
    }

    /// For a binary bitwise error, return the name of the first
    /// operand that is not an integer.
    pub fn bitwise_error_name(&self, lhs: u8, rhs: u8) -> Option<crate::error::VarName> {
        let l = self.get(lhs);
        if l.as_integer().is_none() {
            return self.register_name(lhs);
        }
        self.register_name(rhs)
    }

    fn register_name_inner(&self, slot: u8, depth: u8) -> Option<crate::error::VarName> {
        if depth == 0 {
            return None;
        }
        let pc = self.pc.saturating_sub(1);
        // Check local variable debug info first.
        for desc in &self.proto.locals {
            if desc.slot == slot && pc >= desc.start_pc && pc < desc.end_pc {
                if let Ok(name) = std::str::from_utf8(&desc.name) {
                    return Some(crate::error::VarName::local(name));
                }
            }
        }
        // Fall back: scan backwards for the instruction that loaded this
        // register.  We only look at a small window to avoid being misled
        // by distant instructions.
        let start = pc.saturating_sub(6);
        for scan_pc in (start..=pc).rev() {
            if let Some(&word) = self.proto.code.get(scan_pc) {
                match bytecode::get_opcode(word) {
                    OpCode::GetGlobal if bytecode::get_a(word) == slot => {
                        let name_idx = bytecode::get_bx(word) as u16;
                        if let Some(s) = self.constant_str(name_idx) {
                            return Some(crate::error::VarName::global(s));
                        }
                    }
                    OpCode::Move if bytecode::get_a(word) == slot => {
                        let src = bytecode::get_b(word);
                        return self.register_name_inner(src, depth - 1);
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// Look up the source location where a local variable was defined.
    ///
    /// `LocalDesc::start_pc` is the PC where the variable comes into scope
    /// (after its initializer).  The declaration statement's source location
    /// is on the instruction just before `start_pc` (the initializer itself).
    fn definition_location(&self, var: &crate::error::VarName) -> Option<SourceLocation> {
        if var.kind != crate::error::VarKind::Local {
            return None;
        }
        let pc = self.pc.saturating_sub(1);
        for desc in &self.proto.locals {
            if desc.start_pc <= pc && pc < desc.end_pc && desc.name == var.name.as_bytes() {
                if let Some(ref loc) = desc.decl_location {
                    return Some(loc.clone());
                }
                // Fallback: the initializer instruction is at start_pc - 1;
                // its source location points to the declaration statement.
                let def_pc = desc.start_pc.saturating_sub(1);
                return self
                    .proto
                    .source_locations
                    .get(def_pc)
                    .and_then(|s| s.clone());
            }
        }
        None
    }

    fn is_implicit_self(&self, var: &crate::error::VarName) -> bool {
        if var.kind != crate::error::VarKind::Local {
            return false;
        }
        let pc = self.pc.saturating_sub(1);
        self.proto.locals.iter().any(|desc| {
            desc.start_pc <= pc
                && pc < desc.end_pc
                && desc.name == var.name.as_bytes()
                && desc.is_implicit_self
        })
    }

    /// Scan backwards from the error PC for the most recent instruction
    /// that wrote to the variable's register slot.  Returns the source
    /// location of that instruction.
    ///
    /// Zero runtime cost during normal execution — only runs on error.
    fn last_assignment_location(&self, var: &crate::error::VarName) -> Option<SourceLocation> {
        if var.kind != crate::error::VarKind::Local {
            return None;
        }
        let pc = self.pc.saturating_sub(1);
        // Find the register slot for this variable.
        let slot = self
            .proto
            .locals
            .iter()
            .find(|desc| {
                desc.start_pc <= pc && pc < desc.end_pc && desc.name == var.name.as_bytes()
            })?
            .slot;
        // Scan backwards for the most recent write to this slot.
        // We scan a generous window but stop at the variable's start_pc.
        let start_pc = self
            .proto
            .locals
            .iter()
            .find(|desc| {
                desc.start_pc <= pc && pc < desc.end_pc && desc.name == var.name.as_bytes()
            })?
            .start_pc;
        for scan_pc in (start_pc..pc).rev() {
            if let Some(&word) = self.proto.code.get(scan_pc) {
                if bytecode::dst_reg(word) == Some(slot) {
                    return self
                        .proto
                        .source_locations
                        .get(scan_pc)
                        .and_then(|s| s.clone());
                }
            }
        }
        None
    }

    /// Read a string constant from the proto's constant pool.
    pub fn constant_str(&self, idx: u16) -> Option<&str> {
        match self.proto.constants.get(idx as usize) {
            Some(Value::String(b)) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }
}

impl Drop for LuaFrame {
    fn drop(&mut self) {
        // Last-resort guard: in normal operation, take_registers() will
        // have already closed every upvalue (making this a no-op). If a
        // frame is dropped without going through that path, we must
        // close upvalues here before the register array is deallocated.
        self.close_upvalues();
    }
}

/// Frame representing an in-progress native function call.
pub struct NativeFrame {
    pub signature: Arc<FunctionSignature>,
    pub call_site: Option<SourceLocation>,
}

pub enum CallFrame {
    Lua(LuaFrame),
    Native(NativeFrame),
}

// ---------------------------------------------------------------------------
// Step result (VM-internal)
// ---------------------------------------------------------------------------

enum Step {
    Done(ValueVec),
    Yield(BoxFuture<'static, Result<ValueVec, VmError>>),
}

/// Result of `exec_call` — tells the inner dispatch loop whether it needs
/// to re-fetch the frame reference.
enum CallResult {
    /// The call completed without changing the frame stack (SyncPlain).
    /// The inner loop can continue without `continue 'outer`.
    Done,
    /// The frame stack changed (Lua call, async call, metamethod).
    /// The caller must `continue 'outer` to re-fetch the frame.
    FrameChanged,
    /// The caller should yield or return this step.
    Yield(Step),
}

// ---------------------------------------------------------------------------
// TaskInner
// ---------------------------------------------------------------------------

/// What kind of pending async operation is currently suspended.
enum PendingKind {
    /// A native function call; results are written back to the caller frame.
    NativeCall,
    /// A `__close` metamethod dispatch during normal scope exit; results are
    /// discarded.
    CloseVar,
    /// A `__close` dispatch during error-path unwinding; results are
    /// discarded and the original error is preserved.
    UnwindClose,
    /// An async `__index` lookup driven by an `Invoke` opcode.  When the
    /// future resolves to a `Function` value, the VM dispatches a call
    /// using the original Invoke's argument range.  Continuation
    /// parameters are stashed in `TaskInner::pending_invoke`.
    InvokeAfterIndex,
}

/// State stashed by `exec_invoke` when the method-name lookup needs to
/// suspend (async `__index` dispatch on a userdata, or a Lua-function
/// `__index` metamethod).  Once the lookup completes — either via
/// future resolution or via a Lua metamethod frame's `Return` — the
/// resolved function value is in `R(dst)` and the dispatch loop picks
/// up this continuation, restores the receiver into `R(dst)`, and
/// performs the actual call.
struct InvokeContinuation {
    /// Receiver register; doubles as the self/arg-1 slot and the result
    /// destination.  During the lookup `R(dst)` temporarily holds the
    /// resolved function value (clobbering the receiver); the
    /// continuation restores the receiver here before dispatching.
    dst: u8,
    /// Number of arguments including self; -1 for vararg/multi-return tail.
    nargs: i32,
    /// Number of results to keep; -1 for all.
    nresults: i32,
    /// PC of the Invoke instruction (used for `set_top_call_pc`).
    call_pc: usize,
    /// `frames.len() - 1` at the moment the Invoke was issued.  The
    /// continuation only fires when control returns to this frame
    /// (i.e. the metamethod or its future has finished).
    caller_frame_idx: usize,
    /// The original receiver value, stashed because `R(dst)` is
    /// clobbered by the lookup.
    receiver: Value,
    /// Diagnostics captured at the Invoke site, applied to the caller
    /// frame just before dispatch.
    dot_colon_span: Option<(u32, u32)>,
    receiver_offset: Option<u32>,
}

struct TaskInner {
    global: GlobalEnv,
    frames: Vec<CallFrame>,
    pending: Option<BoxFuture<'static, Result<ValueVec, VmError>>>,
    pending_kind: PendingKind,
    /// nresults expected by the frame that launched the currently-pending
    /// native call (unused for CloseVar/UnwindClose).
    pending_nresults: i32,
    /// Return-register slot in the Lua caller frame for the current pending
    /// native call (unused for CloseVar/UnwindClose).
    pending_dst: usize,
    /// The error being propagated during error-path `<close>` unwinding.
    /// `None` means normal (non-unwind) execution.
    unwind_error: Option<RuntimeError>,
    /// Queue of `<close>` values still to be dispatched during unwinding.
    /// Values are popped from the end (LIFO), so they are pushed in
    /// outermost-first / earliest-declared-first order.
    unwind_close_vals: Vec<Value>,
    /// Free-list of register `Vec<Value>` buffers for reuse across Lua
    /// calls, avoiding repeated malloc/free.
    register_pool: Vec<Box<[Value]>>,
    /// Persistent call stack maintained incrementally.  Push/pop/clone
    /// are all O(1).  Does not contain local-variable values.
    call_stack: CallStack,
    /// Number of frames in `call_stack` that were inherited from the
    /// parent task.  Used by `snapshot_call_stack_with_locals` to
    /// distinguish parent frames (which don't have live locals) from
    /// our own frames.
    parent_stack_len: usize,
    /// Continuation state for an `Invoke` opcode whose method-name
    /// lookup is awaiting an async `__index` future or a Lua
    /// metamethod frame's return.  `Some` from the moment the lookup
    /// is dispatched until the dispatch loop picks it up at the
    /// caller frame and performs the actual call.
    pending_invoke: Option<InvokeContinuation>,
}

const MAX_STACK_DEPTH: usize = 200;

impl TaskInner {
    /// Build a `CallContext` from the current task state.
    ///
    /// Uses the persistent `call_stack` (O(1) clone) rather than iterating
    /// frames.  The persistent stack does not contain locals — those are
    /// accessed via `FrameLocals` for functions that need them.
    /// Begin error-path unwinding: collect all live `<close>` values from
    /// the current frames, then store the error for the poll loop to handle.
    #[cold]
    fn begin_unwind(&mut self, err: VmError) {
        // Discard any pending Invoke continuation — the lookup didn't
        // complete and the receiver/args are about to be dropped.
        self.pending_invoke = None;
        // Capture call stack, variable context, and source text before clearing frames.
        let call_stack = self.snapshot_call_stack_with_locals();
        let var_context = self.resolve_var_context(&err);
        let source_text = self
            .frames
            .iter()
            .rev()
            .find_map(|cf| match cf {
                CallFrame::Lua(f) => Some(f.proto.source_text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let hints = Self::detect_hints(&err, &call_stack, &source_text);
        let vals = collect_close_vals(&mut self.frames);
        // Close open upvalues and recycle register buffers before dropping frames.
        for frame in self.frames.drain(..) {
            if let CallFrame::Lua(mut f) = frame {
                recycle_registers(&mut self.register_pool, f.take_registers());
            }
        }
        self.unwind_close_vals = vals;
        // Peel any host-supplied attributions off the error so they
        // land on the corresponding `RuntimeError` fields rather
        // than leaking the internal wrapper variants to downstream
        // consumers.
        let (error, peeled) = err.peel_attributions();
        let mut hints = hints;
        hints.extend(peeled.hints.into_iter().map(|m| crate::error::Hint {
            location: None,
            message: m,
        }));
        self.unwind_error = Some(RuntimeError {
            error,
            call_stack,
            var_context,
            source_text,
            hints,
            arg_position: peeled.arg_position,
        });
    }

    /// Snapshot the current call stack as a `Vec<StackFrame>` with locals.
    ///
    /// Used only on error paths (`begin_unwind`) where we need local-variable
    /// values for `detect_hints` and `RuntimeError`.  Parent frames are
    /// taken from the persistent `call_stack` (they have no live locals);
    /// our own Lua frames are re-walked to capture locals.
    #[cold]
    fn snapshot_call_stack_with_locals(&self) -> Vec<StackFrame> {
        // Take the parent portion of the persistent stack.
        let full = self.call_stack.to_vec();
        let mut call_stack: Vec<StackFrame> =
            full.into_iter().take(self.parent_stack_len).collect();
        // Re-walk our own frames with locals.
        for cf in &self.frames {
            let f = match cf {
                CallFrame::Lua(f) => f,
                CallFrame::Native(_) => continue,
            };
            // Collect live locals (requires debug info in the proto).
            let locals: Vec<(crate::byte_string::Bytes, Value)> = f
                .proto
                .locals
                .iter()
                .filter(|l| l.start_pc <= f.pc && f.pc < l.end_pc)
                .map(|l| (l.name.clone(), f.get(l.slot)))
                .collect();
            call_stack.push(StackFrame::Lua {
                function: f.proto.signature.clone(),
                proto: f.proto.clone(),
                call_pc: f.pc.checked_sub(1),
                locals,
                last_call_is_method: f.last_call_is_method,
                last_call_dot_colon: f.last_call_dot_colon,
                last_call_receiver_offset: f.last_call_receiver_offset,
                last_call_callee_sig: f.last_call_callee_sig.clone(),
            });
        }
        call_stack
    }

    fn build_call_context(&self, native_name: Option<crate::byte_string::Bytes>) -> CallContext {
        CallContext::new(self.global.clone(), self.call_stack.clone(), native_name)
    }

    fn build_frame_locals(&self, native_name: crate::byte_string::Bytes) -> FrameLocals {
        build_frame_locals_from(
            &self.frames,
            &self.call_stack,
            self.parent_stack_len,
            native_name,
        )
    }

    /// Detect situations where a structured hint can help the user.
    ///
    /// Detects two directions of `.` vs `:` confusion:
    /// 1. **Dot-on-colon**: function defined with `:`  (has implicit `self`)
    ///    but called with `.` — `self` receives a non-table/userdata value.
    /// 2. **Colon-on-dot**: function defined with `.` (no `self` param)
    ///    but called with `:` — an extra `self` argument shifts all params.
    ///
    /// Works for both Lua-defined functions (where the callee frame is on the
    /// stack) and native functions (where `last_call_callee_sig` on the caller
    /// frame provides the callee's signature).
    fn detect_hints(
        err: &VmError,
        call_stack: &[StackFrame],
        source_text: &[u8],
    ) -> Vec<crate::error::Hint> {
        let mut hints = Vec::new();

        // Only emit hints for errors plausibly caused by `.`/`:` confusion.
        let is_relevant = matches!(
            err,
            VmError::ArithmeticOnNonNumber { .. }
                | VmError::ConcatenationError { .. }
                | VmError::IndexNonTable { .. }
                | VmError::CallNonFunction { .. }
                | VmError::LengthNonTableOrString { .. }
                | VmError::InvalidComparison { .. }
                | VmError::BadArgument { .. }
                | VmError::ArgError { .. }
        );
        if !is_relevant {
            return hints;
        }

        // Find the caller Lua frame — the one that issued the Call instruction.
        // It stores `last_call_callee_sig`, `last_call_is_method`, and
        // `last_call_dot_colon` for the most recent call.
        //
        // Layout possibilities:
        //  - Lua-to-Lua error: [..., caller_lua, callee_lua]
        //  - Native validate_args error: [..., caller_lua]  (native not pushed)
        //  - Native runtime error: [..., caller_lua]  (native popped before unwind)
        //
        // In all cases, the caller is the innermost Lua frame that has
        // `last_call_callee_sig` set.  For Lua-to-Lua calls, the callee
        // frame sits above the caller and provides locals for the self check.
        let mut caller_idx = None;
        for (i, frame) in call_stack.iter().enumerate().rev() {
            if let StackFrame::Lua {
                last_call_callee_sig: Some(_),
                ..
            } = frame
            {
                caller_idx = Some(i);
                break;
            }
        }

        let Some(caller_idx) = caller_idx else {
            return hints;
        };

        let StackFrame::Lua {
            last_call_is_method: called_with_colon,
            last_call_dot_colon,
            last_call_receiver_offset,
            last_call_callee_sig: Some(callee_sig),
            proto: caller_proto,
            ..
        } = &call_stack[caller_idx]
        else {
            return hints;
        };

        // Extract the receiver name from source text (e.g. "c" from "c.add(5)").
        let receiver_name: Option<&str> = match (last_call_receiver_offset, last_call_dot_colon) {
            (Some(recv_off), Some((dot_off, _))) if (*recv_off as usize) < (*dot_off as usize) => {
                let slice = &source_text[*recv_off as usize..*dot_off as usize];
                std::str::from_utf8(slice).ok().map(|s| s.trim())
            }
            _ => None,
        };

        // Build a SourceLocation pointing at the `.`/`:` token for the hint.
        let dot_colon_loc = last_call_dot_colon.map(|(offset, len)| crate::proto::SourceLocation {
            source_name: Arc::clone(&caller_proto.source_name),
            line: 0,
            column: 0,
            byte_offset: offset,
            byte_len: len,
        });

        // Determine if the callee is a method definition.
        // Check both the param name ("self") and arg_offset (used by native
        // userdata methods where the first Lua arg is the implicit self).
        let is_method_def = callee_sig.arg_offset > 0
            || callee_sig
                .params
                .first()
                .and_then(|p| p.name.as_ref())
                .map_or(false, |n| n == "self");

        // Check for a Lua callee frame above the caller (Lua-to-Lua calls).
        let lua_callee = if caller_idx + 1 < call_stack.len() {
            match &call_stack[caller_idx + 1] {
                StackFrame::Lua {
                    function, locals, ..
                } => Some((function, locals)),
                _ => None,
            }
        } else {
            None
        };

        let func_name = String::from_utf8_lossy(&callee_sig.name);
        let method_name = func_name
            .split_once(':')
            .or_else(|| func_name.split_once('.'))
            .map_or(func_name.as_ref(), |(_, m)| m);

        if is_method_def && !called_with_colon {
            // Dot-on-colon: function uses `:` syntax but was called with `.`.
            // For Lua callees, verify by checking the runtime value of `self`
            // to avoid false positives when `obj.method(obj)` is used.
            let should_hint = if let Some((_fn_sig, locals)) = lua_callee {
                locals
                    .iter()
                    .find(|(n, _)| n == "self")
                    .map_or(false, |(_, self_val)| {
                        !matches!(self_val, Value::Table(_) | Value::Userdata(_))
                    })
            } else {
                // Native callee — no locals to inspect, but an error occurred
                // so the hint is appropriate.
                true
            };
            if should_hint {
                let recv = receiver_name.unwrap_or("obj");
                hints.push(crate::error::Hint {
                    location: dot_colon_loc,
                    message: format!(
                        "'{func_name}' uses ':' syntax \u{2014} \
                         call as {recv}:{method_name}() \
                         not {recv}.{method_name}()"
                    ),
                });
            }
        } else if !is_method_def && *called_with_colon {
            // Colon-on-dot: function uses `.` syntax but was called with `:`.
            // The implicit `self` argument shifted all parameters.
            let recv = receiver_name.unwrap_or("obj");
            hints.push(crate::error::Hint {
                location: dot_colon_loc,
                message: format!(
                    "'{func_name}' uses '.' syntax \u{2014} \
                     call as {recv}.{method_name}() \
                     not {recv}:{method_name}()"
                ),
            });
        }

        hints
    }

    /// Resolve variable-context annotations for a runtime error.
    ///
    /// Looks up the definition and last-assignment source locations for
    /// the variable named in the error (if any).  Only runs on the error
    /// path, so has zero cost during normal execution.
    #[cold]
    fn resolve_var_context(&self, err: &VmError) -> Option<crate::error::VarContext> {
        let var = err.var_name()?;
        // Use the innermost Lua frame.
        let frame = self.frames.iter().rev().find_map(|cf| match cf {
            CallFrame::Lua(f) => Some(f),
            _ => None,
        })?;
        let definition = frame.definition_location(var);
        let last_assignment = frame.last_assignment_location(var);
        let is_implicit_self = frame.is_implicit_self(var);
        if definition.is_none() && last_assignment.is_none() {
            return None;
        }
        Some(crate::error::VarContext {
            definition,
            last_assignment,
            is_implicit_self,
        })
    }

    /// Write `values` into the topmost Lua frame at `return_dst`.
    fn write_return_values(&mut self, values: ValueVec, dst: usize, nresults: i32) {
        let caller = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return,
        };
        let n = if nresults < 0 {
            values.len()
        } else {
            nresults as usize
        };
        // Resize the register vector so that `Return { nresults: -1 }` uses
        // exactly `dst + n` as the upper bound.
        // When nresults < 0 (variable-count return), we also *truncate* any
        // stale values that were left behind from the call's argument area —
        // without this, a subsequent `Return { nresults: -1 }` would pick up
        // those old arg registers as extra return values.
        let needed = dst + n;
        caller.ensure_registers(needed);
        // With fixed-capacity registers, update reg_count instead of
        // resize/truncate.  The underlying slots already exist as Nil.
        if needed > caller.reg_count {
            caller.reg_count = needed;
        } else if nresults < 0 {
            caller.reg_count = needed;
        }
        // Clear padding slots to Nil before writing values: if the callee
        // returned fewer values than requested, slots [dst + values.len() .. dst + n)
        // may still hold stale data from the call setup (e.g. the table and
        // key used to resolve an indexed call like `os.clock()`), and those
        // must be nil per Lua's adjust-to-n semantics.
        let provided = values.len().min(n);
        for i in provided..n {
            write_reg(&mut caller.registers[dst + i], Value::Nil);
        }
        for (i, v) in values.into_iter().enumerate().take(n) {
            write_reg(&mut caller.registers[dst + i], v);
        }
    }

    /// Transfer return values directly from the callee's owned register vec
    /// into the caller frame, avoiding an intermediate `Vec<Value>` allocation.
    fn write_return_from_registers(
        &mut self,
        mut callee_regs: Box<[Value]>,
        callee_reg_count: usize,
        base: usize,
        nresults: i32,
        dst: usize,
        pending_nresults: i32,
    ) {
        let caller = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return,
        };
        // For known return counts, use nresults directly.
        // For variable returns (nresults < 0), use reg_count.
        let actual_returned = if nresults < 0 {
            callee_reg_count.saturating_sub(base)
        } else {
            nresults as usize
        };
        let n = if pending_nresults < 0 {
            if nresults < 0 {
                actual_returned
            } else {
                (nresults as usize).min(actual_returned)
            }
        } else {
            pending_nresults as usize
        };
        let needed = dst + n;
        caller.ensure_registers(needed);
        if needed > caller.reg_count {
            caller.reg_count = needed;
        } else if pending_nresults < 0 {
            caller.reg_count = needed;
        }
        let provided = actual_returned.min(n);
        // Move values directly from the callee register array.
        for i in 0..provided {
            let src_idx = base + i;
            let val = std::mem::replace(&mut callee_regs[src_idx], Value::Nil);
            write_reg(&mut caller.registers[dst + i], val);
        }
        // Nil-fill remaining slots.
        for i in provided..n {
            write_reg(&mut caller.registers[dst + i], Value::Nil);
        }
        recycle_registers(&mut self.register_pool, callee_regs);
    }

    /// Handle the metamethod fallback for a binary arithmetic operation that
    /// failed the fast path. Looks up __add/__sub/etc. on the operands and
    /// dispatches via Lua function, userdata, or returns the original error.
    #[cold]
    #[inline(never)]
    fn handle_binary_metamethod(
        &mut self,
        l: Value,
        r: Value,
        mm_name: &'static str,
        e: VmError,
        name: Option<crate::error::VarName>,
        dst: usize,
    ) -> Result<Option<Step>, VmError> {
        match get_arith_metamethod(&l, &r, mm_name.as_bytes(), &self.global) {
            Some(ArithMetamethod::Function(mm_fn)) => {
                self.dispatch_mm_or_yield(mm_fn, valuevec![l, r], 1, dst, false)
            }
            Some(ArithMetamethod::Userdata(ud)) => Ok(Some(self.dispatch_ud_mm(
                ud,
                mm_name,
                valuevec![l, r],
                dst,
            )?)),
            None => Err(e.with_name(name)),
        }
    }

    /// Handle the metamethod fallback for a unary operation that failed the
    /// fast path.
    #[cold]
    #[inline(never)]
    fn handle_unary_metamethod(
        &mut self,
        v: Value,
        mm_name: &'static str,
        e: VmError,
        name: Option<crate::error::VarName>,
        dst: usize,
    ) -> Result<Option<Step>, VmError> {
        match get_arith_metamethod(&v, &v, mm_name.as_bytes(), &self.global) {
            Some(ArithMetamethod::Function(mm_fn)) => {
                self.dispatch_mm_or_yield(mm_fn, valuevec![v.clone(), v], 1, dst, false)
            }
            Some(ArithMetamethod::Userdata(ud)) => Ok(Some(self.dispatch_ud_mm(
                ud,
                mm_name,
                valuevec![v.clone(), v],
                dst,
            )?)),
            None => Err(e.with_name(name)),
        }
    }

    /// Handle the metamethod fallback for a comparison operation.
    #[cold]
    #[inline(never)]
    fn handle_compare_metamethod(
        &mut self,
        l: Value,
        r: Value,
        mm_name: &'static str,
        e: VmError,
        lhs_name: Option<crate::error::VarName>,
        rhs_name: Option<crate::error::VarName>,
        dst: usize,
    ) -> Result<Option<Step>, VmError> {
        match get_arith_metamethod(&l, &r, mm_name.as_bytes(), &self.global) {
            Some(ArithMetamethod::Function(mm_fn)) => {
                self.dispatch_mm_or_yield(mm_fn, valuevec![l, r], 1, dst, true)
            }
            Some(ArithMetamethod::Userdata(ud)) => Ok(Some(self.dispatch_ud_mm(
                ud,
                mm_name,
                valuevec![l, r],
                dst,
            )?)),
            None => Err(e.with_comparison_names(lhs_name, rhs_name)),
        }
    }

    /// Shared helper: set return coordinates on the caller frame, dispatch a
    /// metamethod (Lua or native), and — if a native yield is needed — fill in
    /// the pending-call bookkeeping and return `Ok(Some(Step::Yield(...)))`.
    /// Returns `Ok(None)` when the metamethod was dispatched inline (Lua call
    /// frame pushed) and the main loop should simply continue.
    #[cold]
    #[inline(never)]
    fn dispatch_mm_or_yield(
        &mut self,
        mm_fn: crate::function::Function,
        args: ValueVec,
        nresults: i32,
        dst: usize,
        coerce_to_bool: bool,
    ) -> Result<Option<Step>, VmError> {
        if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
            c.return_dst = dst;
            c.pending_nresults = nresults;
        }
        match dispatch_metamethod(
            &mut self.frames,
            &mut self.register_pool,
            &self.global,
            &mut self.call_stack,
            self.parent_stack_len,
            mm_fn,
            args,
            nresults,
            dst,
            coerce_to_bool,
        )? {
            None => Ok(None),
            Some(fut) => {
                self.pending_kind = PendingKind::NativeCall;
                self.pending_nresults = nresults;
                self.pending_dst = dst;
                Ok(Some(Step::Yield(Box::pin(fut))))
            }
        }
    }

    /// Dispatch a userdata metamethod.  Always yields (returns
    /// `Ok(Step::Yield(...))`) because userdata dispatch is async.
    #[cold]
    #[inline(never)]
    fn dispatch_ud_mm(
        &mut self,
        ud: Arc<dyn Userdata + Send + Sync>,
        mm_name: &'static str,
        args: ValueVec,
        dst: usize,
    ) -> Result<Step, VmError> {
        let source_label = format!("=[{}]", ud.type_name());
        if let Some(CallFrame::Lua(caller)) = self.frames.last() {
            self.call_stack.set_top_call_pc(caller.pc.checked_sub(1));
        }
        let ctx = self.build_call_context(None);
        let fut = Arc::clone(&ud).dispatch(ctx, mm_name, args);
        self.pending_kind = PendingKind::NativeCall;
        self.pending_nresults = 1;
        self.pending_dst = dst;
        if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
            caller.return_dst = dst;
            caller.pending_nresults = 1;
        }
        let native_name = crate::byte_string::Bytes::from(mm_name.as_bytes());
        self.call_stack.push(StackFrame::Native {
            function_name: native_name.clone(),
        });
        self.frames.push(CallFrame::Native(NativeFrame {
            signature: Arc::new(FunctionSignature {
                name: native_name,
                source: crate::byte_string::Bytes::from(source_label),
                type_params: vec![],
                params: vec![],
                variadic: true,

                variadic_doc: None,
                arg_offset: 0,
                returns: None,
                lua_returns: None,
                line_defined: 0,
                last_line_defined: 0,
                num_upvalues: 0,
                has_runtime_types: false,
            }),
            call_site: None,
        }));
        Ok(Step::Yield(Box::pin(fut)))
    }

    /// Execute the Call opcode.
    #[inline(never)]
    fn exec_call(&mut self, word: u32) -> Result<CallResult, VmError> {
        let func = bytecode::get_a(word);
        let is_method_call = bytecode::get_k(word);
        let b = bytecode::get_b(word);
        let c = bytecode::get_c(word);
        let nargs: i32 = if b == 0 { -1 } else { (b - 1) as i32 };
        let nresults: i32 = if c == 0 { -1 } else { (c - 1) as i32 };
        let return_dst = func as usize;

        // SyncPlain fast path: borrow the register to extract just
        // the inner call Arc, avoiding a clone of the outer Value.
        // Hints are deferred to the error path.
        let sync_plain = {
            let frame = match self.frames.last() {
                Some(CallFrame::Lua(f)) => f,
                _ => return Ok(CallResult::Done),
            };
            let func_ref = frame.get_ref(func);
            let arg_start = func as usize + 1;
            let arg_end = if nargs < 0 {
                frame.reg_count
            } else {
                arg_start + nargs as usize
            };
            match func_ref {
                Value::Function(f) => match f.state() {
                    FunctionState::Native(nf) => match &nf.call {
                        crate::function::NativeCall::SyncPlain(c) => {
                            Some((Arc::clone(c), nf.signature.clone(), arg_start, arg_end))
                        }
                        _ => None,
                    },
                    _ => None,
                },
                _ => None,
            }
        };
        if let Some((call, callee_sig, arg_start, arg_end)) = sync_plain {
            let result = {
                let frame = match self.frames.last() {
                    Some(CallFrame::Lua(f)) => f,
                    _ => return Ok(CallResult::Done),
                };
                let arg_slice = &frame.registers[arg_start..arg_end];
                call(arg_slice)
            };
            match result {
                Ok(results) => {
                    if nresults == 1 && results.len() == 1 {
                        let val = results.into_iter().next().expect("len==1");
                        let Some(CallFrame::Lua(frame)) = self.frames.last_mut() else {
                            unreachable!("exec_call is only invoked from Lua opcode dispatch");
                        };
                        write_reg(&mut frame.registers[return_dst], val);
                    } else {
                        self.write_return_values(results, return_dst, nresults);
                    }
                    return Ok(CallResult::Done);
                }
                Err(e) => {
                    let frame = match self.frames.last_mut() {
                        Some(CallFrame::Lua(f)) => f,
                        _ => return Err(e),
                    };
                    let call_site = frame.proto.call_site_info.get(&(frame.pc - 1));
                    frame.last_call_is_method = is_method_call;
                    frame.last_call_dot_colon =
                        call_site.map(|i| (i.dot_colon_offset, i.dot_colon_len));
                    frame.last_call_receiver_offset = call_site.map(|i| i.receiver_offset);
                    frame.last_call_callee_sig = Some(callee_sig);
                    return Err(e);
                }
            }
        }

        // General path: extract func_val, set diagnostics, dispatch.
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(CallResult::Done),
        };
        let func_val = frame.get(func);
        let call_pc = frame.pc.saturating_sub(1);
        let (dot_colon_span, receiver_offset) = frame
            .proto
            .call_site_info
            .get(&call_pc)
            .map(|info| {
                (
                    (info.dot_colon_offset, info.dot_colon_len),
                    info.receiver_offset,
                )
            })
            .unzip();
        let arg_start = func as usize + 1;
        let arg_end = if nargs < 0 {
            frame.reg_count
        } else {
            arg_start + nargs as usize
        };
        // Record call-site hint info and callee signature on the caller
        // frame BEFORE dispatch so it's available if validate_args fails.
        let callee_sig: Option<Arc<FunctionSignature>> = match &func_val {
            Value::Function(f) => match f.state() {
                FunctionState::Lua(lf) => Some(lf.proto.signature.clone()),
                FunctionState::Native(nf) => Some(nf.signature.clone()),
            },
            _ => None,
        };
        frame.last_call_is_method = is_method_call;
        frame.last_call_dot_colon = dot_colon_span;
        frame.last_call_receiver_offset = receiver_offset;
        frame.last_call_callee_sig = callee_sig;

        self.dispatch_general_call(
            func_val, arg_start, arg_end, return_dst, nresults, func, call_pc,
        )
    }

    /// Dispatch an already-resolved `func_val` over a contiguous register
    /// range.  Handles all `Function` variants (Lua, native sync/async),
    /// `Table` `__call` metamethods, and the non-callable error case.
    ///
    /// `arg_start..arg_end` is the slice of the current Lua frame's
    /// registers holding the call arguments.  `return_dst` is the
    /// register index where results should be written.  `func_slot` is
    /// used only for `register_name` lookup in error messages (the
    /// register where the function value was loaded, if any — callers
    /// that resolve the function out-of-band can pass the receiver slot).
    /// `call_pc` is the bytecode address of the calling instruction (used
    /// for stack-trace `set_top_call_pc`).
    ///
    /// Caller is responsible for setting `frame.last_call_*` diagnostics
    /// before invoking this helper (so they're available if
    /// `validate_args` fails).
    fn dispatch_general_call(
        &mut self,
        func_val: Value,
        arg_start: usize,
        arg_end: usize,
        return_dst: usize,
        nresults: i32,
        func_slot: u8,
        call_pc: usize,
    ) -> Result<CallResult, VmError> {
        let frame_count = self.frames.len();
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(CallResult::Done),
        };
        match func_val {
            Value::Function(f) => match f.state() {
                FunctionState::Lua(lf) => {
                    if frame_count >= MAX_STACK_DEPTH {
                        return Err(VmError::StackOverflow);
                    }
                    let arg_slice = &frame.registers[arg_start..arg_end];
                    validate_args(&lf.proto.signature, arg_slice)?;
                    let mut pool = std::mem::take(&mut self.register_pool);
                    let new_frame = make_lua_frame_from_slice(
                        &mut pool,
                        lf.proto.clone(),
                        lf.upvalues.clone(),
                        arg_slice,
                    );
                    self.register_pool = pool;
                    frame.return_dst = return_dst;
                    frame.pending_nresults = nresults;
                    self.call_stack.set_top_call_pc(Some(call_pc));
                    self.call_stack.push(StackFrame::lua(
                        lf.proto.signature.clone(),
                        lf.proto.clone(),
                    ));
                    self.frames.push(CallFrame::Lua(new_frame));
                }
                FunctionState::Native(nf) => {
                    let arg_slice = &frame.registers[arg_start..arg_end];
                    match &nf.call {
                        crate::function::NativeCall::SyncPlain(call) => {
                            let call = Arc::clone(call);
                            let frame = match self.frames.last_mut() {
                                Some(CallFrame::Lua(f)) => f,
                                _ => return Ok(CallResult::Done),
                            };
                            let arg_slice = &frame.registers[arg_start..arg_end];
                            let results = call(arg_slice)?;
                            self.write_return_values(results, return_dst, nresults);
                        }
                        crate::function::NativeCall::SyncWithCtx(call) => {
                            let call = Arc::clone(call);
                            let native_name = nf.signature.name.clone();
                            self.call_stack.set_top_call_pc(Some(call_pc));
                            let ctx = CallContext::new(
                                self.global.clone(),
                                self.call_stack.clone(),
                                Some(native_name),
                            );
                            let frame = match self.frames.last_mut() {
                                Some(CallFrame::Lua(f)) => f,
                                _ => return Ok(CallResult::Done),
                            };
                            let arg_slice = &frame.registers[arg_start..arg_end];
                            let results = call(ctx, arg_slice)?;
                            self.write_return_values(results, return_dst, nresults);
                        }
                        crate::function::NativeCall::SyncWithLocals(call) => {
                            let call = Arc::clone(call);
                            let native_name = nf.signature.name.clone();
                            self.call_stack.set_top_call_pc(Some(call_pc));
                            let locals = self.build_frame_locals(native_name.clone());
                            let ctx = CallContext::new(
                                self.global.clone(),
                                self.call_stack.clone(),
                                Some(native_name),
                            );
                            let frame = match self.frames.last_mut() {
                                Some(CallFrame::Lua(f)) => f,
                                _ => return Ok(CallResult::Done),
                            };
                            let arg_slice = &frame.registers[arg_start..arg_end];
                            let results = call(ctx, locals, arg_slice)?;
                            self.write_return_values(results, return_dst, nresults);
                        }
                        crate::function::NativeCall::Async(call) => {
                            let args: ValueVec = arg_slice.into();
                            frame.return_dst = return_dst;
                            frame.pending_nresults = nresults;
                            self.call_stack.set_top_call_pc(Some(call_pc));
                            let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                            self.call_stack.push(StackFrame::Native {
                                function_name: nf.signature.name.clone(),
                            });
                            let fut = call(ctx, args);
                            self.pending_kind = PendingKind::NativeCall;
                            self.pending_nresults = nresults;
                            self.pending_dst = return_dst;
                            self.frames.push(CallFrame::Native(NativeFrame {
                                signature: nf.signature.clone(),
                                call_site: None,
                            }));
                            return Ok(CallResult::Yield(Step::Yield(fut)));
                        }
                        crate::function::NativeCall::AsyncWithLocals(call) => {
                            let args: ValueVec = arg_slice.into();
                            frame.return_dst = return_dst;
                            frame.pending_nresults = nresults;
                            self.call_stack.set_top_call_pc(Some(call_pc));
                            let locals = self.build_frame_locals(nf.signature.name.clone());
                            let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                            self.call_stack.push(StackFrame::Native {
                                function_name: nf.signature.name.clone(),
                            });
                            let fut = call(ctx, locals, args);
                            self.pending_kind = PendingKind::NativeCall;
                            self.pending_nresults = nresults;
                            self.pending_dst = return_dst;
                            self.frames.push(CallFrame::Native(NativeFrame {
                                signature: nf.signature.clone(),
                                call_site: None,
                            }));
                            return Ok(CallResult::Yield(Step::Yield(fut)));
                        }
                    }
                }
            },
            Value::Table(tab) => {
                let args: ValueVec = frame.registers[arg_start..arg_end].into();
                match tab.get_metamethod("__call") {
                    Some(Value::Function(mm_fn)) => {
                        let mut mm_args = valuevec![Value::Table(tab)];
                        mm_args.extend(args);
                        if let Some(step) =
                            self.dispatch_mm_or_yield(mm_fn, mm_args, nresults, return_dst, false)?
                        {
                            return Ok(CallResult::Yield(step));
                        }
                    }
                    _ => {
                        let name = self.frames.last().and_then(|cf| match cf {
                            CallFrame::Lua(f) => f.register_name(func_slot),
                            _ => None,
                        });
                        return Err(VmError::CallNonFunction {
                            type_name: "table",
                            name,
                        });
                    }
                }
            }
            other => {
                let name = self.frames.last().and_then(|cf| match cf {
                    CallFrame::Lua(f) => f.register_name(func_slot),
                    _ => None,
                });
                return Err(VmError::CallNonFunction {
                    type_name: other.type_name(),
                    name,
                });
            }
        }
        Ok(CallResult::FrameChanged)
    }

    /// Execute the Invoke opcode (fused method call).
    ///
    /// `R(A)` holds the receiver (and acts as self/arg-1).  Explicit args
    /// are at `R(A+1)`..`R(A+B-1)`.  The trailing `ExtraArg` word's `Ax`
    /// field gives the constant-pool index of the method name.
    ///
    /// Sync resolution paths (Userdata `index`, Table `raw_get` hits,
    /// `__index` table chains, sync string metatable lookups) call
    /// `dispatch_general_call` directly.  Async paths (Userdata async
    /// `__index`, `__index` function metamethods) stash a continuation
    /// in `self.pending_invoke`, dispatch the lookup with `R(A)` as the
    /// destination (clobbering the receiver), and yield.  When control
    /// returns to the caller frame, `step()`'s outer loop picks up the
    /// continuation, restores the receiver, and dispatches.
    #[inline(never)]
    fn exec_invoke(&mut self, word: u32) -> Result<CallResult, VmError> {
        let dst = bytecode::get_a(word);
        let b = bytecode::get_b(word);
        let c = bytecode::get_c(word);
        let nargs: i32 = if b == 0 { -1 } else { (b - 1) as i32 };
        let nresults: i32 = if c == 0 { -1 } else { (c - 1) as i32 };
        let return_dst = dst as usize;

        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(CallResult::Done),
        };
        let extra = frame.proto.code[frame.pc];
        frame.pc += 1;
        let method_const_idx = bytecode::get_ax(extra) as usize;
        let call_pc = frame.pc.saturating_sub(2);
        let receiver = frame.get(dst);
        let key = frame.proto.constants[method_const_idx].clone();
        let (dot_colon_span, receiver_offset) = frame
            .proto
            .call_site_info
            .get(&call_pc)
            .map(|info| {
                (
                    (info.dot_colon_offset, info.dot_colon_len),
                    info.receiver_offset,
                )
            })
            .unzip();

        // Userdata fast path: try `Userdata::invoke` (sync) and
        // `Userdata::invoke_async` (async) before falling through to the
        // `index`-then-call resolution.  Both bypass `Function`-value
        // materialisation; the sync path also bypasses
        // `dispatch_general_call`.
        if let Value::Userdata(ud) = &receiver {
            let arg_start = dst as usize;
            let arg_end = if nargs < 0 {
                frame.reg_count
            } else {
                arg_start + nargs as usize
            };
            let method_bytes: Option<&[u8]> = match &key {
                Value::String(s) => Some(s.as_ref()),
                _ => None,
            };
            if let Some(method_bytes) = method_bytes {
                // Sync fast path.
                let arg_slice = &frame.registers[arg_start..arg_end];
                if let Some(result) = ud.invoke(method_bytes, arg_slice) {
                    let values = result?;
                    let frame = match self.frames.last_mut() {
                        Some(CallFrame::Lua(f)) => f,
                        _ => return Ok(CallResult::Done),
                    };
                    frame.last_call_is_method = true;
                    frame.last_call_dot_colon = dot_colon_span;
                    frame.last_call_receiver_offset = receiver_offset;
                    frame.last_call_callee_sig = None;
                    // Single-value return fast path: skip
                    // `write_return_values` for the (very common) case of
                    // exactly one result going into one register.
                    if nresults == 1 && values.len() == 1 {
                        let val = values.into_iter().next().expect("len==1");
                        write_reg(&mut frame.registers[return_dst], val);
                        return Ok(CallResult::Done);
                    }
                    self.write_return_values(values, return_dst, nresults);
                    return Ok(CallResult::Done);
                }
                // Async fast path.
                let args: ValueVec = arg_slice.into();
                let ud_for_async = Arc::clone(ud);
                if let Some((sig, fut)) = ud_for_async.invoke_async(method_bytes, args) {
                    let frame = match self.frames.last_mut() {
                        Some(CallFrame::Lua(f)) => f,
                        _ => return Ok(CallResult::Done),
                    };
                    frame.return_dst = return_dst;
                    frame.pending_nresults = nresults;
                    frame.last_call_is_method = true;
                    frame.last_call_dot_colon = dot_colon_span;
                    frame.last_call_receiver_offset = receiver_offset;
                    frame.last_call_callee_sig = Some(sig.clone());
                    self.call_stack.set_top_call_pc(Some(call_pc));
                    self.call_stack.push(StackFrame::Native {
                        function_name: sig.name.clone(),
                    });
                    self.frames.push(CallFrame::Native(NativeFrame {
                        signature: sig,
                        call_site: None,
                    }));
                    self.pending_kind = PendingKind::NativeCall;
                    self.pending_nresults = nresults;
                    self.pending_dst = return_dst;
                    return Ok(CallResult::Yield(Step::Yield(fut)));
                }
            }
        }

        // Try sync resolution.  Returns Some(func_val) on hit, None when
        // the lookup needs to dispatch an async metamethod.
        let sync_func: Option<Value> = match &receiver {
            Value::Userdata(ud) => match ud.index(&key) {
                Some(result) => {
                    let values = result?;
                    Some(values.into_iter().next().unwrap_or(Value::Nil))
                }
                None => None,
            },
            Value::Table(tab) => {
                let v = tab
                    .raw_get(&key)
                    .map_err(|e| e.with_table_name(frame.register_name(dst)))?;
                if !v.is_nil() {
                    Some(v)
                } else {
                    match tab.get_metamethod("__index") {
                        None => Some(Value::Nil),
                        Some(Value::Table(idx_tab)) => match index_table_chain(idx_tab, &key)? {
                            IndexChainResult::Value(v) => Some(v),
                            IndexChainResult::Function(_, _) => None,
                        },
                        Some(Value::Function(_)) => None,
                        Some(_) => Some(Value::Nil),
                    }
                }
            }
            Value::String(_) => match self.global.get_string_metatable() {
                Some(mt) => {
                    let index_key = Value::string("__index");
                    let mm = mt.raw_get(&index_key).ok().filter(|v| !v.is_nil());
                    match mm {
                        Some(Value::Table(idx_tab)) => match index_table_chain(idx_tab, &key)? {
                            IndexChainResult::Value(v) => Some(v),
                            IndexChainResult::Function(_, _) => None,
                        },
                        Some(Value::Function(_)) => None,
                        _ => Some(Value::Nil),
                    }
                }
                None => {
                    return Err(VmError::IndexNonTable {
                        type_name: "string",
                        name: frame.register_name(dst),
                        key: displayable_key(&key),
                    });
                }
            },
            other => {
                return Err(VmError::IndexNonTable {
                    type_name: other.type_name(),
                    name: frame.register_name(dst),
                    key: displayable_key(&key),
                });
            }
        };

        // Sync path: dispatch the resolved function directly.
        if let Some(func_val) = sync_func {
            let arg_start = dst as usize;
            let arg_end = if nargs < 0 {
                frame.reg_count
            } else {
                arg_start + nargs as usize
            };
            let callee_sig: Option<Arc<FunctionSignature>> = match &func_val {
                Value::Function(f) => match f.state() {
                    FunctionState::Lua(lf) => Some(lf.proto.signature.clone()),
                    FunctionState::Native(nf) => Some(nf.signature.clone()),
                },
                _ => None,
            };
            frame.last_call_is_method = true;
            frame.last_call_dot_colon = dot_colon_span;
            frame.last_call_receiver_offset = receiver_offset;
            frame.last_call_callee_sig = callee_sig;
            return self.dispatch_general_call(
                func_val, arg_start, arg_end, return_dst, nresults, dst, call_pc,
            );
        }

        // Async path: stash continuation, dispatch metamethod with R(dst)
        // as destination, yield (or fall through for Lua-frame metamethods).
        let caller_frame_idx = self.frames.len() - 1;
        self.pending_invoke = Some(InvokeContinuation {
            dst,
            nargs,
            nresults,
            call_pc,
            caller_frame_idx,
            receiver: receiver.clone(),
            dot_colon_span,
            receiver_offset,
        });

        match receiver {
            Value::Userdata(ud) => {
                let mm_args = valuevec![Value::Userdata(Arc::clone(&ud)), key];
                let step = self.dispatch_ud_mm(ud, "__index", mm_args, dst as usize)?;
                // dispatch_ud_mm set pending_kind = NativeCall; override.
                self.pending_kind = PendingKind::InvokeAfterIndex;
                Ok(CallResult::Yield(step))
            }
            Value::Table(tab) => {
                let (mm_fn, owner) = match tab.get_metamethod("__index") {
                    Some(Value::Function(f)) => (f, tab.clone()),
                    Some(Value::Table(idx_tab)) => match index_table_chain(idx_tab, &key)? {
                        IndexChainResult::Function(f, owner) => (f, owner),
                        _ => unreachable!("sync path covers IndexChainResult::Value"),
                    },
                    _ => unreachable!("sync path covers missing/non-callable __index"),
                };
                let mm_args = valuevec![Value::Table(owner), key];
                match self.dispatch_mm_or_yield(mm_fn, mm_args, 1, dst as usize, false)? {
                    None => {
                        // Lua metamethod frame pushed; continuation fires on return.
                        Ok(CallResult::FrameChanged)
                    }
                    Some(step) => {
                        self.pending_kind = PendingKind::InvokeAfterIndex;
                        Ok(CallResult::Yield(step))
                    }
                }
            }
            Value::String(_) => {
                let mt = self
                    .global
                    .get_string_metatable()
                    .expect("sync path covers missing string metatable");
                let index_key = Value::string("__index");
                let mm = mt.raw_get(&index_key).ok().filter(|v| !v.is_nil());
                let (mm_fn, mm_args) = match mm {
                    Some(Value::Function(f)) => (f, valuevec![receiver.clone(), key]),
                    Some(Value::Table(idx_tab)) => match index_table_chain(idx_tab, &key)? {
                        IndexChainResult::Function(f, owner) => {
                            (f, valuevec![Value::Table(owner), key])
                        }
                        _ => unreachable!("sync path covers IndexChainResult::Value"),
                    },
                    _ => unreachable!("sync path covers missing/non-callable __index"),
                };
                match self.dispatch_mm_or_yield(mm_fn, mm_args, 1, dst as usize, false)? {
                    None => Ok(CallResult::FrameChanged),
                    Some(step) => {
                        self.pending_kind = PendingKind::InvokeAfterIndex;
                        Ok(CallResult::Yield(step))
                    }
                }
            }
            _ => unreachable!("sync path covers other receiver types as errors"),
        }
    }

    /// Resume an `Invoke` continuation: the metamethod or future has
    /// completed, leaving the resolved function value in `R(cont.dst)`.
    /// Restore the original receiver into `R(cont.dst)`, then dispatch
    /// the resolved function as a call with arg range `[cont.dst,
    /// cont.dst + cont.nargs)`.
    #[inline(never)]
    fn resume_invoke_continuation(&mut self) -> Result<CallResult, VmError> {
        let cont = self
            .pending_invoke
            .take()
            .expect("resume_invoke_continuation called with no pending invoke");
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(CallResult::Done),
        };
        // R(dst) currently holds the resolved function value (written
        // there by the metamethod's exec_return, or by
        // write_return_values after a native future resolved).  Swap
        // it out for the original receiver.
        let func_val = std::mem::replace(&mut frame.registers[cont.dst as usize], cont.receiver);
        let arg_start = cont.dst as usize;
        let arg_end = if cont.nargs < 0 {
            frame.reg_count
        } else {
            arg_start + cont.nargs as usize
        };
        let callee_sig: Option<Arc<FunctionSignature>> = match &func_val {
            Value::Function(f) => match f.state() {
                FunctionState::Lua(lf) => Some(lf.proto.signature.clone()),
                FunctionState::Native(nf) => Some(nf.signature.clone()),
            },
            _ => None,
        };
        frame.last_call_is_method = true;
        frame.last_call_dot_colon = cont.dot_colon_span;
        frame.last_call_receiver_offset = cont.receiver_offset;
        frame.last_call_callee_sig = callee_sig;
        self.dispatch_general_call(
            func_val,
            arg_start,
            arg_end,
            cont.dst as usize,
            cont.nresults,
            cont.dst,
            cont.call_pc,
        )
    }

    /// Execute the GenericForCall opcode.
    #[inline(never)]
    fn exec_generic_for_call(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let base = bytecode::get_a(word);
        let nresults_u8 = bytecode::get_b(word);
        let func_val = frame.get(base);
        let args = valuevec![frame.get(base + 1), frame.get(base + 2)];
        let return_dst = (base + 4) as usize;
        let nresults = nresults_u8 as i32;

        match func_val {
            Value::Function(f) => match f.state() {
                FunctionState::Lua(lf) => {
                    if self.frames.len() >= MAX_STACK_DEPTH {
                        return Err(VmError::StackOverflow);
                    }
                    validate_args(&lf.proto.signature, &args)?;
                    if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                        caller.return_dst = return_dst;
                        caller.pending_nresults = nresults;
                    }
                    let new_frame = make_lua_frame(
                        &mut self.register_pool,
                        lf.proto.clone(),
                        lf.upvalues.clone(),
                        args,
                    );
                    if let Some(CallFrame::Lua(caller)) = self.frames.last() {
                        self.call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                    }
                    self.call_stack.push(StackFrame::lua(
                        lf.proto.signature.clone(),
                        lf.proto.clone(),
                    ));
                    self.frames.push(CallFrame::Lua(new_frame));
                }
                FunctionState::Native(nf) => match &nf.call {
                    crate::function::NativeCall::SyncPlain(call) => {
                        let results = call(&args)?;
                        self.write_return_values(results, return_dst, nresults);
                    }
                    crate::function::NativeCall::SyncWithCtx(call) => {
                        if let Some(CallFrame::Lua(caller)) = self.frames.last() {
                            self.call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                        }
                        let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                        let results = call(ctx, &args)?;
                        self.write_return_values(results, return_dst, nresults);
                    }
                    crate::function::NativeCall::SyncWithLocals(call) => {
                        if let Some(CallFrame::Lua(caller)) = self.frames.last() {
                            self.call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                        }
                        let locals = self.build_frame_locals(nf.signature.name.clone());
                        let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                        let results = call(ctx, locals, &args)?;
                        self.write_return_values(results, return_dst, nresults);
                    }
                    crate::function::NativeCall::Async(call) => {
                        if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                            caller.return_dst = return_dst;
                            caller.pending_nresults = nresults;
                        }
                        if let Some(CallFrame::Lua(caller)) = self.frames.last() {
                            self.call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                        }
                        let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                        self.call_stack.push(StackFrame::Native {
                            function_name: nf.signature.name.clone(),
                        });
                        let fut = call(ctx, args);
                        self.pending_kind = PendingKind::NativeCall;
                        self.pending_nresults = nresults;
                        self.pending_dst = return_dst;
                        self.frames.push(CallFrame::Native(NativeFrame {
                            signature: nf.signature.clone(),
                            call_site: None,
                        }));
                        return Ok(Some(Step::Yield(fut)));
                    }
                    crate::function::NativeCall::AsyncWithLocals(call) => {
                        if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                            caller.return_dst = return_dst;
                            caller.pending_nresults = nresults;
                        }
                        if let Some(CallFrame::Lua(caller)) = self.frames.last() {
                            self.call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                        }
                        let locals = self.build_frame_locals(nf.signature.name.clone());
                        let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                        self.call_stack.push(StackFrame::Native {
                            function_name: nf.signature.name.clone(),
                        });
                        let fut = call(ctx, locals, args);
                        self.pending_kind = PendingKind::NativeCall;
                        self.pending_nresults = nresults;
                        self.pending_dst = return_dst;
                        self.frames.push(CallFrame::Native(NativeFrame {
                            signature: nf.signature.clone(),
                            call_site: None,
                        }));
                        return Ok(Some(Step::Yield(fut)));
                    }
                },
            },
            other => {
                return Err(VmError::CallNonFunction {
                    type_name: other.type_name(),
                    name: frame.register_name(base),
                });
            }
        }
        Ok(None)
    }

    /// Read `key` from `tab` with `__index` metamethod support and
    /// write the result into register `dst`.
    ///
    /// Returns `Ok(None)` if resolution completes synchronously.
    /// Returns `Ok(Some(step))` if a metamethod dispatch needs the VM
    /// to yield or push a Lua frame.  `table_name` is the optional
    /// originating variable name used to enrich error context (the
    /// register holding the table for `GetTable`, or e.g. `_ENV` for
    /// the global path).
    #[inline]
    fn get_in_table(
        &mut self,
        tab: crate::table::Table,
        key: Value,
        dst: u8,
        table_name: Option<crate::error::VarName>,
    ) -> Result<Option<Step>, VmError> {
        let v = tab
            .raw_get(&key)
            .map_err(|e| e.with_table_name(table_name))?;
        if !v.is_nil() {
            if let Some(CallFrame::Lua(f)) = self.frames.last_mut() {
                f.set(dst, v);
            }
            return Ok(None);
        }
        // Follow table-only __index chain first.  If the chain ends at
        // a function, fall through to function dispatch.
        let mm = tab.get_metamethod("__index");
        match mm {
            None => {
                if let Some(CallFrame::Lua(f)) = self.frames.last_mut() {
                    f.set(dst, Value::Nil);
                }
            }
            Some(Value::Table(idx_tab)) => match index_table_chain(idx_tab, &key)? {
                IndexChainResult::Value(v) => {
                    if let Some(CallFrame::Lua(f)) = self.frames.last_mut() {
                        f.set(dst, v);
                    }
                }
                IndexChainResult::Function(mm_fn, owner) => {
                    let mm_args = valuevec![Value::Table(owner), key];
                    if let Some(step) =
                        self.dispatch_mm_or_yield(mm_fn, mm_args, 1, dst as usize, false)?
                    {
                        return Ok(Some(step));
                    }
                }
            },
            Some(Value::Function(mm_fn)) => {
                let mm_args = valuevec![Value::Table(tab), key];
                if let Some(step) =
                    self.dispatch_mm_or_yield(mm_fn, mm_args, 1, dst as usize, false)?
                {
                    return Ok(Some(step));
                }
            }
            Some(_) => {
                // __index is neither table nor function.
                if let Some(CallFrame::Lua(f)) = self.frames.last_mut() {
                    f.set(dst, Value::Nil);
                }
            }
        }
        Ok(None)
    }

    /// Write `value` to `tab[key]` with `__newindex` metamethod
    /// support.
    ///
    /// Returns `Ok(None)` if resolution completes synchronously.
    /// Returns `Ok(Some(step))` if a metamethod dispatch needs the VM
    /// to yield or push a Lua frame.
    #[inline]
    fn set_in_table(
        &mut self,
        tab: crate::table::Table,
        key: Value,
        value: Value,
        table_name: Option<crate::error::VarName>,
    ) -> Result<Option<Step>, VmError> {
        // __newindex is only triggered when the key is absent.
        let existing = tab
            .raw_get(&key)
            .map_err(|e| e.with_table_name(table_name.clone()))?;
        if !existing.is_nil() {
            // Key already exists — raw write, no metamethod.
            tab.raw_set(key, value)
                .map_err(|e| e.with_table_name(table_name))?;
            return Ok(None);
        }
        let mm = tab.get_metamethod("__newindex");
        match mm {
            None => {
                tab.raw_set(key, value)
                    .map_err(|e| e.with_table_name(table_name))?;
            }
            Some(Value::Table(dst_tab)) => match newindex_table_chain(dst_tab, &key)? {
                NewindexChainResult::Table(target) => {
                    target
                        .raw_set(key, value)
                        .map_err(|e| e.with_table_name(table_name))?;
                }
                NewindexChainResult::Function(mm_fn, owner) => {
                    let mm_args = valuevec![Value::Table(owner), key, value];
                    if let Some(step) = self.dispatch_mm_or_yield(mm_fn, mm_args, 0, 0, false)? {
                        return Ok(Some(step));
                    }
                }
            },
            Some(Value::Function(mm_fn)) => {
                let mm_args = valuevec![Value::Table(tab), key, value];
                // __newindex result is discarded (0 results).
                if let Some(step) = self.dispatch_mm_or_yield(mm_fn, mm_args, 0, 0, false)? {
                    return Ok(Some(step));
                }
            }
            Some(_) => {
                // Unknown __newindex type: raw write.
                tab.raw_set(key, value)
                    .map_err(|e| e.with_table_name(table_name))?;
            }
        }
        Ok(None)
    }

    /// Execute the GetGlobal opcode — a free-name read, equivalent to
    /// `_ENV[name]` with `__index` metamethod support.  Raises
    /// `IndexNonTable` if `_ENV` has been bound to a non-table value.
    #[inline(never)]
    fn exec_get_global(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let dst = bytecode::get_a(word);
        let name = bytecode::get_bx(word) as usize;
        let key = frame.proto.constants[name].clone();
        let env_val = frame_env_value(frame);
        let env = match env_val {
            Value::Table(t) => t,
            _ => return Err(env_not_table_error(&env_val, &key)),
        };
        let table_name = match &key {
            Value::String(s) => std::str::from_utf8(s)
                .ok()
                .map(crate::error::VarName::global),
            _ => None,
        };
        self.get_in_table(env, key, dst, table_name)
    }

    /// Execute the SetGlobal opcode — a free-name write, equivalent
    /// to `_ENV[name] = value` with `__newindex` metamethod support.
    /// Raises `IndexNonTable` if `_ENV` is bound to a non-table value.
    #[inline(never)]
    fn exec_set_global(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let src = bytecode::get_a(word);
        let name = bytecode::get_bx(word) as usize;
        let key = frame.proto.constants[name].clone();
        let value = frame.get(src);
        let env_val = frame_env_value(frame);
        let env = match env_val {
            Value::Table(t) => t,
            _ => return Err(env_not_table_error(&env_val, &key)),
        };
        let table_name = match &key {
            Value::String(s) => std::str::from_utf8(s)
                .ok()
                .map(crate::error::VarName::global),
            _ => None,
        };
        self.set_in_table(env, key, value, table_name)
    }

    /// Execute the GetTable opcode.
    #[inline(never)]
    fn exec_get_table(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let (dst, table, key) = (
            bytecode::get_a(word),
            bytecode::get_b(word),
            bytecode::get_c(word),
        );
        let t = frame.get(table);
        let k = frame.get(key);
        match t {
            Value::Table(tab) => {
                let table_name = frame.register_name(table);
                return self.get_in_table(tab, k, dst, table_name);
            }
            Value::Userdata(ud) => {
                // Try the synchronous __index fast path first.
                if let Some(result) = ud.index(&k) {
                    let values = result?;
                    let v = values.into_iter().next().unwrap_or(Value::Nil);
                    let frame = match self.frames.last_mut() {
                        Some(CallFrame::Lua(f)) => f,
                        _ => return Ok(None),
                    };
                    frame.set(dst, v);
                    return Ok(None);
                }
                // Fall back to async dispatch.
                let args = valuevec![Value::Userdata(Arc::clone(&ud)), k];
                let d = dst as usize;
                return Ok(Some(self.dispatch_ud_mm(ud, "__index", args, d)?));
            }
            Value::String(_) => {
                // Consult the shared string metatable so that
                // method-call syntax like ("hello"):upper() works.
                if let Some(mt) = self.global.get_string_metatable() {
                    let index_key = Value::string("__index");
                    let mm = mt.raw_get(&index_key).ok().filter(|v| !v.is_nil());
                    match mm {
                        Some(Value::Table(idx_tab)) => match index_table_chain(idx_tab, &k)? {
                            IndexChainResult::Value(v) => {
                                frame.set(dst, v);
                            }
                            IndexChainResult::Function(mm_fn, owner) => {
                                let mm_args = valuevec![Value::Table(owner), k];
                                if let Some(step) = self.dispatch_mm_or_yield(
                                    mm_fn,
                                    mm_args,
                                    1,
                                    dst as usize,
                                    false,
                                )? {
                                    return Ok(Some(step));
                                }
                            }
                        },
                        Some(Value::Function(mm_fn)) => {
                            let mm_args = valuevec![t, k];
                            let d = dst as usize;
                            if let Some(step) =
                                self.dispatch_mm_or_yield(mm_fn, mm_args, 1, d, false)?
                            {
                                return Ok(Some(step));
                            }
                        }
                        _ => frame.set(dst, Value::Nil),
                    }
                } else {
                    return Err(VmError::IndexNonTable {
                        type_name: "string",
                        name: frame.register_name(table),
                        key: displayable_key(&k),
                    });
                }
            }
            other => {
                return Err(VmError::IndexNonTable {
                    type_name: other.type_name(),
                    name: frame.register_name(table),
                    key: displayable_key(&k),
                });
            }
        }
        Ok(None)
    }

    /// Execute the SetTable opcode.
    #[inline(never)]
    fn exec_set_table(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let (table, key, src) = (
            bytecode::get_a(word),
            bytecode::get_b(word),
            bytecode::get_c(word),
        );
        let k = frame.get(key);
        let v = frame.get(src);
        let table_slot = table;
        // Fast path: peek at the table register without cloning
        // to avoid Arc refcount overhead on every table write.
        {
            let t_ref = frame.get_ref(table);
            if let Value::Table(tab) = &*t_ref {
                if !tab.has_metatable() {
                    tab.raw_set(k, v)
                        .map_err(|e| e.with_table_name(frame.register_name(table_slot)))?;
                    return Ok(None);
                }
            }
        }
        // Slow path: clone the value and handle metamethods.
        let t = frame.get(table);
        match t {
            Value::Table(tab) => {
                let table_name = frame.register_name(table_slot);
                return self.set_in_table(tab, k, v, table_name);
            }
            Value::Userdata(ud) => {
                // Try the synchronous __newindex fast path first.
                if let Some(result) = ud.newindex(&k, &v) {
                    result?;
                    return Ok(None);
                }
                // Fall back to async dispatch.
                let args = valuevec![Value::Userdata(Arc::clone(&ud)), k, v];
                return Ok(Some(self.dispatch_ud_mm(ud, "__newindex", args, 0)?));
            }
            other => Err(VmError::IndexNonTable {
                type_name: other.type_name(),
                name: frame.register_name(table),
                key: displayable_key(&k),
            }),
        }
    }

    /// Execute the Concat opcode.
    #[inline(never)]
    fn exec_concat(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let dst = bytecode::get_a(word);
        let base = bytecode::get_b(word);
        let count = bytecode::get_c(word);
        // Collect all operand values up front.
        let vals: Vec<Value> = (0..count).map(|i| frame.get(base + i)).collect();
        // Try the fast path: all operands are strings or numbers.
        let mut buf = Vec::<u8>::new();
        let mut coerce_fail: Option<usize> = None;
        for (i, v) in vals.iter().enumerate() {
            match v {
                Value::String(s) => buf.extend_from_slice(s),
                Value::Integer(_) | Value::Float(_) => {
                    buf.extend_from_slice(v.to_string().as_bytes());
                }
                _ => {
                    coerce_fail = Some(i);
                    break;
                }
            }
        }
        if coerce_fail.is_none() {
            frame.set(dst, Value::String(crate::byte_string::Bytes::from(buf)));
        } else {
            // At least one operand isn't a string/number.
            // The compiler always emits count=2; support __concat for that case.
            let lhs = vals[0].clone();
            let rhs = vals[1].clone();
            match get_arith_metamethod(&lhs, &rhs, b"__concat", &self.global) {
                Some(ArithMetamethod::Function(mm_fn)) => {
                    let d = dst as usize;
                    if let Some(step) =
                        self.dispatch_mm_or_yield(mm_fn, valuevec![lhs, rhs], 1, d, false)?
                    {
                        return Ok(Some(step));
                    }
                }
                Some(ArithMetamethod::Userdata(ud)) => {
                    let d = dst as usize;
                    return Ok(Some(self.dispatch_ud_mm(
                        ud,
                        "__concat",
                        valuevec![lhs, rhs],
                        d,
                    )?));
                }
                None => {
                    let type_name = match coerce_fail.and_then(|i| vals.get(i)) {
                        Some(Value::Nil) => "nil",
                        Some(Value::Boolean(_)) => "boolean",
                        Some(Value::Table(_)) => "table",
                        Some(Value::Function(_)) => "function",
                        Some(Value::Userdata(_)) => "userdata",
                        _ => "value",
                    };
                    // fail_idx < count (u8) and base+count fits in u8
                    // (compiler invariant), so this won't overflow.
                    let fail_idx = coerce_fail.expect("inside coerce_fail.is_some() branch");
                    let fail_slot = base + fail_idx as u8;
                    return Err(VmError::ConcatenationError {
                        type_name,
                        name: frame.register_name(fail_slot),
                    });
                }
            }
        }
        Ok(None)
    }

    /// Execute a comparison opcode (Lt, Le, Gt, Ge).
    /// `swap` indicates whether operands are swapped for the comparison
    /// function (Gt uses compare_lt with swapped args, Ge uses compare_le
    /// with swapped args).
    #[inline(never)]
    fn exec_compare(
        &mut self,
        word: u32,
        compare_fn: fn(&Value, &Value) -> Result<bool, VmError>,
        mm_name: &'static str,
        swap: bool,
    ) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let (dst, lhs, rhs) = (
            bytecode::get_a(word),
            bytecode::get_b(word),
            bytecode::get_c(word),
        );
        let di = dst as usize;
        let li = lhs as usize;
        let ri = rhs as usize;
        if let (Value::Integer(a), Value::Integer(b)) = (&frame.registers[li], &frame.registers[ri])
        {
            let result = if swap {
                b < a || (mm_name == "__le" && b == a)
            } else {
                if mm_name == "__le" {
                    a <= b
                } else {
                    a < b
                }
            };
            write_reg(&mut frame.registers[di], Value::Boolean(result));
        } else {
            let l = frame.registers[li].clone();
            let r = frame.registers[ri].clone();
            let (cl, cr) = if swap { (&r, &l) } else { (&l, &r) };
            match compare_fn(cl, cr) {
                Ok(v) => {
                    write_reg(&mut frame.registers[di], Value::Boolean(v));
                }
                Err(e) => {
                    let names = (frame.register_name(lhs), frame.register_name(rhs));
                    let (ml, mr) = if swap { (r, l) } else { (l, r) };
                    if let Some(step) =
                        self.handle_compare_metamethod(ml, mr, mm_name, e, names.0, names.1, di)?
                    {
                        return Ok(Some(step));
                    }
                }
            }
        }
        Ok(None)
    }

    #[inline(never)]
    fn exec_return(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let base = bytecode::get_a(word) as usize;
        let b = bytecode::get_b(word);
        let nresults: i32 = if b == 0 { -1 } else { (b - 1) as i32 };
        let frame = match self.frames.last() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let coerce = frame.coerce_result_to_bool;

        // Pop the callee frame, close its upvalues, and take its registers.
        let mut callee = match self.frames.pop() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        self.call_stack.pop();
        let callee_rc = callee.reg_count;
        let mut callee_regs = callee.take_registers();

        if self.frames.is_empty() {
            // Top-level return — must build a Vec for the caller.
            let results: ValueVec = if coerce {
                let truthy = callee_regs
                    .get(base)
                    .map(|v| v.is_truthy())
                    .unwrap_or(false);
                valuevec![Value::Boolean(truthy)]
            } else {
                let end = if nresults < 0 {
                    callee_rc
                } else {
                    base + nresults as usize
                };
                callee_regs[base..end]
                    .iter_mut()
                    .map(|v| std::mem::replace(v, Value::Nil))
                    .collect()
            };
            recycle_registers(&mut self.register_pool, callee_regs);
            return Ok(Some(Step::Done(results)));
        }

        let (return_dst, pending_nresults) = match self.frames.last() {
            Some(CallFrame::Lua(f)) => (f.return_dst, f.pending_nresults),
            _ => (0, -1),
        };

        if coerce {
            let truthy = callee_regs
                .get(base)
                .map(|v| v.is_truthy())
                .unwrap_or(false);
            recycle_registers(&mut self.register_pool, callee_regs);
            self.write_return_values(
                valuevec![Value::Boolean(truthy)],
                return_dst,
                pending_nresults,
            );
        } else {
            self.write_return_from_registers(
                callee_regs,
                callee_rc,
                base,
                nresults,
                return_dst,
                pending_nresults,
            );
        }
        Ok(None)
    }

    #[inline(never)]
    fn exec_new_closure(&mut self, word: u32) {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return,
        };
        let dst = bytecode::get_a(word);
        let proto_idx = bytecode::get_bx(word) as u16;
        let child_proto = frame
            .proto
            .protos
            .get(proto_idx as usize)
            .cloned()
            .unwrap_or_else(|| frame.proto.clone());
        // Capture upvalues according to the proto's descriptors.
        let mut upvalues: Vec<UpvalueCell> = Vec::new();
        for desc in &child_proto.upvalues {
            if desc.in_stack {
                // Capture a register from this frame.  Re-use an
                // existing open cell for the slot if one exists, so
                // sibling closures share the same cell.
                let slot = desc.index;
                let cell =
                    if let Some((_, c)) = frame.open_upvalues.iter().find(|(s, _)| *s == slot) {
                        c.clone()
                    } else {
                        // Create an open upvalue that points directly into
                        // the frame's register array.  Reads and writes go
                        // through the raw pointer with zero routing overhead.
                        let ptr = &mut frame.registers[slot as usize] as *mut Value;
                        // Safety: the register array is a fixed-capacity
                        // Box<[Value]> that lives as long as the frame.
                        // The pointer remains valid until the frame's
                        // registers are reallocated (ensure_registers) or
                        // recycled (take_registers), both of which close
                        // or reopen all open upvalues first.
                        let cell = Arc::new(unsafe { UpvalueInner::new_open(ptr) });
                        frame.open_upvalues.push((slot, cell.clone()));
                        cell
                    };
                upvalues.push(cell);
            } else {
                // Capture one of this frame's own upvalue cells.
                upvalues.push(
                    frame
                        .upvalues
                        .get(desc.index as usize)
                        .cloned()
                        .unwrap_or_else(|| Arc::new(UpvalueInner::new_closed(Value::Nil))),
                );
            }
        }
        let func = Function::lua(child_proto, upvalues);
        self.global.track_function(&func);
        frame.set(dst, Value::Function(func));
    }

    #[inline(never)]
    fn exec_set_list(&mut self, word: u32) -> Result<(), VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(()),
        };
        let table = bytecode::get_a(word);
        let src_base = bytecode::get_b(word);
        let c = bytecode::get_c(word);
        // Read ExtraArg for array_start constant index.
        let extra = frame.proto.code[frame.pc];
        frame.pc += 1;
        let array_start_idx = bytecode::get_ax(extra) as usize;
        let t = match frame.get(table) {
            Value::Table(t) => t,
            other => {
                return Err(VmError::IndexNonTable {
                    type_name: other.type_name(),
                    name: None,
                    key: None,
                });
            }
        };
        let base_idx = match &frame.proto.constants[array_start_idx] {
            Value::Integer(i) => *i,
            _ => 1,
        };
        // c==0 means "all from src_base to top"; c>0 means c-1 values.
        let n = if c == 0 {
            frame.reg_count.saturating_sub(src_base as usize)
        } else {
            (c - 1) as usize
        };
        for i in 0..n {
            let v = frame
                .registers
                .get(src_base as usize + i)
                .cloned()
                .unwrap_or(Value::Nil);
            t.raw_set(Value::Integer(base_idx + i as i64), v)?;
        }
        Ok(())
    }

    #[inline(never)]
    fn exec_tostring(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let dst = bytecode::get_a(word);
        let src = bytecode::get_b(word);
        let val = frame.get(src);
        if let Some(sv) = val.to_string_value() {
            if dst != src || !matches!(val, Value::String(_)) {
                frame.set(dst, sv);
            }
        } else {
            match &val {
                Value::Table(t) => {
                    if let Some(Value::Function(mm)) = t.get_metamethod("__tostring") {
                        let d = dst as usize;
                        if let Some(step) =
                            self.dispatch_mm_or_yield(mm, valuevec![val], 1, d, false)?
                        {
                            return Ok(Some(step));
                        }
                    } else {
                        let frame = match self.frames.last_mut() {
                            Some(CallFrame::Lua(f)) => f,
                            _ => return Ok(None),
                        };
                        frame.set(
                            dst,
                            Value::String(crate::byte_string::Bytes::from(val.to_string())),
                        );
                    }
                }
                Value::Userdata(ud) => {
                    let d = dst as usize;
                    let args = valuevec![Value::Userdata(Arc::clone(ud))];
                    return Ok(Some(self.dispatch_ud_mm(
                        Arc::clone(ud),
                        "__tostring",
                        args,
                        d,
                    )?));
                }
                _ => unreachable!(),
            }
        }
        Ok(None)
    }

    #[inline(never)]
    fn exec_len(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let dst = bytecode::get_a(word);
        let src = bytecode::get_b(word);
        let v = frame.get(src);
        match &v {
            Value::String(s) => {
                let n = s.len() as i64;
                frame.set(dst, Value::Integer(n));
            }
            Value::Table(tab) => {
                // Check __len before falling back to raw_len.
                match tab.get_metamethod("__len") {
                    None => {
                        let n = tab.raw_len();
                        frame.set(dst, Value::Integer(n));
                    }
                    Some(Value::Function(mm_fn)) => {
                        let mm_args = valuevec![v];
                        let d = dst as usize;
                        if let Some(step) =
                            self.dispatch_mm_or_yield(mm_fn, mm_args, 1, d, false)?
                        {
                            return Ok(Some(step));
                        }
                    }
                    Some(_) => {
                        let n = tab.raw_len();
                        frame.set(dst, Value::Integer(n));
                    }
                }
            }
            Value::Userdata(ud) => {
                let ud_arc = Arc::clone(ud);
                let args = valuevec![v];
                let d = dst as usize;
                return Ok(Some(self.dispatch_ud_mm(ud_arc, "__len", args, d)?));
            }
            _ => {
                return Err(VmError::LengthNonTableOrString {
                    type_name: v.type_name(),
                    name: frame.register_name(src),
                });
            }
        }
        Ok(None)
    }

    #[inline(never)]
    fn exec_eq(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let (dst, lhs, rhs) = (
            bytecode::get_a(word),
            bytecode::get_b(word),
            bytecode::get_c(word),
        );
        // Fast path: compare registers directly without cloning for
        // primitive types.
        let di = dst as usize;
        let li = lhs as usize;
        let ri = rhs as usize;
        let result = match (&frame.registers[li], &frame.registers[ri]) {
            (Value::Integer(a), Value::Integer(b)) => Some(a == b),
            (Value::Float(a), Value::Float(b)) => Some(a == b),
            (Value::Boolean(a), Value::Boolean(b)) => Some(a == b),
            (Value::Nil, Value::Nil) => Some(true),
            _ => None,
        };
        if let Some(eq) = result {
            write_reg(&mut frame.registers[di], Value::Boolean(eq));
            return Ok(None);
        }
        let l = frame.get(lhs);
        let r = frame.get(rhs);
        if l == r {
            frame.set(dst, Value::Boolean(true));
            return Ok(None);
        }
        // Table __eq: try lhs's metatable first, then rhs's.
        let mm = match (&l, &r) {
            (Value::Table(lt), Value::Table(rt)) => lt
                .get_metamethod("__eq")
                .or_else(|| rt.get_metamethod("__eq")),
            _ => None,
        };
        if let Some(Value::Function(mm_fn)) = mm {
            let d = dst as usize;
            if let Some(step) = self.dispatch_mm_or_yield(mm_fn, valuevec![l, r], 1, d, true)? {
                return Ok(Some(step));
            }
            return Ok(None);
        }
        // Userdata __eq: Lua 5.4 only fires `__eq` when both
        // operands are the same kind (both userdata here).  Pick
        // the first operand whose type implements `__eq`.  Missing
        // metamethods on both sides fall back to rawequal-false
        // per the spec; we don't error.
        if let (Value::Userdata(lu), Value::Userdata(ru)) = (&l, &r) {
            let ud = if lu.has_metamethod("__eq") {
                Some(Arc::clone(lu))
            } else if ru.has_metamethod("__eq") {
                Some(Arc::clone(ru))
            } else {
                None
            };
            if let Some(ud) = ud {
                let d = dst as usize;
                return Ok(Some(self.dispatch_ud_mm(ud, "__eq", valuevec![l, r], d)?));
            }
        }
        frame.set(dst, Value::Boolean(false));
        Ok(None)
    }

    fn step(&mut self) -> Result<Step, VmError> {
        // Outer loop: re-entered after frame-changing operations (calls,
        // returns, metamethods).  The inner dispatch loop runs with a
        // cached frame reference, avoiding the `self.frames.last_mut()`
        // lookup on every opcode.
        'outer: loop {
            // If an `Invoke` continuation is pending and control has
            // returned to the caller frame (the metamethod or future has
            // finished), resume it now — the resolved function is in
            // `R(cont.dst)` and we need to perform the actual call.
            if let Some(cont) = self.pending_invoke.as_ref() {
                if self.frames.len() == cont.caller_frame_idx + 1 {
                    match self.resume_invoke_continuation()? {
                        CallResult::Done | CallResult::FrameChanged => continue 'outer,
                        CallResult::Yield(step) => return Ok(step),
                    }
                }
            }

            let frame = match self.frames.last_mut() {
                None => return Ok(Step::Done(valuevec![])),
                Some(CallFrame::Native(_)) => {
                    // Should not happen: native frames are only present while
                    // pending is Some.
                    self.frames.pop();
                    self.call_stack.pop();
                    continue;
                }
                Some(CallFrame::Lua(f)) => f,
            };

            // Inner dispatch loop: hot opcodes stay here without
            // re-fetching the frame reference.
            loop {
                // The compiler guarantees every chunk ends with a Return
                // opcode, so we can fetch without a bounds guard.
                let word = frame.proto.code[frame.pc];
                frame.pc += 1;

                macro_rules! binary_op_with_metamethod {
                    ($dst:expr, $lhs:expr, $rhs:expr, $op:ident, $mm:literal, $err_name:expr) => {{
                        let l = frame.get($lhs);
                        let r = frame.get($rhs);
                        match l.$op(&r) {
                            Ok(v) => frame.set($dst, v),
                            Err(e) => {
                                let name = $err_name;
                                let d = $dst as usize;
                                if let Some(step) =
                                    self.handle_binary_metamethod(l, r, $mm, e, name, d)?
                                {
                                    return Ok(step);
                                }
                                continue 'outer;
                            }
                        }
                    }};
                }

                macro_rules! int_fast_binary_op {
                    ($dst:expr, $lhs:expr, $rhs:expr, |$a:ident, $b:ident| $int_expr:expr, $op:ident, $mm:literal, $err_name:expr) => {{
                        let di = $dst as usize;
                        let li = $lhs as usize;
                        let ri = $rhs as usize;
                        if let (Value::Integer($a), Value::Integer($b)) =
                            (&frame.registers[li], &frame.registers[ri])
                        {
                            let result = $int_expr;
                            write_reg(&mut frame.registers[di], Value::Integer(result));
                        } else {
                            let l = frame.registers[li].clone();
                            let r = frame.registers[ri].clone();
                            match l.$op(&r) {
                                Ok(v) => {
                                    write_reg(&mut frame.registers[di], v);
                                }
                                Err(e) => {
                                    let name = $err_name;
                                    if let Some(step) =
                                        self.handle_binary_metamethod(l, r, $mm, e, name, di)?
                                    {
                                        return Ok(step);
                                    }
                                    continue 'outer;
                                }
                            }
                        }
                    }};
                }

                macro_rules! unary_op_with_metamethod {
                    ($dst:expr, $src:expr, $op:ident, $mm:literal, $err_name:expr) => {{
                        let v = frame.get($src);
                        match v.$op() {
                            Ok(result) => frame.set($dst, result),
                            Err(e) => {
                                let name = $err_name;
                                let d = $dst as usize;
                                if let Some(step) =
                                    self.handle_unary_metamethod(v, $mm, e, name, d)?
                                {
                                    return Ok(step);
                                }
                                continue 'outer;
                            }
                        }
                    }};
                }

                match bytecode::get_opcode(word) {
                    OpCode::LoadNil => {
                        let dst = bytecode::get_a(word);
                        frame.set(dst, Value::Nil);
                    }
                    OpCode::LoadBool => {
                        let dst = bytecode::get_a(word);
                        let value = bytecode::get_b(word) != 0;
                        frame.set(dst, Value::Boolean(value));
                    }
                    OpCode::LoadK => {
                        let dst = bytecode::get_a(word);
                        let idx = bytecode::get_bx(word) as usize;
                        let c = frame.proto.constants[idx].clone();
                        frame.set(dst, c);
                    }
                    OpCode::Move => {
                        let dst = bytecode::get_a(word);
                        let src = bytecode::get_b(word);
                        let di = dst as usize;
                        let si = src as usize;
                        if di == si {
                            continue;
                        }
                        let (left, right) = if di < si {
                            let (l, r) = frame.registers.split_at_mut(si);
                            (&mut l[di], &r[0])
                        } else {
                            let (l, r) = frame.registers.split_at_mut(di);
                            (&mut r[0], &l[si])
                        };
                        copy_reg(left, right);
                    }
                    OpCode::GetGlobal => {
                        // Fast path: env table has no metatable — raw
                        // lookup, no Arc clone, no metamethod check.
                        // Matches the pre-`__index` behaviour for the
                        // common case where `_G` is unmodified.
                        let dst = bytecode::get_a(word);
                        let name = bytecode::get_bx(word) as usize;
                        if let Some(idx) = frame.proto.env_upvalue_idx {
                            if let Some(cell) = frame.upvalues.get(idx as usize) {
                                // SAFETY: see `frame_env`; the cell
                                // is alive for the frame's lifetime.
                                let env_val = unsafe { cell.read() };
                                if let Value::Table(tab) = env_val {
                                    if !tab.has_metatable() {
                                        let key = &frame.proto.constants[name];
                                        let v = tab.raw_get(key).unwrap_or(Value::Nil);
                                        frame.set(dst, v);
                                        continue;
                                    }
                                }
                            }
                        }
                        let _ = frame;
                        if let Some(step) = self.exec_get_global(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::SetGlobal => {
                        // Fast path mirroring `SetTable`: env has no
                        // metatable, raw write.
                        let src = bytecode::get_a(word);
                        let name = bytecode::get_bx(word) as usize;
                        if let Some(idx) = frame.proto.env_upvalue_idx {
                            if let Some(cell) = frame.upvalues.get(idx as usize) {
                                // SAFETY: see `frame_env`.
                                let env_val = unsafe { cell.read() };
                                if let Value::Table(tab) = env_val {
                                    if !tab.has_metatable() {
                                        let key = frame.proto.constants[name].clone();
                                        let v = frame.get(src);
                                        tab.raw_set(key, v).ok();
                                        continue;
                                    }
                                }
                            }
                        }
                        let _ = frame;
                        if let Some(step) = self.exec_set_global(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::Jump => {
                        let offset = bytecode::get_sj(word);
                        apply_offset(&mut frame.pc, offset);
                    }
                    OpCode::BranchFalse => {
                        let src = bytecode::get_a(word);
                        let offset = bytecode::get_sbx(word);
                        if !frame.get(src).is_truthy() {
                            apply_offset(&mut frame.pc, offset);
                        }
                    }
                    OpCode::BranchTrue => {
                        let src = bytecode::get_a(word);
                        let offset = bytecode::get_sbx(word);
                        if frame.get(src).is_truthy() {
                            apply_offset(&mut frame.pc, offset);
                        }
                    }

                    // Arithmetic & bitwise ops
                    OpCode::Add => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        int_fast_binary_op!(
                            dst,
                            lhs,
                            rhs,
                            |a, b| a.wrapping_add(*b),
                            arith_add,
                            "__add",
                            frame.arith_error_name(lhs, rhs)
                        );
                    }
                    OpCode::Sub => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        int_fast_binary_op!(
                            dst,
                            lhs,
                            rhs,
                            |a, b| a.wrapping_sub(*b),
                            arith_sub,
                            "__sub",
                            frame.arith_error_name(lhs, rhs)
                        );
                    }
                    OpCode::Mul => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        int_fast_binary_op!(
                            dst,
                            lhs,
                            rhs,
                            |a, b| a.wrapping_mul(*b),
                            arith_mul,
                            "__mul",
                            frame.arith_error_name(lhs, rhs)
                        );
                    }
                    OpCode::Div => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        binary_op_with_metamethod!(
                            dst,
                            lhs,
                            rhs,
                            arith_div,
                            "__div",
                            frame.arith_error_name(lhs, rhs)
                        );
                    }
                    OpCode::IDiv => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        binary_op_with_metamethod!(
                            dst,
                            lhs,
                            rhs,
                            arith_idiv,
                            "__idiv",
                            frame.arith_error_name(lhs, rhs)
                        );
                    }
                    OpCode::Mod => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        binary_op_with_metamethod!(
                            dst,
                            lhs,
                            rhs,
                            arith_mod,
                            "__mod",
                            frame.arith_error_name(lhs, rhs)
                        );
                    }
                    OpCode::Pow => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        binary_op_with_metamethod!(
                            dst,
                            lhs,
                            rhs,
                            arith_pow,
                            "__pow",
                            frame.arith_error_name(lhs, rhs)
                        );
                    }
                    OpCode::Neg => {
                        let (dst, src) = (bytecode::get_a(word), bytecode::get_b(word));
                        unary_op_with_metamethod!(
                            dst,
                            src,
                            arith_neg,
                            "__unm",
                            frame.register_name(src)
                        );
                    }
                    OpCode::BAnd => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        int_fast_binary_op!(
                            dst,
                            lhs,
                            rhs,
                            |a, b| a & b,
                            arith_band,
                            "__band",
                            frame.bitwise_error_name(lhs, rhs)
                        );
                    }
                    OpCode::BOr => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        int_fast_binary_op!(
                            dst,
                            lhs,
                            rhs,
                            |a, b| a | b,
                            arith_bor,
                            "__bor",
                            frame.bitwise_error_name(lhs, rhs)
                        );
                    }
                    OpCode::BXor => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        int_fast_binary_op!(
                            dst,
                            lhs,
                            rhs,
                            |a, b| a ^ b,
                            arith_bxor,
                            "__bxor",
                            frame.bitwise_error_name(lhs, rhs)
                        );
                    }
                    OpCode::BNot => {
                        let (dst, src) = (bytecode::get_a(word), bytecode::get_b(word));
                        unary_op_with_metamethod!(
                            dst,
                            src,
                            arith_bnot,
                            "__bnot",
                            frame.register_name(src)
                        );
                    }

                    OpCode::Not => {
                        let (dst, src) = (bytecode::get_a(word), bytecode::get_b(word));
                        let v = !frame.get(src).is_truthy();
                        frame.set(dst, Value::Boolean(v));
                    }

                    // Comparison
                    OpCode::Eq => {
                        let _ = frame;
                        if let Some(step) = self.exec_eq(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::Ne => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        let v = frame.get(lhs) != frame.get(rhs);
                        frame.set(dst, Value::Boolean(v));
                    }
                    OpCode::Lt => {
                        let _ = frame;
                        if let Some(step) = self.exec_compare(word, compare_lt, "__lt", false)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::Le => {
                        let _ = frame;
                        if let Some(step) = self.exec_compare(word, compare_le, "__le", false)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::Gt => {
                        let _ = frame;
                        if let Some(step) = self.exec_compare(word, compare_lt, "__lt", true)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::Ge => {
                        let _ = frame;
                        if let Some(step) = self.exec_compare(word, compare_le, "__le", true)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }

                    // Numeric for
                    OpCode::ForPrep => {
                        let base = bytecode::get_a(word);
                        let exit_offset = bytecode::get_sbx(word);
                        let limit = base + 1;
                        let step = base + 2;
                        if for_prep(frame, base, limit, step)? {
                            apply_offset(&mut frame.pc, exit_offset);
                        }
                    }
                    OpCode::ForStep => {
                        let base = bytecode::get_a(word);
                        let body_offset = bytecode::get_sbx(word);
                        let limit = base + 1;
                        let step = base + 2;
                        if for_step(frame, base, limit, step)? {
                            apply_offset(&mut frame.pc, body_offset);
                        }
                    }

                    // Generic for
                    OpCode::GenericForCall => {
                        let _ = frame;
                        if let Some(step) = self.exec_generic_for_call(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::GenericForCheck => {
                        let base = bytecode::get_a(word);
                        let exit_offset = bytecode::get_sbx(word);
                        let vars = base + 4;
                        let control = base + 2;
                        let first_var = frame.get(vars);
                        if first_var.is_nil() {
                            apply_offset(&mut frame.pc, exit_offset);
                        } else {
                            frame.set(control, first_var);
                        }
                    }

                    // Function call
                    OpCode::Call => {
                        let _ = frame;
                        match self.exec_call(word)? {
                            CallResult::Done | CallResult::FrameChanged => {}
                            CallResult::Yield(step) => return Ok(step),
                        }
                        continue 'outer;
                    }

                    // Fused method call (obj:method(args))
                    OpCode::Invoke => {
                        let _ = frame;
                        match self.exec_invoke(word)? {
                            CallResult::Done | CallResult::FrameChanged => {}
                            CallResult::Yield(step) => return Ok(step),
                        }
                        continue 'outer;
                    }

                    OpCode::Return => {
                        let _ = frame;
                        if let Some(done) = self.exec_return(word)? {
                            return Ok(done);
                        }
                        continue 'outer;
                    }

                    OpCode::CollectGarbage => {
                        self.global.collect_cycles();
                    }

                    OpCode::GetUpval => {
                        let dst = bytecode::get_a(word);
                        let upval = bytecode::get_b(word);
                        let val = frame
                            .upvalues
                            .get(upval as usize)
                            // Safety: upvalue cells on a running frame are
                            // either open (pointing into a live ancestor
                            // frame) or closed.
                            .map(|cell| unsafe { cell.read() })
                            .unwrap_or(Value::Nil);
                        frame.set(dst, val);
                    }
                    OpCode::SetUpval => {
                        let upval = bytecode::get_a(word);
                        let src = bytecode::get_b(word);
                        let val = frame.get(src);
                        if let Some(cell) = frame.upvalues.get(upval as usize) {
                            // Safety: upvalue cells on a running frame are
                            // either open (pointing into a live ancestor
                            // frame) or closed.
                            unsafe { cell.write(val) };
                        }
                    }

                    OpCode::GetTable => {
                        let (dst, table, key) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word) as usize,
                            bytecode::get_c(word) as usize,
                        );
                        if let Value::Table(tab) = &frame.registers[table] {
                            if !tab.has_metatable() {
                                let k = &frame.registers[key];
                                let v = tab.raw_get(k).map_err(|e| {
                                    e.with_table_name(frame.register_name(table as u8))
                                })?;
                                frame.set(dst, v);
                                continue;
                            }
                        }
                        let _ = frame;
                        if let Some(step) = self.exec_get_table(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::SetTable => {
                        let (table, key, src) = (
                            bytecode::get_a(word) as usize,
                            bytecode::get_b(word) as usize,
                            bytecode::get_c(word) as usize,
                        );
                        if let Value::Table(tab) = &frame.registers[table] {
                            if !tab.has_metatable() {
                                let k = frame.registers[key].clone();
                                let v = frame.registers[src].clone();
                                tab.raw_set(k, v).map_err(|e| {
                                    e.with_table_name(frame.register_name(table as u8))
                                })?;
                                continue;
                            }
                        }
                        let _ = frame;
                        if let Some(step) = self.exec_set_table(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::NewTable => {
                        let dst = bytecode::get_a(word);
                        let t = Table::new();
                        self.global.track_table(&t);
                        frame.set(dst, Value::Table(t));
                    }
                    OpCode::SetList => {
                        let _ = frame;
                        self.exec_set_list(word)?;
                        continue 'outer;
                    }
                    OpCode::NewClosure => {
                        let _ = frame;
                        self.exec_new_closure(word);
                        continue 'outer;
                    }
                    OpCode::Concat => {
                        let _ = frame;
                        if let Some(step) = self.exec_concat(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::ToString => {
                        let _ = frame;
                        if let Some(step) = self.exec_tostring(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::CloseVar => {
                        let slot = bytecode::get_a(word);
                        let val = frame.get(slot);
                        // Nil the slot immediately to prevent double-close.
                        frame.set(slot, Value::Nil);
                        if let Some(fut) = close_future(val, &self.global, self.call_stack.clone())
                        {
                            self.pending_kind = PendingKind::CloseVar;
                            return Ok(Step::Yield(fut));
                        }
                    }
                    OpCode::CloseUpvalues => {
                        if !frame.open_upvalues.is_empty() {
                            let from = bytecode::get_a(word);
                            let mut i = 0;
                            while i < frame.open_upvalues.len() {
                                if frame.open_upvalues[i].0 >= from {
                                    let (_slot, cell) = frame.open_upvalues.swap_remove(i);
                                    // Safety: the frame's register array is still
                                    // alive, so any Open pointer is valid. Closing
                                    // copies the pointed-to value into the cell,
                                    // converting Open(*mut Value) → Closed(Value),
                                    // so the closure retains the per-iteration
                                    // snapshot of the variable.
                                    unsafe { cell.close() };
                                } else {
                                    i += 1;
                                }
                            }
                        }
                    }
                    // Labels are no-ops at runtime.
                    OpCode::Label => {}
                    // Goto must have been resolved to Jump during compilation.
                    OpCode::Goto => {
                        return Err(VmError::ArithmeticOnNonNumber {
                            type_name: "unresolved Goto in bytecode (compiler bug)",
                            name: None,
                        });
                    }
                    OpCode::Len => {
                        let _ = frame;
                        if let Some(step) = self.exec_len(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::Vararg => {
                        let dst = bytecode::get_a(word);
                        let b = bytecode::get_b(word);
                        let nresults: i16 = if b == 0 { -1 } else { (b - 1) as i16 };
                        let varargs = frame.varargs.clone();
                        if nresults < 0 {
                            // Expand all varargs and update reg_count so
                            // that `Return { nresults: -1 }` and
                            // `Call { nargs: -1 }` see the right count.
                            let n = varargs.len();
                            let new_len = dst as usize + n;
                            frame.ensure_registers(new_len);
                            frame.reg_count = new_len;
                            for (i, v) in varargs.into_iter().enumerate() {
                                frame.registers[dst as usize + i] = v;
                            }
                        } else {
                            for i in 0..nresults as usize {
                                let v = varargs.get(i).cloned().unwrap_or(Value::Nil);
                                frame.set(dst + i as u8, v);
                            }
                        }
                    }
                    OpCode::ExtraArg => {
                        // ExtraArg is consumed inline by SetList; reaching it
                        // standalone is a compiler bug.
                    }
                }
            } // inner dispatch loop
        }
    }
}

// ---------------------------------------------------------------------------
// Task (public)
// ---------------------------------------------------------------------------

pub struct Task {
    inner: TaskInner,
}

impl Task {
    /// Gracefully cancel a partially-polled task.
    ///
    /// Collects every live `<close>` variable from all Lua frames (in
    /// innermost-first order), calls their `__close` handlers, and returns.
    /// Any error produced by a `__close` handler is silently discarded; the
    /// original cancellation takes priority.
    ///
    /// Call this instead of dropping the `Task` when the host has abandoned
    /// execution mid-flight (e.g. due to a timeout), so that to-be-closed
    /// resources are cleaned up correctly.
    pub async fn dispose(mut self) {
        // Safety: `self` is owned and never moved before the await
        // below; pinning to the stack is sound.
        let pinned = unsafe { Pin::new_unchecked(&mut self) };
        pinned.begin_dispose();
        // Drive the unwind loop (dispatches __close handlers), then discard
        // the final error — it is either the synthetic cancel error above or
        // the original error that triggered an already-in-progress unwind.
        let _ = self.await;
    }

    /// Begin graceful cancellation of a partially-polled task.
    ///
    /// Mirrors [`Self::dispose`] but operates on a pinned mutable
    /// reference instead of consuming `self`.  Use this when the
    /// task is being driven by an outer future (e.g. inside a
    /// `tokio::select!`) that needs to keep polling it after
    /// signalling cancellation, so that `<close>` / `__close`
    /// handlers run to completion.
    ///
    /// After calling this, continuing to `.await` (or otherwise
    /// poll) the same `Task` drives the unwind loop and resolves
    /// with whatever (discardable) error the unwind produces.
    /// Idempotent: calling twice has no additional effect.
    pub fn begin_dispose(self: Pin<&mut Self>) {
        // Safety: only mutates fields of `inner` in place; never
        // moves the `Task` or any pinned sub-field.
        let inner = unsafe { &mut self.get_unchecked_mut().inner };
        if inner.unwind_error.is_none() {
            // Drop any pending async operation (blocking native, in-flight
            // __close, etc.) and collect live <close> locals from all frames.
            inner.pending = None;
            inner.begin_unwind(VmError::LuaError {
                display: "task cancelled".to_owned(),
                value: Value::Nil,
            });
        }
    }

    /// Create a new top-level task.
    ///
    /// When `func` is a Lua closure whose proto declares an `_ENV`
    /// upvalue (typical for any chunk that performs a free-name
    /// access), the upvalue list may be missing the env cell —
    /// `Function::lua` doesn't know about `GlobalEnv` and so leaves
    /// it absent.  This method synthesises a closed cell pointing at
    /// `global._G` for that case, so embedders can use the simple
    /// `bc.into_function()` + `Task::new(env, ...)`
    /// pattern without thinking about `_ENV` plumbing.  Closures
    /// constructed via `Function::lua_with_env` (or that already have
    /// the env cell from `NewClosure` propagation) are run
    /// unmodified.
    pub fn new(global: GlobalEnv, func: Function, args: ValueVec) -> Self {
        Self::new_inner(global, func, args, CallStack::new())
    }

    /// Create a task that inherits a parent call stack.  Used by
    /// `CallContext::call_function` so that nested native→Lua calls appear
    /// in stack traces with the full outer context prepended.
    pub fn new_with_parent(
        global: GlobalEnv,
        func: Function,
        args: ValueVec,
        parent_stack: CallStack,
    ) -> Self {
        Self::new_inner(global, func, args, parent_stack)
    }

    fn new_inner(
        global: GlobalEnv,
        func: Function,
        args: ValueVec,
        parent_stack: CallStack,
    ) -> Self {
        let parent_stack_len = parent_stack.len();
        match func.state() {
            FunctionState::Lua(lf) => {
                let validation_err = validate_args(&lf.proto.signature, &args).err();
                let mut pool = Vec::new();
                let mut frame =
                    make_lua_frame(&mut pool, lf.proto.clone(), lf.upvalues.clone(), args);
                ensure_env_upvalue(&mut frame, &global);
                let unwind_error = validation_err.map(|err| {
                    let (error, peeled) = err.peel_attributions();
                    let hints = peeled
                        .hints
                        .into_iter()
                        .map(|m| crate::error::Hint {
                            location: None,
                            message: m,
                        })
                        .collect();
                    RuntimeError {
                        error,
                        call_stack: parent_stack.to_vec(),
                        var_context: None,
                        source_text: lf.proto.source_text.clone(),
                        hints,
                        arg_position: peeled.arg_position,
                    }
                });
                // Push initial Lua frame onto persistent stack.
                let mut call_stack = parent_stack;
                call_stack.push(StackFrame::lua(
                    lf.proto.signature.clone(),
                    lf.proto.clone(),
                ));
                Task {
                    inner: TaskInner {
                        global,
                        frames: vec![CallFrame::Lua(frame)],
                        call_stack,
                        parent_stack_len,
                        pending: None,
                        pending_kind: PendingKind::NativeCall,
                        pending_nresults: -1,
                        pending_dst: 0,
                        unwind_error,
                        unwind_close_vals: Vec::new(),
                        register_pool: pool,
                        pending_invoke: None,
                    },
                }
            }
            FunctionState::Native(nf) => {
                // No Lua frames yet; build a context with the inherited parent
                // stack plus this native's own name.
                let mut call_stack = parent_stack;
                call_stack.push(StackFrame::Native {
                    function_name: nf.signature.name.clone(),
                });
                let build_ctx = || {
                    CallContext::new(
                        global.clone(),
                        call_stack.clone(),
                        Some(nf.signature.name.clone()),
                    )
                };
                let fut: BoxFuture<'static, Result<ValueVec, VmError>> = match &nf.call {
                    crate::function::NativeCall::SyncPlain(call) => {
                        let result = call(&args);
                        Box::pin(async move { result })
                    }
                    crate::function::NativeCall::SyncWithCtx(call) => {
                        let result = call(build_ctx(), &args);
                        Box::pin(async move { result })
                    }
                    crate::function::NativeCall::SyncWithLocals(call) => {
                        let locals = FrameLocals::new(call_stack.to_vec());
                        let result = call(build_ctx(), locals, &args);
                        Box::pin(async move { result })
                    }
                    crate::function::NativeCall::Async(call) => call(build_ctx(), args),
                    crate::function::NativeCall::AsyncWithLocals(call) => {
                        let locals = FrameLocals::new(call_stack.to_vec());
                        call(build_ctx(), locals, args)
                    }
                };
                Task {
                    inner: TaskInner {
                        global,
                        frames: vec![CallFrame::Native(NativeFrame {
                            signature: nf.signature.clone(),
                            call_site: None,
                        })],
                        call_stack,
                        parent_stack_len,
                        pending: Some(fut),
                        pending_kind: PendingKind::NativeCall,
                        pending_nresults: -1,
                        pending_dst: 0,
                        unwind_error: None,
                        unwind_close_vals: Vec::new(),
                        register_pool: Vec::new(),
                        pending_invoke: None,
                    },
                }
            }
        }
    }
}

impl std::future::Future for Task {
    type Output = Result<ValueVec, RuntimeError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            // ---------------------------------------------------------------
            // Poll any pending async operation.
            // ---------------------------------------------------------------
            if let Some(fut) = &mut self.inner.pending {
                match fut.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(result) => {
                        self.inner.pending = None;
                        match result {
                            Ok(values) => {
                                match self.inner.pending_kind {
                                    PendingKind::NativeCall => {
                                        self.inner.frames.pop();
                                        self.inner.call_stack.pop();
                                        if self.inner.frames.is_empty() {
                                            return Poll::Ready(Ok(values));
                                        }
                                        let dst = self.inner.pending_dst;
                                        let nresults = self.inner.pending_nresults;
                                        // Single-value return fast path:
                                        // common after async userdata methods
                                        // like `msg:get_data()`.
                                        if nresults == 1 && values.len() == 1 {
                                            let val = values.into_iter().next().expect("len==1");
                                            if let Some(CallFrame::Lua(frame)) =
                                                self.inner.frames.last_mut()
                                            {
                                                write_reg(&mut frame.registers[dst], val);
                                            }
                                        } else {
                                            self.inner.write_return_values(values, dst, nresults);
                                        }
                                    }
                                    PendingKind::InvokeAfterIndex => {
                                        self.inner.frames.pop();
                                        self.inner.call_stack.pop();
                                        if self.inner.frames.is_empty() {
                                            return Poll::Ready(Ok(values));
                                        }
                                        // Deliver the resolved function value
                                        // into `R(cont.dst)`.  The dispatch
                                        // loop's outer iteration picks up the
                                        // continuation from there.
                                        let dst = self
                                            .inner
                                            .pending_invoke
                                            .as_ref()
                                            .expect("InvokeAfterIndex must have continuation")
                                            .dst
                                            as usize;
                                        self.inner.write_return_values(values, dst, 1);
                                    }
                                    PendingKind::CloseVar | PendingKind::UnwindClose => {
                                        // __close results are discarded.
                                    }
                                }
                            }
                            Err(e) => {
                                match self.inner.pending_kind {
                                    PendingKind::NativeCall | PendingKind::InvokeAfterIndex => {
                                        // A native call failed — start unwinding.
                                        // Discard any pending Invoke continuation
                                        // since the lookup didn't deliver a value.
                                        self.inner.frames.pop();
                                        self.inner.call_stack.pop();
                                        self.inner.pending_invoke = None;
                                        self.inner.begin_unwind(e);
                                    }
                                    PendingKind::CloseVar => {
                                        // __close error during normal exit — start
                                        // unwinding with this error.
                                        self.inner.begin_unwind(e);
                                    }
                                    PendingKind::UnwindClose => {
                                        // __close error during unwind — discard
                                        // (original error takes priority).
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ---------------------------------------------------------------
            // Error-path `<close>` unwind loop.
            // ---------------------------------------------------------------
            if self.inner.unwind_error.is_some() {
                match self.inner.unwind_close_vals.pop() {
                    Some(val) => {
                        if let Some(fut) =
                            close_future(val, &self.inner.global, self.inner.call_stack.clone())
                        {
                            self.inner.pending = Some(fut);
                            self.inner.pending_kind = PendingKind::UnwindClose;
                            // Loop to poll the new future immediately.
                            continue;
                        }
                        // No __close handler — skip and try next.
                        continue;
                    }
                    None => {
                        // All __close calls complete; return the original error.
                        return Poll::Ready(Err(self
                            .inner
                            .unwind_error
                            .take()
                            .expect("unwind_error set")));
                    }
                }
            }

            // ---------------------------------------------------------------
            // Normal step execution.
            // ---------------------------------------------------------------
            match self.inner.step() {
                Ok(Step::Done(v)) => return Poll::Ready(Ok(v)),
                Ok(Step::Yield(fut)) => {
                    self.inner.pending = Some(fut);
                    // Loop to poll the new future immediately.
                }
                Err(e) => {
                    // Start the error-unwind sequence.
                    self.inner.begin_unwind(e);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the first `__mm` metamethod found on `lhs` (checked first) or `rhs`.
/// Only tables can have metamethods; non-table operands are skipped.
/// Result of walking a table-only `__newindex` chain.
enum NewindexChainResult {
    /// Chain ended at a table with no `__newindex` (or key already exists).
    /// The caller should raw-write into this table.
    Table(crate::table::Table),
    /// Chain ended at a function `__newindex` that the caller must dispatch.
    /// Contains `(function, table_that_owns_it)` for the metamethod args.
    Function(Function, crate::table::Table),
}

/// Follow the `__newindex` chain for purely-table metamethods.
///
/// `__newindex` fires only when the key is absent. If the chain reaches a
/// table where the key already exists, that table is returned for a raw
/// write. If a function `__newindex` is encountered, it is returned for
/// the caller to dispatch.
fn newindex_table_chain(
    mut table: crate::table::Table,
    key: &Value,
) -> Result<NewindexChainResult, VmError> {
    for _ in 0..crate::METAMETHOD_CHAIN_LIMIT {
        let existing = table.raw_get(key)?;
        if !existing.is_nil() {
            return Ok(NewindexChainResult::Table(table));
        }
        match table.get_metamethod("__newindex") {
            None => return Ok(NewindexChainResult::Table(table)),
            Some(Value::Table(next)) => table = next,
            Some(Value::Function(f)) => {
                return Ok(NewindexChainResult::Function(f, table));
            }
            Some(_) => return Ok(NewindexChainResult::Table(table)),
        }
    }
    Err(VmError::LuaError {
        display: "'__newindex' chain too long".to_owned(),
        value: Value::string("'__newindex' chain too long"),
    }
    .with_hint(
        "the `__newindex` metamethod chain hit the recursion guard; \
         this usually means a metatable cycle, or a `__newindex` \
         that always delegates to another table whose own \
         `__newindex` keeps redirecting",
    ))
}

/// Result of looking up an arithmetic/bitwise metamethod on either operand.
enum ArithMetamethod {
    /// Found a Lua function in a table's metatable.
    Function(Function),
    /// One of the operands is a userdata that may handle this metamethod
    /// via its async `dispatch` method.
    Userdata(Arc<dyn crate::userdata::Userdata + Send + Sync>),
}

/// Look up an arithmetic metamethod (`__add`, `__sub`, …) on either operand.
///
/// Checks table metatables first (synchronous), then falls back to userdata
/// operands (which require async dispatch by the caller).
fn get_arith_metamethod(
    lhs: &Value,
    rhs: &Value,
    event: &[u8],
    global: &GlobalEnv,
) -> Option<ArithMetamethod> {
    if let Value::Table(t) = lhs {
        if let Some(Value::Function(f)) = t.get_metamethod(event) {
            return Some(ArithMetamethod::Function(f));
        }
    }
    if let Value::Table(t) = rhs {
        if let Some(Value::Function(f)) = t.get_metamethod(event) {
            return Some(ArithMetamethod::Function(f));
        }
    }
    if let Value::Userdata(u) = lhs {
        return Some(ArithMetamethod::Userdata(Arc::clone(u)));
    }
    if let Value::Userdata(u) = rhs {
        return Some(ArithMetamethod::Userdata(Arc::clone(u)));
    }
    // Consult the shared string metatable for `Value::String`
    // operands (Lua 5.4 §6.4): user code can install bitwise and
    // arithmetic metamethods on the string type via
    // `getmetatable("").__band = ...` etc.
    if matches!(lhs, Value::String(_)) || matches!(rhs, Value::String(_)) {
        if let Some(mt) = global.get_string_metatable() {
            let key = Value::string(event);
            if let Ok(Value::Function(f)) = mt.raw_get(&key) {
                return Some(ArithMetamethod::Function(f));
            }
        }
    }
    None
}

/// Result of walking a table-only `__index` chain.
enum IndexChainResult {
    /// Found a non-nil value (or chain ended with no metamethod → nil).
    Value(Value),
    /// Chain ended at a function `__index` that the caller must dispatch
    /// asynchronously.  Contains `(function, table_that_owns_it)` so the
    /// caller can build the metamethod args `(table, key)`.
    Function(Function, crate::table::Table),
}

/// Follow the `__index` chain for purely-table metamethods.
///
/// Returns [`IndexChainResult::Value`] when the chain resolves to a
/// concrete value (including `Nil`), or [`IndexChainResult::Function`]
/// when the chain ends at a function `__index` that must be dispatched
/// by the caller.
fn index_table_chain(
    mut table: crate::table::Table,
    key: &Value,
) -> Result<IndexChainResult, VmError> {
    for _ in 0..crate::METAMETHOD_CHAIN_LIMIT {
        let v = table.raw_get(key)?;
        if !v.is_nil() {
            return Ok(IndexChainResult::Value(v));
        }
        match table.get_metamethod("__index") {
            None => return Ok(IndexChainResult::Value(Value::Nil)),
            Some(Value::Table(next)) => table = next,
            Some(Value::Function(f)) => {
                return Ok(IndexChainResult::Function(f, table));
            }
            Some(_) => return Ok(IndexChainResult::Value(Value::Nil)),
        }
    }
    Err(VmError::LuaError {
        display: "'__index' chain too long".to_owned(),
        value: Value::string("'__index' chain too long"),
    }
    .with_hint(
        "the `__index` metamethod chain hit the recursion guard; \
         this usually means a metatable cycle, or a `__index` \
         that always returns another table whose own `__index` \
         keeps redirecting",
    ))
}

/// Dispatch a synchronous-or-async metamethod call.  If `mm_fn` is a Lua
/// function, pushes a new frame onto `frames` and returns `None`.  If it is
/// a native, returns the future to yield on.
///
/// The caller is responsible for having already set `return_dst` and
/// `pending_nresults` on the top Lua frame before calling this.
///
/// `coerce_to_bool`: when `true` the first result is converted to
/// `Value::Boolean` before being written back to the caller.  Used by
/// comparison metamethods (`__eq`, `__lt`, `__le`) so the instruction result
/// is always a strict Lua boolean.
fn dispatch_metamethod(
    frames: &mut Vec<CallFrame>,
    register_pool: &mut Vec<Box<[Value]>>,
    global: &crate::global_env::GlobalEnv,
    call_stack: &mut CallStack,
    parent_stack_len: usize,
    mm_fn: crate::function::Function,
    args: ValueVec,
    _pending_nresults: i32,
    _pending_dst: usize,
    coerce_to_bool: bool,
) -> Result<Option<futures::future::BoxFuture<'static, Result<ValueVec, VmError>>>, VmError> {
    match mm_fn.state() {
        FunctionState::Lua(lf) => {
            validate_args(&lf.proto.signature, &args)?;
            let mut new_frame =
                make_lua_frame(register_pool, lf.proto.clone(), lf.upvalues.clone(), args);
            new_frame.coerce_result_to_bool = coerce_to_bool;
            if let Some(CallFrame::Lua(caller)) = frames.last() {
                call_stack.set_top_call_pc(caller.pc.checked_sub(1));
            }
            call_stack.push(StackFrame::lua(
                lf.proto.signature.clone(),
                lf.proto.clone(),
            ));
            frames.push(CallFrame::Lua(new_frame));
            Ok(None)
        }
        FunctionState::Native(nf) => {
            let native_name = Some(nf.signature.name.clone());
            match &nf.call {
                crate::function::NativeCall::SyncPlain(call) => {
                    let mut results = call(&args)?;
                    if coerce_to_bool {
                        let b = results.first().map(|v| v.is_truthy()).unwrap_or(false);
                        results = valuevec![Value::Boolean(b)];
                    }
                    // Write results back into the caller frame directly.
                    if let Some(CallFrame::Lua(caller)) = frames.last_mut() {
                        let dst = caller.return_dst;
                        let nr = caller.pending_nresults;
                        let rlen = results.len();
                        let count = if nr < 0 { rlen } else { nr as usize };
                        for (i, v) in results.into_iter().take(count).enumerate() {
                            caller.set((dst + i) as u8, v);
                        }
                        for i in rlen..count {
                            caller.set((dst + i) as u8, Value::Nil);
                        }
                    }
                    Ok(None)
                }
                crate::function::NativeCall::SyncWithCtx(call) => {
                    if let Some(CallFrame::Lua(caller)) = frames.last() {
                        call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                    }
                    let ctx =
                        CallContext::new(global.clone(), call_stack.clone(), native_name.clone());
                    let mut results = call(ctx, &args)?;
                    if coerce_to_bool {
                        let b = results.first().map(|v| v.is_truthy()).unwrap_or(false);
                        results = valuevec![Value::Boolean(b)];
                    }
                    // Write results back into the caller frame directly.
                    if let Some(CallFrame::Lua(caller)) = frames.last_mut() {
                        let dst = caller.return_dst;
                        let nr = caller.pending_nresults;
                        let rlen = results.len();
                        let count = if nr < 0 { rlen } else { nr as usize };
                        for (i, v) in results.into_iter().take(count).enumerate() {
                            caller.set((dst + i) as u8, v);
                        }
                        for i in rlen..count {
                            caller.set((dst + i) as u8, Value::Nil);
                        }
                    }
                    Ok(None)
                }
                crate::function::NativeCall::SyncWithLocals(call) => {
                    if let Some(CallFrame::Lua(caller)) = frames.last() {
                        call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                    }
                    let locals = build_frame_locals_from(
                        frames,
                        call_stack,
                        parent_stack_len,
                        nf.signature.name.clone(),
                    );
                    let ctx =
                        CallContext::new(global.clone(), call_stack.clone(), native_name.clone());
                    let mut results = call(ctx, locals, &args)?;
                    if coerce_to_bool {
                        let b = results.first().map(|v| v.is_truthy()).unwrap_or(false);
                        results = valuevec![Value::Boolean(b)];
                    }
                    if let Some(CallFrame::Lua(caller)) = frames.last_mut() {
                        let dst = caller.return_dst;
                        let nr = caller.pending_nresults;
                        let rlen = results.len();
                        let count = if nr < 0 { rlen } else { nr as usize };
                        for (i, v) in results.into_iter().take(count).enumerate() {
                            caller.set((dst + i) as u8, v);
                        }
                        for i in rlen..count {
                            caller.set((dst + i) as u8, Value::Nil);
                        }
                    }
                    Ok(None)
                }
                crate::function::NativeCall::Async(call) => {
                    if let Some(CallFrame::Lua(caller)) = frames.last() {
                        call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                    }
                    let ctx = CallContext::new(global.clone(), call_stack.clone(), native_name);
                    call_stack.push(StackFrame::Native {
                        function_name: nf.signature.name.clone(),
                    });
                    let raw_fut = call(ctx, args);
                    let fut: futures::future::BoxFuture<'static, Result<ValueVec, VmError>> =
                        if coerce_to_bool {
                            Box::pin(async move {
                                let results = raw_fut.await?;
                                let b = results.first().map(|v| v.is_truthy()).unwrap_or(false);
                                Ok(valuevec![Value::Boolean(b)])
                            })
                        } else {
                            raw_fut
                        };
                    frames.push(CallFrame::Native(NativeFrame {
                        signature: nf.signature.clone(),
                        call_site: None,
                    }));
                    Ok(Some(fut))
                }
                crate::function::NativeCall::AsyncWithLocals(call) => {
                    if let Some(CallFrame::Lua(caller)) = frames.last() {
                        call_stack.set_top_call_pc(caller.pc.checked_sub(1));
                    }
                    let locals = build_frame_locals_from(
                        frames,
                        call_stack,
                        parent_stack_len,
                        nf.signature.name.clone(),
                    );
                    let ctx = CallContext::new(global.clone(), call_stack.clone(), native_name);
                    call_stack.push(StackFrame::Native {
                        function_name: nf.signature.name.clone(),
                    });
                    let raw_fut = call(ctx, locals, args);
                    let fut: futures::future::BoxFuture<'static, Result<ValueVec, VmError>> =
                        if coerce_to_bool {
                            Box::pin(async move {
                                let results = raw_fut.await?;
                                let b = results.first().map(|v| v.is_truthy()).unwrap_or(false);
                                Ok(valuevec![Value::Boolean(b)])
                            })
                        } else {
                            raw_fut
                        };
                    frames.push(CallFrame::Native(NativeFrame {
                        signature: nf.signature.clone(),
                        call_site: None,
                    }));
                    Ok(Some(fut))
                }
            }
        }
    }
}

fn build_frame_locals_from(
    frames: &[CallFrame],
    call_stack: &CallStack,
    parent_stack_len: usize,
    native_name: crate::byte_string::Bytes,
) -> FrameLocals {
    let mut result: Vec<StackFrame> = call_stack
        .to_vec()
        .into_iter()
        .take(parent_stack_len)
        .collect();
    for cf in frames {
        let f = match cf {
            CallFrame::Lua(f) => f,
            CallFrame::Native(_) => continue,
        };
        let locals: Vec<(crate::byte_string::Bytes, Value)> = f
            .proto
            .locals
            .iter()
            .filter(|l| l.start_pc <= f.pc && f.pc < l.end_pc)
            .map(|l| (l.name.clone(), f.get(l.slot)))
            .collect();
        result.push(StackFrame::Lua {
            function: f.proto.signature.clone(),
            proto: f.proto.clone(),
            call_pc: f.pc.checked_sub(1),
            locals,
            last_call_is_method: f.last_call_is_method,
            last_call_dot_colon: f.last_call_dot_colon,
            last_call_receiver_offset: f.last_call_receiver_offset,
            last_call_callee_sig: f.last_call_callee_sig.clone(),
        });
    }
    result.push(StackFrame::Native {
        function_name: native_name,
    });
    FrameLocals::new(result)
}

/// Collect all live `<close>` values from every Lua frame, nil their slots
/// to prevent double-closing, and return the values in the order they should
/// be dispatched: outermost frame first, earliest-declared first within each
/// frame.  Callers pop from the end of the returned `Vec` to process
/// innermost-frame / last-declared values first (Lua LIFO semantics).
fn collect_close_vals(frames: &mut Vec<CallFrame>) -> Vec<Value> {
    let mut vals: Vec<Value> = Vec::new();
    for frame in frames.iter_mut() {
        let f = match frame {
            CallFrame::Lua(f) => f,
            CallFrame::Native(_) => continue,
        };
        // Collect (slot, val) pairs first to avoid borrow conflict between
        // iterating `proto.locals` and mutating frame registers.
        let to_close: Vec<(u8, Value)> = f
            .proto
            .locals
            .iter()
            .filter(|ld| ld.attr == LocalAttr::Close && f.pc >= ld.start_pc)
            .map(|ld| (ld.slot, f.get(ld.slot)))
            .filter(|(_, v)| !v.is_nil())
            .collect();
        for (slot, val) in to_close {
            f.set(slot, Value::Nil);
            vals.push(val);
        }
    }
    vals
}

/// Build a future that calls `__close` on `val`, or `None` if `val` does not
/// have a `__close` handler.  Used for both the normal `CloseVar` path and
/// the error-unwind path.
fn close_future(
    val: Value,
    global: &GlobalEnv,
    call_stack: CallStack,
) -> Option<BoxFuture<'static, Result<ValueVec, VmError>>> {
    match val {
        Value::Userdata(ud) => {
            let ud_arg = ud.clone();
            let ctx = CallContext::new(global.clone(), call_stack, Some("__close".into()));
            Some(ud.dispatch(ctx, "__close", valuevec![Value::Userdata(ud_arg)]))
        }
        Value::Table(ref t) => {
            if let Some(Value::Function(mm)) = t.get_metamethod("__close") {
                // Run the __close metamethod as a nested task so we can
                // handle both Lua and native implementations.
                let task = Task::new_with_parent(global.clone(), mm, valuevec![val], call_stack);
                Some(Box::pin(async move {
                    // Ignore result and error — the original error propagates.
                    let _ = task.await;
                    Ok(valuevec![])
                }))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Write a value into a register slot, skipping the drop of the old
/// value when it is a primitive (Integer, Float, Boolean, Nil).
#[inline(always)]
fn write_reg(slot: &mut Value, val: Value) {
    if slot.is_copy() {
        // SAFETY: the old value is a primitive with no heap resources,
        // so skipping Drop is safe.  We overwrite it in place.
        unsafe { std::ptr::write(slot, val) };
    } else {
        *slot = val;
    }
}

/// Copy one register slot to another, skipping both Clone and Drop
/// when both source and destination are primitives.
#[inline(always)]
fn copy_reg(dst: &mut Value, src: &Value) {
    if dst.is_copy() && src.is_copy() {
        // SAFETY: both values are primitives — no heap resources to
        // drop on the destination or to ref-count on the source.
        unsafe { std::ptr::copy_nonoverlapping(src, dst, 1) };
    } else {
        *dst = src.clone();
    }
}

/// Take a register buffer from the pool, or allocate a new one.
/// The returned Vec has exactly `size` elements, all `Value::Nil`.
fn acquire_registers(pool: &mut Vec<Box<[Value]>>, size: usize) -> Box<[Value]> {
    // Best-fit: find the pooled box whose length is >= size with
    // the smallest excess, avoiding reallocation.
    let mut best_idx = None;
    let mut best_excess = usize::MAX;
    for (i, v) in pool.iter().enumerate() {
        let len = v.len();
        if len >= size && len - size < best_excess {
            best_excess = len - size;
            best_idx = Some(i);
            if best_excess == 0 {
                break;
            }
        }
    }
    if let Some(idx) = best_idx {
        let mut regs = pool.swap_remove(idx);
        // Zero out all slots for the new frame.
        for slot in regs.iter_mut() {
            *slot = Value::Nil;
        }
        regs
    } else {
        vec![Value::Nil; size].into_boxed_slice()
    }
}

const REGISTER_POOL_CAP: usize = 8;

/// Return a register buffer to the pool for reuse.
fn recycle_registers(pool: &mut Vec<Box<[Value]>>, regs: Box<[Value]>) {
    if regs.is_empty() {
        return;
    }
    if pool.len() < REGISTER_POOL_CAP {
        pool.push(regs);
    }
}

/// Build a `LuaFrame` by cloning arguments directly from a register slice.
///
/// The first `param_count` args are cloned into registers; any extras become
/// `varargs` (only when `proto.signature.variadic` is true).  This avoids
/// allocating an intermediate `Vec<Value>` for the arguments.
fn make_lua_frame_from_slice(
    pool: &mut Vec<Box<[Value]>>,
    proto: Arc<Proto>,
    upvalues: Vec<UpvalueCell>,
    arg_slice: &[Value],
) -> LuaFrame {
    let param_count = proto.signature.params.len();
    let varargs = if proto.signature.variadic && arg_slice.len() > param_count {
        arg_slice[param_count..].to_vec()
    } else {
        vec![]
    };
    // Pre-size to accommodate vararg expansion so that
    // `Vararg { nresults: -1 }` doesn't need to reallocate.
    let vararg_headroom = if proto.signature.variadic {
        varargs.len()
    } else {
        0
    };
    let stack_size = (proto.max_stack_size as usize).max(param_count).max(1) + vararg_headroom;
    let mut regs = acquire_registers(pool, stack_size);
    let copy_count = arg_slice.len().min(param_count).min(stack_size);
    regs[..copy_count].clone_from_slice(&arg_slice[..copy_count]);
    let reg_count = copy_count;
    LuaFrame {
        proto,
        pc: 0,
        registers: regs,
        reg_count,
        upvalues,
        open_upvalues: vec![],
        call_site: None,
        return_dst: 0,
        pending_nresults: -1,
        varargs,
        coerce_result_to_bool: false,
        last_call_is_method: false,
        last_call_dot_colon: None,
        last_call_receiver_offset: None,
        last_call_callee_sig: None,
    }
}

/// Build a `LuaFrame` from an owned `ValueVec` of arguments.
///
/// The first `param_count` args are moved into registers; any extras become
/// `varargs` (only when `proto.signature.variadic` is true).
fn make_lua_frame(
    pool: &mut Vec<Box<[Value]>>,
    proto: Arc<Proto>,
    upvalues: Vec<UpvalueCell>,
    args: ValueVec,
) -> LuaFrame {
    let param_count = proto.signature.params.len();
    let varargs = if proto.signature.variadic && args.len() > param_count {
        args[param_count..].to_vec()
    } else {
        vec![]
    };
    // Pre-size to accommodate vararg expansion so that
    // `Vararg { nresults: -1 }` doesn't need to reallocate.
    let vararg_headroom = if proto.signature.variadic {
        varargs.len()
    } else {
        0
    };
    let stack_size = (proto.max_stack_size as usize).max(param_count).max(1) + vararg_headroom;
    let mut regs = acquire_registers(pool, stack_size);
    let mut reg_count = 0;
    for (i, a) in args.into_iter().take(param_count).enumerate() {
        regs[i] = a;
        reg_count = i + 1;
    }
    LuaFrame {
        proto,
        pc: 0,
        registers: regs,
        reg_count,
        upvalues,
        open_upvalues: vec![],
        call_site: None,
        return_dst: 0,
        pending_nresults: -1,
        varargs,
        coerce_result_to_bool: false,
        last_call_is_method: false,
        last_call_dot_colon: None,
        last_call_receiver_offset: None,
        last_call_callee_sig: None,
    }
}

/// Read the current `_ENV` value from `frame`'s upvalue at the slot
/// declared by `proto.env_upvalue_idx`.  Returns `Value::Nil` if the
/// proto has no env upvalue (no `GetGlobal`/`SetGlobal` opcodes will
/// be executed in that case so the result is never consulted).
#[inline]
fn frame_env_value(frame: &LuaFrame) -> Value {
    let Some(idx) = frame.proto.env_upvalue_idx else {
        return Value::Nil;
    };
    let Some(cell) = frame.upvalues.get(idx as usize) else {
        return Value::Nil;
    };
    // SAFETY: An upvalue cell is either `Closed` (owns the value) or
    // `Open` (points at a register in an enclosing frame).
    // `frame.upvalues` is the closure's own upvalue list, inherited
    // at frame setup, and remains valid for the lifetime of this
    // frame.  Any `Open` cells point into ancestor frames that are
    // still on the call stack (open upvalues are closed when their
    // owning frame returns), so the pointer is live.
    unsafe { cell.read() }
}

/// Build the error returned when `GetGlobal`/`SetGlobal` is reached
/// but the closure's `_ENV` upvalue holds a non-table value (typically
/// because user code wrote `_ENV = nil` or a non-table).  Mirrors
/// Lua's *"attempt to index a nil value (upvalue '_ENV')"*.
fn env_not_table_error(env_val: &Value, key: &Value) -> VmError {
    VmError::IndexNonTable {
        type_name: env_val.type_name(),
        name: Some(crate::error::VarName::upvalue("_ENV")),
        key: displayable_key(key),
    }
}

/// Ensure `frame.upvalues` has the `_ENV` cell at the slot the proto
/// declares.
///
/// Top-level chunk closures are typically constructed via
/// `Function::lua(proto, vec![])` (tests, examples, the CLI) without
/// an env upvalue.  The proto's upvalue desc list still names `_ENV`
/// at the synthetic root slot, so we synthesize a closed cell holding
/// `GlobalEnv._G` to satisfy that contract.  This keeps the top-level
/// `Function::lua` API ergonomic for embedders.
fn ensure_env_upvalue(frame: &mut LuaFrame, global: &GlobalEnv) {
    let Some(idx) = frame.proto.env_upvalue_idx else {
        return;
    };
    let idx = idx as usize;
    if frame.upvalues.get(idx).is_none() {
        while frame.upvalues.len() < idx {
            frame.upvalues.push(std::sync::Arc::new(
                crate::upvalue::UpvalueInner::new_closed(Value::Nil),
            ));
        }
        let env_cell = std::sync::Arc::new(crate::upvalue::UpvalueInner::new_closed(Value::Table(
            global.0.env.clone(),
        )));
        frame.upvalues.push(env_cell);
    }
}

/// Format a `Value` as a short key string for error messages.
/// Returns `None` for compound types (tables, functions, userdata)
/// whose `Display` output is unstable or unhelpful.
fn displayable_key(v: &Value) -> Option<String> {
    match v {
        Value::Table(_) | Value::Function(_) | Value::Userdata(_) => None,
        Value::String(s) if s.len() > 64 => None,
        other => Some(other.to_string()),
    }
}

fn apply_offset(pc: &mut usize, offset: i32) {
    *pc = (*pc as i64 + offset as i64) as usize;
}

fn compare_lt(a: &Value, b: &Value) -> Result<bool, VmError> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok(x < y),
        (Value::Float(x), Value::Float(y)) => Ok(x < y),
        (Value::Integer(x), Value::Float(y)) => Ok((*x as f64) < *y),
        (Value::Float(x), Value::Integer(y)) => Ok(*x < (*y as f64)),
        (Value::String(x), Value::String(y)) => Ok(x < y),
        _ => Err(VmError::InvalidComparison {
            lhs: a.type_name(),
            lhs_name: None,
            rhs: b.type_name(),
            rhs_name: None,
        }),
    }
}

fn compare_le(a: &Value, b: &Value) -> Result<bool, VmError> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok(x <= y),
        (Value::Float(x), Value::Float(y)) => Ok(x <= y),
        (Value::Integer(x), Value::Float(y)) => Ok((*x as f64) <= *y),
        (Value::Float(x), Value::Integer(y)) => Ok(*x <= (*y as f64)),
        (Value::String(x), Value::String(y)) => Ok(x <= y),
        _ => Err(VmError::InvalidComparison {
            lhs: a.type_name(),
            lhs_name: None,
            rhs: b.type_name(),
            rhs_name: None,
        }),
    }
}

/// Returns `true` if the loop should be skipped (counter already past limit).
fn for_prep(frame: &mut LuaFrame, counter: u8, limit: u8, step: u8) -> Result<bool, VmError> {
    let c = frame.get(counter);
    let l = frame.get(limit);
    let s = frame.get(step);

    if let (Value::Integer(ci), Value::Integer(li), Value::Integer(si)) = (&c, &l, &s) {
        if *si == 0 {
            return Err(VmError::ArithmeticOnNonNumber {
                type_name: "zero step in numeric for",
                name: None,
            });
        }
        return Ok(if *si > 0 { ci > li } else { ci < li });
    }

    let (cf, lf, sf) = match (c.to_float(), l.to_float(), s.to_float()) {
        (Some(c), Some(l), Some(s)) => (c, l, s),
        _ => {
            return Err(VmError::ArithmeticOnNonNumber {
                type_name: "non-numeric for loop bound",
                name: None,
            });
        }
    };
    if sf == 0.0 {
        return Err(VmError::ArithmeticOnNonNumber {
            type_name: "zero step in numeric for",
            name: None,
        });
    }
    frame.set(counter, Value::Float(cf));
    frame.set(limit, Value::Float(lf));
    frame.set(step, Value::Float(sf));
    Ok(if sf > 0.0 { cf > lf } else { cf < lf })
}

/// Returns `true` if the loop should continue (counter still in range).
fn for_step(frame: &mut LuaFrame, counter: u8, limit: u8, step: u8) -> Result<bool, VmError> {
    let regs = &mut frame.registers;
    let ci = counter as usize;
    let li = limit as usize;
    let si = step as usize;
    if ci < regs.len() && li < regs.len() && si < regs.len() {
        if let (Value::Integer(cv), Value::Integer(lv), Value::Integer(sv)) =
            (&regs[ci], &regs[li], &regs[si])
        {
            let next = cv.wrapping_add(*sv);
            let cont = if *sv > 0 { next <= *lv } else { next >= *lv };
            write_reg(&mut regs[ci], Value::Integer(next));
            return Ok(cont);
        }
        if let (Some(cf), Some(lf), Some(sf)) = (
            regs[ci].to_float(),
            regs[li].to_float(),
            regs[si].to_float(),
        ) {
            let next = cf + sf;
            let cont = if sf > 0.0 { next <= lf } else { next >= lf };
            write_reg(&mut regs[ci], Value::Float(next));
            return Ok(cont);
        }
    }
    match (frame.get(counter), frame.get(limit), frame.get(step)) {
        (Value::Integer(ci), Value::Integer(li), Value::Integer(si)) => {
            let next = ci.wrapping_add(si);
            frame.set(counter, Value::Integer(next));
            Ok(if si > 0 { next <= li } else { next >= li })
        }
        (c, l, s) => {
            let cf = c.to_float().expect("float counter");
            let lf = l.to_float().expect("float limit");
            let sf = s.to_float().expect("float step");
            let next = cf + sf;
            frame.set(counter, Value::Float(next));
            Ok(if sf > 0.0 { next <= lf } else { next >= lf })
        }
    }
}

/// Validate `args` against the runtime-typed parameters declared in `sig`.
/// Parameters with no `runtime_type` annotation are unconstrained and skipped.
/// A signature with no annotated parameters passes without any checks.
fn validate_args(sig: &FunctionSignature, args: &[Value]) -> Result<(), VmError> {
    if !sig.has_runtime_types {
        return Ok(());
    }
    let offset = sig.arg_offset;
    for (i, param) in sig.params.iter().enumerate() {
        let idx = offset + i;
        if idx >= args.len() {
            if param.runtime_type.is_some() {
                return Err(VmError::BadArgument {
                    position: i + 1,
                    function: String::from_utf8_lossy(&sig.name).into_owned(),
                    expected: "value".to_owned(),
                    got: "no value".to_owned(),
                });
            }
        } else if let Some(rt) = &param.runtime_type {
            let v = &args[idx];
            if !value_matches_type(v, rt) {
                return Err(VmError::BadArgument {
                    position: i + 1,
                    function: String::from_utf8_lossy(&sig.name).into_owned(),
                    expected: rt.type_name().to_owned(),
                    got: v.type_name().to_owned(),
                });
            }
        }
    }
    Ok(())
}

pub fn value_matches_type(v: &Value, rt: &ValueType) -> bool {
    match rt {
        ValueType::Any => true,
        ValueType::Nil => matches!(v, Value::Nil),
        ValueType::Boolean => matches!(v, Value::Boolean(_)),
        ValueType::Integer => matches!(v, Value::Integer(_)),
        ValueType::Float => matches!(v, Value::Float(_)),
        ValueType::Number => matches!(v, Value::Integer(_) | Value::Float(_)),
        ValueType::String => matches!(v, Value::String(_)),
        ValueType::Table => matches!(v, Value::Table(_)),
        ValueType::Function => matches!(v, Value::Function(_)),
        ValueType::Userdata => matches!(v, Value::Userdata(_)),
        ValueType::UserdataOf(name) => {
            if let Value::Userdata(u) = v {
                u.type_name() == *name
            } else {
                false
            }
        }
    }
}

impl ValueType {
    pub fn type_name(&self) -> &'static str {
        match self {
            ValueType::Any => "any",
            ValueType::Nil => "nil",
            ValueType::Boolean => "boolean",
            ValueType::Integer => "integer",
            ValueType::Float => "float",
            ValueType::Number => "number",
            ValueType::String => "string",
            ValueType::Table => "table",
            ValueType::Function => "function",
            ValueType::Userdata => "userdata",
            ValueType::UserdataOf(_) => "userdata",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_box(size: usize) -> Box<[Value]> {
        vec![Value::Nil; size].into_boxed_slice()
    }

    #[test]
    fn acquire_from_empty_pool() {
        let mut pool: Vec<Box<[Value]>> = Vec::new();
        let regs = acquire_registers(&mut pool, 5);
        k9::assert_equal!(regs.len(), 5);
        assert!(regs.iter().all(|v| matches!(v, Value::Nil)));
    }

    #[test]
    fn recycle_and_reuse() {
        let mut pool: Vec<Box<[Value]>> = Vec::new();
        let mut regs = make_box(10);
        regs[0] = Value::Integer(42);
        recycle_registers(&mut pool, regs);
        k9::assert_equal!(pool.len(), 1);

        // Acquire should reuse the recycled box (len 10 >= 5).
        let regs = acquire_registers(&mut pool, 5);
        k9::assert_equal!(regs.len(), 10);
        assert!(regs.iter().all(|v| matches!(v, Value::Nil)));
        k9::assert_equal!(pool.len(), 0);
    }

    #[test]
    fn best_fit_selection() {
        let mut pool: Vec<Box<[Value]>> = Vec::new();

        recycle_registers(&mut pool, make_box(4));
        recycle_registers(&mut pool, make_box(20));
        recycle_registers(&mut pool, make_box(8));

        k9::assert_equal!(pool.len(), 3);

        // Request size 6: should pick the len-8 box (best fit).
        let regs = acquire_registers(&mut pool, 6);
        k9::assert_equal!(regs.len(), 8);
        k9::assert_equal!(pool.len(), 2);
    }

    #[test]
    fn best_fit_skips_too_small() {
        let mut pool: Vec<Box<[Value]>> = Vec::new();

        recycle_registers(&mut pool, make_box(3));

        // Request size 5: len-3 is too small, should allocate new.
        let regs = acquire_registers(&mut pool, 5);
        k9::assert_equal!(regs.len(), 5);
        // The small box should still be in the pool.
        k9::assert_equal!(pool.len(), 1);
    }

    #[test]
    fn pool_cap_enforced() {
        let mut pool: Vec<Box<[Value]>> = Vec::new();

        for _ in 0..REGISTER_POOL_CAP + 5 {
            recycle_registers(&mut pool, make_box(4));
        }

        k9::assert_equal!(pool.len(), REGISTER_POOL_CAP);
    }
}
