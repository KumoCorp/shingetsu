use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::future::BoxFuture;

use crate::bytecode::{self, OpCode};
use crate::call_context::{CallContext, StackFrame};
use crate::error::{RuntimeError, VmError};
use crate::function::{Function, FunctionState, UpvalueCell};
use crate::global_env::GlobalEnv;
use crate::proto::{Proto, SourceLocation};
use crate::table::Table;
use crate::types::{FunctionSignature, LocalAttr, ValueType};
use crate::userdata::Userdata;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Call frames
// ---------------------------------------------------------------------------

pub struct LuaFrame {
    pub proto: Arc<Proto>,
    pub pc: usize,
    pub registers: Vec<Value>,
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
    /// Per-closure `_ENV` override set by `load(chunk, name, mode, env)`.
    /// When present, `GetGlobal`/`SetGlobal` use this table instead of
    /// the shared `GlobalEnv.env`.
    pub env_override: Option<crate::table::Table>,
}

impl LuaFrame {
    /// Read a register, routing through its open upvalue cell when present.
    #[inline]
    pub fn get(&self, slot: u8) -> Value {
        for (s, cell) in &self.open_upvalues {
            if *s == slot {
                return cell.read().clone();
            }
        }
        self.registers
            .get(slot as usize)
            .cloned()
            .unwrap_or(Value::Nil)
    }

    /// Borrow a register value without cloning.  Falls back to cloning
    /// when the slot is captured as an open upvalue.
    #[inline]
    pub fn get_ref(&self, slot: u8) -> std::borrow::Cow<'_, Value> {
        for (s, cell) in &self.open_upvalues {
            if *s == slot {
                return std::borrow::Cow::Owned(cell.read().clone());
            }
        }
        match self.registers.get(slot as usize) {
            Some(v) => std::borrow::Cow::Borrowed(v),
            None => std::borrow::Cow::Owned(Value::Nil),
        }
    }

    /// Write a register, keeping the open upvalue cell in sync when present.
    #[inline]
    pub fn set(&mut self, slot: u8, val: Value) {
        for (s, cell) in &self.open_upvalues {
            if *s == slot {
                *cell.write() = val.clone();
                break;
            }
        }
        let i = slot as usize;
        if i >= self.registers.len() {
            self.registers.resize(i + 1, Value::Nil);
        }
        self.registers[i] = val;
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
    Done(Vec<Value>),
    Yield(BoxFuture<'static, Result<Vec<Value>, VmError>>),
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
}

struct TaskInner {
    global: GlobalEnv,
    frames: Vec<CallFrame>,
    pending: Option<BoxFuture<'static, Result<Vec<Value>, VmError>>>,
    pending_kind: PendingKind,
    /// nresults expected by the frame that launched the currently-pending
    /// native call (unused for CloseVar/UnwindClose).
    pending_nresults: i32,
    /// Return-register slot in the Lua caller frame for the current pending
    /// native call (unused for CloseVar/UnwindClose).
    pending_dst: usize,
    /// Call stack frames inherited from the task that spawned this one via
    /// `CallContext::call_function`.  Empty for top-level tasks.  Prepended
    /// to this task's own Lua frames when building a `CallContext`.
    parent_stack: Arc<Vec<StackFrame>>,
    /// The error being propagated during error-path `<close>` unwinding.
    /// `None` means normal (non-unwind) execution.
    unwind_error: Option<RuntimeError>,
    /// Queue of `<close>` values still to be dispatched during unwinding.
    /// Values are popped from the end (LIFO), so they are pushed in
    /// outermost-first / earliest-declared-first order.
    unwind_close_vals: Vec<Value>,
    /// Free-list of register `Vec<Value>` buffers for reuse across Lua
    /// calls, avoiding repeated malloc/free.
    register_pool: Vec<Vec<Value>>,
}

const MAX_STACK_DEPTH: usize = 200;

impl TaskInner {
    /// Build a `CallContext` from the current task state.
    ///
    /// The call stack starts with any frames inherited from the parent task
    /// (`self.parent_stack`), followed by a `StackFrame::Lua` entry for each
    /// live Lua frame in this task.  `native_name` is forwarded into the
    /// returned `CallContext` so the native can insert itself when calling
    /// `call_function`.
    /// Begin error-path unwinding: collect all live `<close>` values from
    /// the current frames, then store the error for the poll loop to handle.
    #[cold]
    fn begin_unwind(&mut self, err: VmError) {
        // Capture call stack, variable context, and source text before clearing frames.
        let call_stack = self.snapshot_call_stack();
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
        // Recycle register buffers before dropping frames.
        for frame in self.frames.drain(..) {
            if let CallFrame::Lua(mut f) = frame {
                recycle_registers(&mut self.register_pool, std::mem::take(&mut f.registers));
            }
        }
        self.unwind_close_vals = vals;
        self.unwind_error = Some(RuntimeError {
            error: err,
            call_stack,
            var_context,
            source_text,
            hints,
        });
    }

    /// Snapshot the current call stack as a `Vec<StackFrame>`.
    #[cold]
    fn snapshot_call_stack(&self) -> Vec<StackFrame> {
        let mut call_stack: Vec<StackFrame> = (*self.parent_stack).clone();
        for cf in &self.frames {
            let f = match cf {
                CallFrame::Lua(f) => f,
                CallFrame::Native(_) => continue,
            };
            let source_location =
                f.pc.checked_sub(1)
                    .and_then(|pc| f.proto.source_locations.get(pc))
                    .and_then(|s| s.clone());
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
                source_location,
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
        CallContext {
            global: self.global.clone(),
            call_stack: Arc::new(self.snapshot_call_stack()),
            native_name,
        }
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
            source_location: caller_source_loc,
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
        let dot_colon_loc = last_call_dot_colon.map(|(offset, len)| {
            let source_name = caller_source_loc
                .as_ref()
                .map_or_else(String::new, |sl| sl.source_name.clone());
            crate::proto::SourceLocation {
                source_name,
                line: 0,
                column: 0,
                byte_offset: offset,
                byte_len: len,
            }
        });

        // Determine if the callee is a method definition.
        // Check both the param name ("self") and arg_offset (used by native
        // userdata methods where the first Lua arg is the implicit self).
        let is_method_def = callee_sig.arg_offset > 0
            || callee_sig
                .params
                .first()
                .and_then(|p| p.name.as_ref())
                .map_or(false, |n| n.as_ref() == b"self");

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
                    .find(|(n, _)| n.as_ref() == b"self")
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
    fn write_return_values(&mut self, values: Vec<Value>, dst: usize, nresults: i32) {
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
        if caller.registers.len() < needed {
            caller.registers.resize(needed, Value::Nil);
        } else if nresults < 0 {
            caller.registers.truncate(needed);
        }
        // Clear padding slots to Nil before writing values: if the callee
        // returned fewer values than requested, slots [dst + values.len() .. dst + n)
        // may still hold stale data from the call setup (e.g. the table and
        // key used to resolve an indexed call like `os.clock()`), and those
        // must be nil per Lua's adjust-to-n semantics.
        let provided = values.len().min(n);
        for i in provided..n {
            caller.set((dst + i) as u8, Value::Nil);
        }
        for (i, v) in values.into_iter().enumerate().take(n) {
            caller.set((dst + i) as u8, v);
        }
    }

    /// Transfer return values directly from the callee's owned register vec
    /// into the caller frame, avoiding an intermediate `Vec<Value>` allocation.
    fn write_return_from_registers(
        &mut self,
        callee_regs: Vec<Value>,
        base: usize,
        nresults: i32,
        dst: usize,
        pending_nresults: i32,
    ) {
        let caller = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return,
        };
        let actual_returned = callee_regs.len().saturating_sub(base);
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
        if caller.registers.len() < needed {
            caller.registers.resize(needed, Value::Nil);
        } else if pending_nresults < 0 {
            caller.registers.truncate(needed);
        }
        let provided = actual_returned.min(n);
        // Move values directly from the callee register vec.
        let mut callee_regs = callee_regs;
        for i in 0..provided {
            let src_idx = base + i;
            let val = std::mem::replace(&mut callee_regs[src_idx], Value::Nil);
            if caller.open_upvalues.is_empty() {
                write_reg(&mut caller.registers[dst + i], val);
            } else {
                caller.set((dst + i) as u8, val);
            }
        }
        // Nil-fill remaining slots.
        for i in provided..n {
            if caller.open_upvalues.is_empty() {
                write_reg(&mut caller.registers[dst + i], Value::Nil);
            } else {
                caller.set((dst + i) as u8, Value::Nil);
            }
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
        match get_arith_metamethod(&l, &r, mm_name.as_bytes()) {
            Some(ArithMetamethod::Function(mm_fn)) => {
                self.dispatch_mm_or_yield(mm_fn, vec![l, r], 1, dst, false)
            }
            Some(ArithMetamethod::Userdata(ud)) => {
                Ok(Some(self.dispatch_ud_mm(ud, mm_name, vec![l, r], dst)?))
            }
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
        match get_arith_metamethod(&v, &v, mm_name.as_bytes()) {
            Some(ArithMetamethod::Function(mm_fn)) => {
                self.dispatch_mm_or_yield(mm_fn, vec![v.clone(), v], 1, dst, false)
            }
            Some(ArithMetamethod::Userdata(ud)) => Ok(Some(self.dispatch_ud_mm(
                ud,
                mm_name,
                vec![v.clone(), v],
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
        match get_arith_metamethod(&l, &r, mm_name.as_bytes()) {
            Some(ArithMetamethod::Function(mm_fn)) => {
                self.dispatch_mm_or_yield(mm_fn, vec![l, r], 1, dst, true)
            }
            Some(ArithMetamethod::Userdata(ud)) => {
                Ok(Some(self.dispatch_ud_mm(ud, mm_name, vec![l, r], dst)?))
            }
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
        args: Vec<Value>,
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
            &self.parent_stack,
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
        args: Vec<Value>,
        dst: usize,
    ) -> Result<Step, VmError> {
        let source_label = format!("=[{}]", ud.type_name());
        let ctx = self.build_call_context(None);
        let fut = Arc::clone(&ud).dispatch(ctx, mm_name, args);
        self.pending_kind = PendingKind::NativeCall;
        self.pending_nresults = 1;
        self.pending_dst = dst;
        if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
            caller.return_dst = dst;
            caller.pending_nresults = 1;
        }
        self.frames.push(CallFrame::Native(NativeFrame {
            signature: Arc::new(FunctionSignature {
                name: crate::byte_string::Bytes::from(mm_name.as_bytes()),
                source: crate::byte_string::Bytes::from(source_label),
                type_params: vec![],
                params: vec![],
                variadic: true,
                arg_offset: 0,
                returns: None,
                lua_returns: None,
                line_defined: 0,
                last_line_defined: 0,
                num_upvalues: 0,
            }),
            call_site: None,
        }));
        Ok(Step::Yield(Box::pin(fut)))
    }

    /// Execute the Call opcode.
    /// Returns `Some(step)` when the caller should yield/return,
    /// `None` when the main loop should continue.
    #[inline(never)]
    fn exec_call(&mut self, word: u32) -> Result<Option<Step>, VmError> {
        let frame_count = self.frames.len();
        let frame = match self.frames.last_mut() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };
        let func = bytecode::get_a(word);
        let is_method_call = bytecode::get_k(word);
        let b = bytecode::get_b(word);
        let c = bytecode::get_c(word);
        let nargs: i32 = if b == 0 { -1 } else { (b - 1) as i32 };
        let nresults: i32 = if c == 0 { -1 } else { (c - 1) as i32 };
        let func_val = frame.get(func);
        let call_site = frame.proto.call_site_info.get(&(frame.pc - 1));
        let dot_colon_span = call_site.map(|info| (info.dot_colon_offset, info.dot_colon_len));
        let receiver_offset = call_site.map(|info| info.receiver_offset);
        let return_dst = func as usize;
        let func_slot = func;
        let arg_start = func as usize + 1;
        let arg_end = if nargs < 0 {
            frame.registers.len()
        } else {
            (arg_start + nargs as usize).min(frame.registers.len())
        };
        // Record call-site hint info and callee signature on the
        // caller frame BEFORE validate_args so it's available if
        // validation fails. Done via `frame` (the current
        // last_mut borrow) to avoid a second borrow.
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

        match func_val {
            Value::Function(f) => match f.state() {
                FunctionState::Lua(lf) => {
                    if frame_count >= MAX_STACK_DEPTH {
                        return Err(VmError::StackOverflow);
                    }
                    let arg_slice = &frame.registers[arg_start..arg_end];
                    validate_args(&lf.proto.signature, arg_slice)?;
                    let mut pool = std::mem::take(&mut self.register_pool);
                    let mut new_frame = make_lua_frame_from_slice(
                        &mut pool,
                        lf.proto.clone(),
                        lf.upvalues.clone(),
                        arg_slice,
                    );
                    self.register_pool = pool;
                    new_frame.env_override = lf.env_override.clone();
                    frame.return_dst = return_dst;
                    frame.pending_nresults = nresults;
                    self.frames.push(CallFrame::Lua(new_frame));
                }
                FunctionState::Native(nf) => {
                    let arg_slice = &frame.registers[arg_start..arg_end];
                    validate_args(&nf.signature, arg_slice)?;
                    match &nf.call {
                        crate::function::NativeCall::SyncPlain(call) => {
                            let call = Arc::clone(call);
                            let frame = match self.frames.last_mut() {
                                Some(CallFrame::Lua(f)) => f,
                                _ => return Ok(None),
                            };
                            let arg_slice = &frame.registers[arg_start..arg_end];
                            let results = call(arg_slice)?;
                            self.write_return_values(results, return_dst, nresults);
                        }
                        crate::function::NativeCall::SyncWithCtx(call) => {
                            let call = Arc::clone(call);
                            let native_name = nf.signature.name.clone();
                            let ctx = self.build_call_context(Some(native_name));
                            let frame = match self.frames.last_mut() {
                                Some(CallFrame::Lua(f)) => f,
                                _ => return Ok(None),
                            };
                            let arg_slice = &frame.registers[arg_start..arg_end];
                            let results = call(ctx, arg_slice)?;
                            self.write_return_values(results, return_dst, nresults);
                        }
                        crate::function::NativeCall::Async(call) => {
                            let args: Vec<Value> = arg_slice.to_vec();
                            frame.return_dst = return_dst;
                            frame.pending_nresults = nresults;
                            let ctx = self.build_call_context(Some(nf.signature.name.clone()));
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
                    }
                }
            },
            Value::Table(tab) => {
                let args: Vec<Value> = frame.registers[arg_start..arg_end].to_vec();
                match tab.get_metamethod("__call") {
                    Some(Value::Function(mm_fn)) => {
                        let mut mm_args = vec![Value::Table(tab)];
                        mm_args.extend(args);
                        if let Some(step) =
                            self.dispatch_mm_or_yield(mm_fn, mm_args, nresults, return_dst, false)?
                        {
                            return Ok(Some(step));
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
        Ok(None)
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
        let args = vec![frame.get(base + 1), frame.get(base + 2)];
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
                    let mut new_frame = make_lua_frame(
                        &mut self.register_pool,
                        lf.proto.clone(),
                        lf.upvalues.clone(),
                        args,
                    );
                    new_frame.env_override = lf.env_override.clone();
                    self.frames.push(CallFrame::Lua(new_frame));
                }
                FunctionState::Native(nf) => {
                    validate_args(&nf.signature, &args)?;
                    match &nf.call {
                        crate::function::NativeCall::SyncPlain(call) => {
                            let results = call(&args)?;
                            self.write_return_values(results, return_dst, nresults);
                        }
                        crate::function::NativeCall::SyncWithCtx(call) => {
                            let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                            let results = call(ctx, &args)?;
                            self.write_return_values(results, return_dst, nresults);
                        }
                        crate::function::NativeCall::Async(call) => {
                            if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                caller.return_dst = return_dst;
                                caller.pending_nresults = nresults;
                            }
                            let ctx = self.build_call_context(Some(nf.signature.name.clone()));
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
                    }
                }
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
                let v = tab
                    .raw_get(&k)
                    .map_err(|e| e.with_table_name(frame.register_name(table)))?;
                if !v.is_nil() {
                    frame.set(dst, v);
                } else {
                    // Follow table-only __index chain first.
                    // If the chain ends at a function, fall through
                    // to function dispatch below.
                    let mm = tab.get_metamethod("__index");
                    match mm {
                        None => {
                            frame.set(dst, Value::Nil);
                        }
                        Some(Value::Table(idx_tab)) => match index_table_chain(idx_tab, &k)? {
                            IndexChainResult::Value(v) => {
                                frame.set(dst, v);
                            }
                            IndexChainResult::Function(mm_fn, owner) => {
                                let mm_args = vec![Value::Table(owner), k];
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
                            let mm_args = vec![Value::Table(tab), k];
                            let d = dst as usize;
                            if let Some(step) =
                                self.dispatch_mm_or_yield(mm_fn, mm_args, 1, d, false)?
                            {
                                return Ok(Some(step));
                            }
                        }
                        Some(_) => {
                            // __index is neither table nor function.
                            frame.set(dst, Value::Nil);
                        }
                    }
                }
            }
            Value::Userdata(ud) => {
                // Dispatch __index on userdata.
                let args = vec![Value::Userdata(Arc::clone(&ud)), k];
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
                                let mm_args = vec![Value::Table(owner), k];
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
                            let mm_args = vec![t, k];
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
                // __newindex is only triggered when the key is absent.
                let existing = tab
                    .raw_get(&k)
                    .map_err(|e| e.with_table_name(frame.register_name(table_slot)))?;
                if !existing.is_nil() {
                    // Key already exists — raw write, no metamethod.
                    tab.raw_set(k, v)
                        .map_err(|e| e.with_table_name(frame.register_name(table_slot)))?;
                } else {
                    let mm = tab.get_metamethod("__newindex");
                    match mm {
                        None => {
                            tab.raw_set(k, v)
                                .map_err(|e| e.with_table_name(frame.register_name(table_slot)))?;
                        }
                        Some(Value::Table(dst_tab)) => match newindex_table_chain(dst_tab, &k)? {
                            NewindexChainResult::Table(target) => {
                                target.raw_set(k, v).map_err(|e| {
                                    e.with_table_name(frame.register_name(table_slot))
                                })?;
                            }
                            NewindexChainResult::Function(mm_fn, owner) => {
                                let mm_args = vec![Value::Table(owner), k, v];
                                if let Some(step) =
                                    self.dispatch_mm_or_yield(mm_fn, mm_args, 0, 0, false)?
                                {
                                    return Ok(Some(step));
                                }
                            }
                        },
                        Some(Value::Function(mm_fn)) => {
                            let mm_args = vec![Value::Table(tab), k, v];
                            // __newindex result is discarded (0 results).
                            if let Some(step) =
                                self.dispatch_mm_or_yield(mm_fn, mm_args, 0, 0, false)?
                            {
                                return Ok(Some(step));
                            }
                        }
                        Some(_) => {
                            // Unknown __newindex type: raw write.
                            tab.raw_set(k, v)
                                .map_err(|e| e.with_table_name(frame.register_name(table_slot)))?;
                        }
                    }
                }
            }
            Value::Userdata(ud) => {
                // Dispatch __newindex on userdata.
                let args = vec![Value::Userdata(Arc::clone(&ud)), k, v];
                return Ok(Some(self.dispatch_ud_mm(ud, "__newindex", args, 0)?));
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
            match get_arith_metamethod(&lhs, &rhs, b"__concat") {
                Some(ArithMetamethod::Function(mm_fn)) => {
                    let d = dst as usize;
                    if let Some(step) =
                        self.dispatch_mm_or_yield(mm_fn, vec![lhs, rhs], 1, d, false)?
                    {
                        return Ok(Some(step));
                    }
                }
                Some(ArithMetamethod::Userdata(ud)) => {
                    let d = dst as usize;
                    return Ok(Some(self.dispatch_ud_mm(
                        ud,
                        "__concat",
                        vec![lhs, rhs],
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
        if frame.open_upvalues.is_empty() {
            if let (Value::Integer(a), Value::Integer(b)) =
                (&frame.registers[li], &frame.registers[ri])
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
                        if let Some(step) = self
                            .handle_compare_metamethod(ml, mr, mm_name, e, names.0, names.1, di)?
                        {
                            return Ok(Some(step));
                        }
                    }
                }
            }
        } else {
            let l = frame.get(lhs);
            let r = frame.get(rhs);
            if let (Value::Integer(a), Value::Integer(b)) = (&l, &r) {
                let result = if swap {
                    b < a || (mm_name == "__le" && b == a)
                } else {
                    if mm_name == "__le" {
                        a <= b
                    } else {
                        a < b
                    }
                };
                frame.set(dst, Value::Boolean(result));
            } else {
                let (cl, cr) = if swap { (&r, &l) } else { (&l, &r) };
                match compare_fn(cl, cr) {
                    Ok(v) => frame.set(dst, Value::Boolean(v)),
                    Err(e) => {
                        let names = (frame.register_name(lhs), frame.register_name(rhs));
                        let (ml, mr) = if swap { (r, l) } else { (l, r) };
                        if let Some(step) = self
                            .handle_compare_metamethod(ml, mr, mm_name, e, names.0, names.1, di)?
                        {
                            return Ok(Some(step));
                        }
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

        // Pop the callee frame to get ownership of its registers.
        let callee = match self.frames.pop() {
            Some(CallFrame::Lua(f)) => f,
            _ => return Ok(None),
        };

        if self.frames.is_empty() {
            // Top-level return — must build a Vec for the caller.
            let mut callee_regs = callee.registers;
            let results: Vec<Value> = if coerce {
                let truthy = callee_regs
                    .get(base)
                    .map(|v| v.is_truthy())
                    .unwrap_or(false);
                vec![Value::Boolean(truthy)]
            } else if nresults < 0 {
                callee_regs.drain(base..).collect()
            } else {
                callee_regs.drain(base..).take(nresults as usize).collect()
            };
            recycle_registers(&mut self.register_pool, callee_regs);
            return Ok(Some(Step::Done(results)));
        }

        let (return_dst, pending_nresults) = match self.frames.last() {
            Some(CallFrame::Lua(f)) => (f.return_dst, f.pending_nresults),
            _ => (0, -1),
        };

        if coerce {
            let truthy = callee
                .registers
                .get(base)
                .map(|v| v.is_truthy())
                .unwrap_or(false);
            recycle_registers(&mut self.register_pool, callee.registers);
            self.write_return_values(vec![Value::Boolean(truthy)], return_dst, pending_nresults);
        } else {
            self.write_return_from_registers(
                callee.registers,
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
                        // Create a fresh cell from the current register
                        // value.  The register itself stays valid; both
                        // frame.get/set and the inner closure now route
                        // through this shared cell.
                        let val = frame
                            .registers
                            .get(slot as usize)
                            .cloned()
                            .unwrap_or(Value::Nil);
                        let cell = Arc::new(parking_lot::RwLock::new(val));
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
                        .unwrap_or_else(|| Arc::new(parking_lot::RwLock::new(Value::Nil))),
                );
            }
        }
        let func = if let Some(env) = frame.env_override.clone() {
            Function::lua_with_env(child_proto, upvalues, env)
        } else {
            Function::lua(child_proto, upvalues)
        };
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
            frame.registers.len().saturating_sub(src_base as usize)
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
                        if let Some(step) = self.dispatch_mm_or_yield(mm, vec![val], 1, d, false)? {
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
                    let args = vec![Value::Userdata(Arc::clone(ud))];
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
                        let mm_args = vec![v];
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
                let args = vec![v];
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
        let l = frame.get(lhs);
        let r = frame.get(rhs);
        if l == r {
            frame.set(dst, Value::Boolean(true));
        } else {
            let mm = match (&l, &r) {
                (Value::Table(lt), Value::Table(rt)) => lt
                    .get_metamethod("__eq")
                    .or_else(|| rt.get_metamethod("__eq")),
                _ => None,
            };
            match mm {
                Some(Value::Function(mm_fn)) => {
                    let d = dst as usize;
                    if let Some(step) = self.dispatch_mm_or_yield(mm_fn, vec![l, r], 1, d, true)? {
                        return Ok(Some(step));
                    }
                }
                _ => {
                    frame.set(dst, Value::Boolean(false));
                }
            }
        }
        Ok(None)
    }

    fn step(&mut self) -> Result<Step, VmError> {
        // Outer loop: re-entered after frame-changing operations (calls,
        // returns, metamethods).  The inner dispatch loop runs with a
        // cached frame reference, avoiding the `self.frames.last_mut()`
        // lookup on every opcode.
        'outer: loop {
            let frame = match self.frames.last_mut() {
                None => return Ok(Step::Done(vec![])),
                Some(CallFrame::Native(_)) => {
                    // Should not happen: native frames are only present while
                    // pending is Some.
                    self.frames.pop();
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
                        if frame.open_upvalues.is_empty() {
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
                        } else {
                            let l = frame.get($lhs);
                            let r = frame.get($rhs);
                            match (&l, &r) {
                                (Value::Integer($a), Value::Integer($b)) => {
                                    frame.set($dst, Value::Integer($int_expr));
                                }
                                _ => match l.$op(&r) {
                                    Ok(v) => frame.set($dst, v),
                                    Err(e) => {
                                        let name = $err_name;
                                        if let Some(step) =
                                            self.handle_binary_metamethod(l, r, $mm, e, name, di)?
                                        {
                                            return Ok(step);
                                        }
                                        continue 'outer;
                                    }
                                },
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
                        if frame.open_upvalues.is_empty() {
                            let di = dst as usize;
                            let si = src as usize;
                            if di >= frame.registers.len() {
                                frame.registers.resize(di + 1, Value::Nil);
                            }
                            if si < frame.registers.len() {
                                let (left, right) = if di < si {
                                    let (l, r) = frame.registers.split_at_mut(si);
                                    (&mut l[di], &r[0])
                                } else if di > si {
                                    let (l, r) = frame.registers.split_at_mut(di);
                                    (&mut r[0], &l[si])
                                } else {
                                    continue;
                                };
                                copy_reg(left, right);
                            } else {
                                write_reg(&mut frame.registers[di], Value::Nil);
                            }
                        } else {
                            let v = frame.get(src);
                            frame.set(dst, v);
                        }
                    }
                    OpCode::GetGlobal => {
                        let dst = bytecode::get_a(word);
                        let name = bytecode::get_bx(word) as usize;
                        let key = frame.proto.constants[name].clone();
                        let env = frame.env_override.as_ref().unwrap_or(&self.global.0.env);
                        let v = env.raw_get(&key).unwrap_or(Value::Nil);
                        frame.set(dst, v);
                    }
                    OpCode::SetGlobal => {
                        let src = bytecode::get_a(word);
                        let name = bytecode::get_bx(word) as usize;
                        let key = frame.proto.constants[name].clone();
                        let v = frame.get(src);
                        let env = frame.env_override.as_ref().unwrap_or(&self.global.0.env);
                        env.raw_set(key, v).ok();
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
                    OpCode::Shl => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        binary_op_with_metamethod!(
                            dst,
                            lhs,
                            rhs,
                            arith_shl,
                            "__shl",
                            frame.bitwise_error_name(lhs, rhs)
                        );
                    }
                    OpCode::Shr => {
                        let (dst, lhs, rhs) = (
                            bytecode::get_a(word),
                            bytecode::get_b(word),
                            bytecode::get_c(word),
                        );
                        binary_op_with_metamethod!(
                            dst,
                            lhs,
                            rhs,
                            arith_shr,
                            "__shr",
                            frame.bitwise_error_name(lhs, rhs)
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
                        if let Some(step) = self.exec_call(word)? {
                            return Ok(step);
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
                            .map(|cell| cell.read().clone())
                            .unwrap_or(Value::Nil);
                        frame.set(dst, val);
                    }
                    OpCode::SetUpval => {
                        let upval = bytecode::get_a(word);
                        let src = bytecode::get_b(word);
                        let val = frame.get(src);
                        if let Some(cell) = frame.upvalues.get(upval as usize) {
                            *cell.write() = val;
                        }
                    }

                    OpCode::GetTable => {
                        let _ = frame;
                        if let Some(step) = self.exec_get_table(word)? {
                            return Ok(step);
                        }
                        continue 'outer;
                    }
                    OpCode::SetTable => {
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
                        if let Some(fut) =
                            close_future(val, &self.global, self.parent_stack.clone())
                        {
                            self.pending_kind = PendingKind::CloseVar;
                            return Ok(Step::Yield(fut));
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
                            // Expand all varargs and resize the register file so
                            // that `Return { nresults: -1 }` and
                            // `Call { nargs: -1 }` see the right count.
                            let n = varargs.len();
                            let new_len = dst as usize + n;
                            frame.registers.resize(new_len, Value::Nil);
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
        if self.inner.unwind_error.is_none() {
            // Drop any pending async operation (blocking native, in-flight
            // __close, etc.) and collect live <close> locals from all frames.
            self.inner.pending = None;
            self.inner.begin_unwind(VmError::LuaError {
                display: "task cancelled".to_owned(),
                value: Value::Nil,
            });
        }
        // Drive the unwind loop (dispatches __close handlers), then discard
        // the final error — it is either the synthetic cancel error above or
        // the original error that triggered an already-in-progress unwind.
        let _ = self.await;
    }

    /// Create a new top-level task.
    pub fn new(global: GlobalEnv, func: Function, args: Vec<Value>) -> Self {
        Self::new_inner(global, func, args, Arc::new(vec![]))
    }

    /// Create a task that inherits a parent call stack.  Used by
    /// `CallContext::call_function` so that nested native→Lua calls appear
    /// in stack traces with the full outer context prepended.
    pub fn new_with_parent(
        global: GlobalEnv,
        func: Function,
        args: Vec<Value>,
        parent_stack: Arc<Vec<StackFrame>>,
    ) -> Self {
        Self::new_inner(global, func, args, parent_stack)
    }

    fn new_inner(
        global: GlobalEnv,
        func: Function,
        args: Vec<Value>,
        parent_stack: Arc<Vec<StackFrame>>,
    ) -> Self {
        match func.state() {
            FunctionState::Lua(lf) => {
                let validation_err = validate_args(&lf.proto.signature, &args).err();
                let mut pool = Vec::new();
                let mut frame =
                    make_lua_frame(&mut pool, lf.proto.clone(), lf.upvalues.clone(), args);
                frame.env_override = lf.env_override.clone();
                let unwind_error = validation_err.map(|error| RuntimeError {
                    error,
                    call_stack: (*parent_stack).clone(),
                    var_context: None,
                    source_text: lf.proto.source_text.clone(),
                    hints: vec![],
                });
                Task {
                    inner: TaskInner {
                        global,
                        frames: vec![CallFrame::Lua(frame)],
                        pending: None,
                        pending_kind: PendingKind::NativeCall,
                        pending_nresults: -1,
                        pending_dst: 0,
                        parent_stack,
                        unwind_error,
                        unwind_close_vals: Vec::new(),
                        register_pool: pool,
                    },
                }
            }
            FunctionState::Native(nf) => {
                // No Lua frames yet; build a context with the inherited parent
                // stack plus this native's own name.
                let build_ctx = || CallContext {
                    global: global.clone(),
                    call_stack: parent_stack.clone(),
                    native_name: Some(nf.signature.name.clone()),
                };
                let fut: BoxFuture<'static, Result<Vec<Value>, VmError>> = match &nf.call {
                    crate::function::NativeCall::SyncPlain(call) => {
                        let result = call(&args);
                        Box::pin(async move { result })
                    }
                    crate::function::NativeCall::SyncWithCtx(call) => {
                        let result = call(build_ctx(), &args);
                        Box::pin(async move { result })
                    }
                    crate::function::NativeCall::Async(call) => call(build_ctx(), args),
                };
                Task {
                    inner: TaskInner {
                        global,
                        frames: vec![CallFrame::Native(NativeFrame {
                            signature: nf.signature.clone(),
                            call_site: None,
                        })],
                        pending: Some(fut),
                        pending_kind: PendingKind::NativeCall,
                        pending_nresults: -1,
                        pending_dst: 0,
                        parent_stack,
                        unwind_error: None,
                        unwind_close_vals: Vec::new(),
                        register_pool: Vec::new(),
                    },
                }
            }
        }
    }
}

impl std::future::Future for Task {
    type Output = Result<Vec<Value>, RuntimeError>;

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
                                        if self.inner.frames.is_empty() {
                                            return Poll::Ready(Ok(values));
                                        }
                                        let dst = self.inner.pending_dst;
                                        let nresults = self.inner.pending_nresults;
                                        self.inner.write_return_values(values, dst, nresults);
                                    }
                                    PendingKind::CloseVar | PendingKind::UnwindClose => {
                                        // __close results are discarded.
                                    }
                                }
                            }
                            Err(e) => {
                                match self.inner.pending_kind {
                                    PendingKind::NativeCall => {
                                        // A native call failed — start unwinding.
                                        self.inner.frames.pop();
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
                            close_future(val, &self.inner.global, self.inner.parent_stack.clone())
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
    })
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
fn get_arith_metamethod(lhs: &Value, rhs: &Value, event: &[u8]) -> Option<ArithMetamethod> {
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
    })
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
    register_pool: &mut Vec<Vec<Value>>,
    global: &crate::global_env::GlobalEnv,
    parent_stack: &std::sync::Arc<Vec<crate::call_context::StackFrame>>,
    mm_fn: crate::function::Function,
    args: Vec<Value>,
    _pending_nresults: i32,
    _pending_dst: usize,
    coerce_to_bool: bool,
) -> Result<Option<futures::future::BoxFuture<'static, Result<Vec<Value>, VmError>>>, VmError> {
    match mm_fn.state() {
        FunctionState::Lua(lf) => {
            validate_args(&lf.proto.signature, &args)?;
            let mut new_frame =
                make_lua_frame(register_pool, lf.proto.clone(), lf.upvalues.clone(), args);
            new_frame.env_override = lf.env_override.clone();
            new_frame.coerce_result_to_bool = coerce_to_bool;
            frames.push(CallFrame::Lua(new_frame));
            Ok(None)
        }
        FunctionState::Native(nf) => {
            validate_args(&nf.signature, &args)?;
            let build_ctx = || {
                let mut call_stack: Vec<crate::call_context::StackFrame> = (**parent_stack).clone();
                for cf in frames.iter() {
                    if let CallFrame::Lua(f) = cf {
                        let source_location =
                            f.pc.checked_sub(1)
                                .and_then(|pc| f.proto.source_locations.get(pc))
                                .and_then(|s| s.clone());
                        call_stack.push(crate::call_context::StackFrame::Lua {
                            function: f.proto.signature.clone(),
                            source_location,
                            locals: vec![],
                            last_call_is_method: f.last_call_is_method,
                            last_call_dot_colon: f.last_call_dot_colon,
                            last_call_receiver_offset: f.last_call_receiver_offset,
                            last_call_callee_sig: f.last_call_callee_sig.clone(),
                        });
                    }
                }
                CallContext {
                    global: global.clone(),
                    call_stack: std::sync::Arc::new(call_stack),
                    native_name: Some(nf.signature.name.clone()),
                }
            };
            match &nf.call {
                crate::function::NativeCall::SyncPlain(call) => {
                    let mut results = call(&args)?;
                    if coerce_to_bool {
                        let b = results.first().map(|v| v.is_truthy()).unwrap_or(false);
                        results = vec![Value::Boolean(b)];
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
                    let ctx = build_ctx();
                    let mut results = call(ctx, &args)?;
                    if coerce_to_bool {
                        let b = results.first().map(|v| v.is_truthy()).unwrap_or(false);
                        results = vec![Value::Boolean(b)];
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
                crate::function::NativeCall::Async(call) => {
                    let ctx = build_ctx();
                    let raw_fut = call(ctx, args);
                    let fut: futures::future::BoxFuture<'static, Result<Vec<Value>, VmError>> =
                        if coerce_to_bool {
                            Box::pin(async move {
                                let results = raw_fut.await?;
                                let b = results.first().map(|v| v.is_truthy()).unwrap_or(false);
                                Ok(vec![Value::Boolean(b)])
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
    parent_stack: Arc<Vec<StackFrame>>,
) -> Option<BoxFuture<'static, Result<Vec<Value>, VmError>>> {
    match val {
        Value::Userdata(ud) => {
            let ud_arg = ud.clone();
            let ctx = CallContext {
                global: global.clone(),
                call_stack: parent_stack,
                native_name: Some(crate::byte_string::Bytes::from("__close")),
            };
            Some(ud.dispatch(ctx, "__close", vec![Value::Userdata(ud_arg)]))
        }
        Value::Table(ref t) => {
            if let Some(Value::Function(mm)) = t.get_metamethod("__close") {
                // Run the __close metamethod as a nested task so we can
                // handle both Lua and native implementations.
                let task = Task::new_with_parent(global.clone(), mm, vec![val], parent_stack);
                Some(Box::pin(async move {
                    // Ignore result and error — the original error propagates.
                    let _ = task.await;
                    Ok(vec![])
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
fn acquire_registers(pool: &mut Vec<Vec<Value>>, size: usize) -> Vec<Value> {
    if let Some(mut regs) = pool.pop() {
        regs.clear();
        regs.resize(size, Value::Nil);
        regs
    } else {
        vec![Value::Nil; size]
    }
}

/// Return a register buffer to the pool for reuse.
fn recycle_registers(pool: &mut Vec<Vec<Value>>, mut regs: Vec<Value>) {
    regs.clear();
    pool.push(regs);
}

/// Build a `LuaFrame` by cloning arguments directly from a register slice.
///
/// The first `param_count` args are cloned into registers; any extras become
/// `varargs` (only when `proto.signature.variadic` is true).  This avoids
/// allocating an intermediate `Vec<Value>` for the arguments.
fn make_lua_frame_from_slice(
    pool: &mut Vec<Vec<Value>>,
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
    let stack_size = (proto.max_stack_size as usize).max(param_count);
    let mut regs = acquire_registers(pool, stack_size);
    let copy_count = arg_slice.len().min(param_count).min(stack_size);
    regs[..copy_count].clone_from_slice(&arg_slice[..copy_count]);
    LuaFrame {
        proto,
        pc: 0,
        registers: regs,
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
        env_override: None,
    }
}

/// Build a `LuaFrame` from an owned `Vec<Value>` of arguments.
///
/// The first `param_count` args are moved into registers; any extras become
/// `varargs` (only when `proto.signature.variadic` is true).
fn make_lua_frame(
    pool: &mut Vec<Vec<Value>>,
    proto: Arc<Proto>,
    upvalues: Vec<UpvalueCell>,
    args: Vec<Value>,
) -> LuaFrame {
    let param_count = proto.signature.params.len();
    let varargs = if proto.signature.variadic && args.len() > param_count {
        args[param_count..].to_vec()
    } else {
        vec![]
    };
    let stack_size = (proto.max_stack_size as usize).max(param_count);
    let mut regs = acquire_registers(pool, stack_size);
    for (i, a) in args.into_iter().take(param_count).enumerate() {
        regs[i] = a;
    }
    LuaFrame {
        proto,
        pc: 0,
        registers: regs,
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
        env_override: None,
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
