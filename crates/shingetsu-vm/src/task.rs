use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::future::BoxFuture;

use crate::call_context::{CallContext, StackFrame};
use crate::error::VmError;
use crate::function::{Function, FunctionState, UpvalueCell};
use crate::global_env::GlobalEnv;
use crate::ir::Instruction;
use crate::proto::{Proto, SourceLocation};
use crate::table::Table;
use crate::types::{FunctionSignature, LocalAttr, ValueType};
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
        if self.registers.len() <= i {
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
            match self.proto.instructions.get(scan_pc) {
                Some(crate::ir::Instruction::GetGlobal { dst, name }) if *dst == slot => {
                    if let Some(s) = self
                        .proto
                        .constants
                        .get(*name as usize)
                        .and_then(|b| std::str::from_utf8(b).ok())
                    {
                        return Some(crate::error::VarName::global(s));
                    }
                }
                Some(crate::ir::Instruction::Move { dst, src }) if *dst == slot => {
                    // The value was moved from another register; follow the chain.
                    return self.register_name_inner(*src, depth - 1);
                }
                _ => {}
            }
        }
        None
    }

    /// Read a string constant from the proto's constant pool.
    pub fn constant_str(&self, idx: u16) -> Option<&str> {
        self.proto
            .constants
            .get(idx as usize)
            .and_then(|b| std::str::from_utf8(b).ok())
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
    unwind_error: Option<VmError>,
    /// Queue of `<close>` values still to be dispatched during unwinding.
    /// Values are popped from the end (LIFO), so they are pushed in
    /// outermost-first / earliest-declared-first order.
    unwind_close_vals: Vec<Value>,
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
    fn begin_unwind(&mut self, err: VmError) {
        let vals = collect_close_vals(&mut self.frames);
        // Drop frames — we no longer need to execute them.
        self.frames.clear();
        self.unwind_close_vals = vals;
        self.unwind_error = Some(err);
    }

    fn build_call_context(&self, native_name: Option<bytes::Bytes>) -> CallContext {
        // Start with frames inherited from the spawning task.
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
            // Collect live locals (requires debug info in the proto;
            // currently always empty until the compiler emits LocalDesc).
            let locals: Vec<(bytes::Bytes, Value)> = f
                .proto
                .locals
                .iter()
                .filter(|l| l.start_pc <= f.pc && f.pc < l.end_pc)
                .map(|l| (l.name.clone(), f.get(l.slot)))
                .collect();
            call_stack.push(StackFrame::Lua {
                function_name: f.proto.signature.name.clone(),
                source_location,
                locals,
            });
        }
        CallContext {
            global: self.global.clone(),
            call_stack: Arc::new(call_stack),
            native_name,
        }
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

    fn step(&mut self) -> Result<Step, VmError> {
        loop {
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

            if frame.pc >= frame.proto.instructions.len() {
                // Implicit return nil at end of chunk.
                self.frames.pop();
                if self.frames.is_empty() {
                    return Ok(Step::Done(vec![]));
                }
                // Read return coordinates from the new top (caller) frame.
                let (return_dst, pending_nresults) = match self.frames.last() {
                    Some(CallFrame::Lua(f)) => (f.return_dst, f.pending_nresults),
                    _ => (0, -1),
                };
                self.write_return_values(vec![], return_dst, pending_nresults);
                continue;
            }

            let instr = frame.proto.instructions[frame.pc].clone();
            frame.pc += 1;

            match instr {
                Instruction::LoadNil { dst } => {
                    frame.set(dst, Value::Nil);
                }
                Instruction::LoadBool { dst, value } => {
                    frame.set(dst, Value::Boolean(value));
                }
                Instruction::LoadInt { dst, value } => {
                    frame.set(dst, Value::Integer(value));
                }
                Instruction::LoadFloat { dst, value } => {
                    frame.set(dst, Value::Float(value));
                }
                Instruction::LoadK { dst, idx } => {
                    let c = frame.proto.constants[idx as usize].clone();
                    frame.set(dst, Value::String(c));
                }
                Instruction::Move { dst, src } => {
                    let v = frame.get(src);
                    frame.set(dst, v);
                }
                Instruction::GetGlobal { dst, name } => {
                    let key = &frame.proto.constants[name as usize];
                    let v = self
                        .global
                        .0
                        .globals
                        .get(key.as_ref())
                        .map(|r| r.clone())
                        .unwrap_or(Value::Nil);
                    frame.set(dst, v);
                }
                Instruction::SetGlobal { name, src } => {
                    let key = frame.proto.constants[name as usize].clone();
                    let v = frame.get(src);
                    self.global.0.globals.insert(key, v);
                }
                Instruction::Jump { offset } => {
                    apply_offset(&mut frame.pc, offset);
                }
                Instruction::BranchFalse { src, offset } => {
                    if !frame.get(src).is_truthy() {
                        apply_offset(&mut frame.pc, offset);
                    }
                }
                Instruction::BranchTrue { src, offset } => {
                    if frame.get(src).is_truthy() {
                        apply_offset(&mut frame.pc, offset);
                    }
                }

                // Arithmetic
                Instruction::Add { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match l.arith_add(&r) {
                        Ok(v) => frame.set(dst, v),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__add") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => return Err(e.with_name(frame.arith_error_name(lhs, rhs))),
                        },
                    }
                }
                Instruction::Sub { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match l.arith_sub(&r) {
                        Ok(v) => frame.set(dst, v),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__sub") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => return Err(e.with_name(frame.arith_error_name(lhs, rhs))),
                        },
                    }
                }
                Instruction::Mul { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match l.arith_mul(&r) {
                        Ok(v) => frame.set(dst, v),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__mul") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => return Err(e.with_name(frame.arith_error_name(lhs, rhs))),
                        },
                    }
                }
                Instruction::Div { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match l.arith_div(&r) {
                        Ok(v) => frame.set(dst, v),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__div") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => return Err(e.with_name(frame.arith_error_name(lhs, rhs))),
                        },
                    }
                }
                Instruction::IDiv { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match l.arith_idiv(&r) {
                        Ok(v) => frame.set(dst, v),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__idiv") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => return Err(e.with_name(frame.arith_error_name(lhs, rhs))),
                        },
                    }
                }
                Instruction::Mod { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match l.arith_mod(&r) {
                        Ok(v) => frame.set(dst, v),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__mod") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => return Err(e.with_name(frame.arith_error_name(lhs, rhs))),
                        },
                    }
                }
                Instruction::Pow { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match l.arith_pow(&r) {
                        Ok(v) => frame.set(dst, v),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__pow") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => return Err(e.with_name(frame.arith_error_name(lhs, rhs))),
                        },
                    }
                }
                Instruction::Neg { dst, src } => {
                    let v = frame.get(src);
                    match v.arith_neg() {
                        Ok(result) => frame.set(dst, result),
                        Err(e) => match get_arith_metamethod(&v, &v, b"__unm") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![v.clone(), v],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => return Err(e.with_name(frame.register_name(src))),
                        },
                    }
                }
                Instruction::BAnd { dst, lhs, rhs } => {
                    let v = frame
                        .get(lhs)
                        .arith_band(&frame.get(rhs))
                        .map_err(|e| e.with_name(frame.bitwise_error_name(lhs, rhs)))?;
                    frame.set(dst, v);
                }
                Instruction::BOr { dst, lhs, rhs } => {
                    let v = frame
                        .get(lhs)
                        .arith_bor(&frame.get(rhs))
                        .map_err(|e| e.with_name(frame.bitwise_error_name(lhs, rhs)))?;
                    frame.set(dst, v);
                }
                Instruction::BXor { dst, lhs, rhs } => {
                    let v = frame
                        .get(lhs)
                        .arith_bxor(&frame.get(rhs))
                        .map_err(|e| e.with_name(frame.bitwise_error_name(lhs, rhs)))?;
                    frame.set(dst, v);
                }
                Instruction::BNot { dst, src } => {
                    let v = frame
                        .get(src)
                        .arith_bnot()
                        .map_err(|e| e.with_name(frame.register_name(src)))?;
                    frame.set(dst, v);
                }
                Instruction::Shl { dst, lhs, rhs } => {
                    let v = frame
                        .get(lhs)
                        .arith_shl(&frame.get(rhs))
                        .map_err(|e| e.with_name(frame.bitwise_error_name(lhs, rhs)))?;
                    frame.set(dst, v);
                }
                Instruction::Shr { dst, lhs, rhs } => {
                    let v = frame
                        .get(lhs)
                        .arith_shr(&frame.get(rhs))
                        .map_err(|e| e.with_name(frame.bitwise_error_name(lhs, rhs)))?;
                    frame.set(dst, v);
                }
                Instruction::Not { dst, src } => {
                    let v = !frame.get(src).is_truthy();
                    frame.set(dst, Value::Boolean(v));
                }

                // Comparison
                Instruction::Eq { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    // Same-reference tables (and all equal primitives) skip __eq.
                    if l == r {
                        frame.set(dst, Value::Boolean(true));
                    } else {
                        // __eq is only checked when both values are tables.
                        let mm = match (&l, &r) {
                            (Value::Table(lt), Value::Table(rt)) => lt
                                .get_metamethod("__eq")
                                .or_else(|| rt.get_metamethod("__eq")),
                            _ => None,
                        };
                        match mm {
                            Some(Value::Function(mm_fn)) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    true,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            _ => {
                                frame.set(dst, Value::Boolean(false));
                            }
                        }
                    }
                }
                Instruction::Ne { dst, lhs, rhs } => {
                    // ~= is always not (==), including metamethods.  The
                    // compiler emits Eq+Not for `~=`, so this fast-path only
                    // runs when the instruction appears in hand-crafted bytecode.
                    let v = frame.get(lhs) != frame.get(rhs);
                    frame.set(dst, Value::Boolean(v));
                }
                Instruction::Lt { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match compare_lt(&l, &r) {
                        Ok(v) => frame.set(dst, Value::Boolean(v)),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__lt") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    true,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => {
                                return Err(e.with_comparison_names(
                                    frame.register_name(lhs),
                                    frame.register_name(rhs),
                                ))
                            }
                        },
                    }
                }
                Instruction::Le { dst, lhs, rhs } => {
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match compare_le(&l, &r) {
                        Ok(v) => frame.set(dst, Value::Boolean(v)),
                        Err(e) => match get_arith_metamethod(&l, &r, b"__le") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![l, r],
                                    1,
                                    d,
                                    true,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => {
                                return Err(e.with_comparison_names(
                                    frame.register_name(lhs),
                                    frame.register_name(rhs),
                                ))
                            }
                        },
                    }
                }
                Instruction::Gt { dst, lhs, rhs } => {
                    // a > b  ↔  b < a  ↔  __lt(b, a)
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match compare_lt(&r, &l) {
                        Ok(v) => frame.set(dst, Value::Boolean(v)),
                        Err(e) => match get_arith_metamethod(&r, &l, b"__lt") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![r, l],
                                    1,
                                    d,
                                    true,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => {
                                return Err(e.with_comparison_names(
                                    frame.register_name(lhs),
                                    frame.register_name(rhs),
                                ))
                            }
                        },
                    }
                }
                Instruction::Ge { dst, lhs, rhs } => {
                    // a >= b  ↔  b <= a  ↔  __le(b, a)
                    let l = frame.get(lhs);
                    let r = frame.get(rhs);
                    match compare_le(&r, &l) {
                        Ok(v) => frame.set(dst, Value::Boolean(v)),
                        Err(e) => match get_arith_metamethod(&r, &l, b"__le") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![r, l],
                                    1,
                                    d,
                                    true,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
                            }
                            None => {
                                return Err(e.with_comparison_names(
                                    frame.register_name(lhs),
                                    frame.register_name(rhs),
                                ))
                            }
                        },
                    }
                }

                // Numeric for
                Instruction::ForPrep {
                    counter,
                    limit,
                    step,
                    exit_offset,
                } => {
                    if for_prep(frame, counter, limit, step)? {
                        apply_offset(&mut frame.pc, exit_offset);
                    }
                }
                Instruction::ForStep {
                    counter,
                    limit,
                    step,
                    body_offset,
                } => {
                    if for_step(frame, counter, limit, step)? {
                        apply_offset(&mut frame.pc, body_offset);
                    }
                }

                // Generic for
                Instruction::GenericForCall {
                    iter,
                    state,
                    control,
                    vars,
                    nresults,
                } => {
                    let func_val = frame.get(iter);
                    let args = vec![frame.get(state), frame.get(control)];
                    let return_dst = vars as usize;
                    let nresults_i32 = nresults as i32;

                    match func_val {
                        Value::Function(f) => match f.state() {
                            FunctionState::Lua(lf) => {
                                if self.frames.len() >= MAX_STACK_DEPTH {
                                    return Err(VmError::StackOverflow);
                                }
                                validate_args(&lf.proto.signature, &args)?;
                                if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                    caller.return_dst = return_dst;
                                    caller.pending_nresults = nresults_i32;
                                }
                                let new_frame =
                                    make_lua_frame(lf.proto.clone(), lf.upvalues.clone(), args);
                                self.frames.push(CallFrame::Lua(new_frame));
                            }
                            FunctionState::Native(nf) => {
                                validate_args(&nf.signature, &args)?;
                                if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                    caller.return_dst = return_dst;
                                    caller.pending_nresults = nresults_i32;
                                }
                                let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                                let fut = (nf.call)(ctx, args);
                                self.pending_kind = PendingKind::NativeCall;
                                self.pending_nresults = nresults_i32;
                                self.pending_dst = return_dst;
                                self.frames.push(CallFrame::Native(NativeFrame {
                                    signature: nf.signature.clone(),
                                    call_site: None,
                                }));
                                return Ok(Step::Yield(fut));
                            }
                        },
                        other => {
                            return Err(VmError::CallNonFunction {
                                type_name: other.type_name(),
                                name: frame.register_name(iter),
                            });
                        }
                    }
                }
                Instruction::GenericForCheck {
                    control,
                    vars,
                    exit_offset,
                } => {
                    let first_var = frame.get(vars);
                    if first_var.is_nil() {
                        apply_offset(&mut frame.pc, exit_offset);
                    } else {
                        frame.set(control, first_var);
                    }
                }

                // Function call
                Instruction::Call {
                    func,
                    nargs,
                    nresults,
                } => {
                    let func_val = frame.get(func);
                    // nargs = -1 means "take everything above `func` on the
                    // register stack" (after a Vararg or multi-return expansion).
                    let actual_nargs: usize = if nargs < 0 {
                        let top = frame.registers.len();
                        let base = func as usize + 1;
                        top.saturating_sub(base)
                    } else {
                        nargs as usize
                    };
                    let args: Vec<Value> = (0..actual_nargs)
                        .map(|i| frame.get(func + 1 + i as u8))
                        .collect();
                    let return_dst = func as usize;

                    match func_val {
                        Value::Function(f) => match f.state() {
                            FunctionState::Lua(lf) => {
                                if self.frames.len() >= MAX_STACK_DEPTH {
                                    return Err(VmError::StackOverflow);
                                }
                                validate_args(&lf.proto.signature, &args)?;
                                // Record return info on the current (caller) frame.
                                if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                    caller.return_dst = return_dst;
                                    caller.pending_nresults = nresults;
                                }
                                let new_frame =
                                    make_lua_frame(lf.proto.clone(), lf.upvalues.clone(), args);
                                self.frames.push(CallFrame::Lua(new_frame));
                            }
                            FunctionState::Native(nf) => {
                                validate_args(&nf.signature, &args)?;
                                // Record return info on the caller.
                                if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                    caller.return_dst = return_dst;
                                    caller.pending_nresults = nresults;
                                }
                                let ctx = self.build_call_context(Some(nf.signature.name.clone()));
                                let fut = (nf.call)(ctx, args);
                                self.pending_kind = PendingKind::NativeCall;
                                self.pending_nresults = nresults;
                                self.pending_dst = return_dst;
                                self.frames.push(CallFrame::Native(NativeFrame {
                                    signature: nf.signature.clone(),
                                    call_site: None,
                                }));
                                return Ok(Step::Yield(fut));
                            }
                        },
                        Value::Table(tab) => {
                            // Check __call metamethod.
                            match tab.get_metamethod("__call") {
                                Some(Value::Function(mm_fn)) => {
                                    // Prepend the table itself as the first arg.
                                    let mut mm_args = vec![Value::Table(tab)];
                                    mm_args.extend(args);
                                    if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                        caller.return_dst = return_dst;
                                        caller.pending_nresults = nresults;
                                    }
                                    match dispatch_metamethod(
                                        &mut self.frames,
                                        &self.global,
                                        &self.parent_stack,
                                        mm_fn,
                                        mm_args,
                                        nresults,
                                        return_dst,
                                        false,
                                    )? {
                                        None => {}
                                        Some(fut) => {
                                            self.pending_kind = PendingKind::NativeCall;
                                            self.pending_nresults = nresults;
                                            self.pending_dst = return_dst;
                                            return Ok(Step::Yield(fut));
                                        }
                                    }
                                }
                                _ => {
                                    return Err(VmError::CallNonFunction {
                                        type_name: "table",
                                        name: frame.register_name(func),
                                    });
                                }
                            }
                        }
                        other => {
                            return Err(VmError::CallNonFunction {
                                type_name: other.type_name(),
                                name: frame.register_name(func),
                            });
                        }
                    }
                }

                Instruction::Return { base, nresults } => {
                    let coerce = frame.coerce_result_to_bool;
                    let raw_results: Vec<Value> = if nresults < 0 {
                        (base as usize..frame.registers.len())
                            .map(|i| frame.registers.get(i).cloned().unwrap_or(Value::Nil))
                            .collect()
                    } else {
                        (0..nresults as usize)
                            .map(|i| {
                                frame
                                    .registers
                                    .get(base as usize + i)
                                    .cloned()
                                    .unwrap_or(Value::Nil)
                            })
                            .collect()
                    };
                    let results = if coerce {
                        let b = raw_results.first().map(|v| v.is_truthy()).unwrap_or(false);
                        vec![Value::Boolean(b)]
                    } else {
                        raw_results
                    };

                    // Pop the callee frame.
                    self.frames.pop();

                    if self.frames.is_empty() {
                        return Ok(Step::Done(results));
                    }

                    // Read return coordinates from the CALLER frame (now on top).
                    let (return_dst, pending_nresults) = match self.frames.last() {
                        Some(CallFrame::Lua(f)) => (f.return_dst, f.pending_nresults),
                        _ => (0, -1),
                    };
                    self.write_return_values(results, return_dst, pending_nresults);
                }

                Instruction::CollectGarbage => {
                    self.global.collect_cycles();
                }

                Instruction::GetUpval { dst, upval } => {
                    let val = frame
                        .upvalues
                        .get(upval as usize)
                        .map(|cell| cell.read().clone())
                        .unwrap_or(Value::Nil);
                    frame.set(dst, val);
                }
                Instruction::SetUpval { upval, src } => {
                    let val = frame.get(src);
                    if let Some(cell) = frame.upvalues.get(upval as usize) {
                        *cell.write() = val;
                    }
                }

                Instruction::GetTable { dst, table, key } => {
                    let t = frame.get(table);
                    let k = frame.get(key);
                    match t {
                        Value::Table(tab) => {
                            let v = tab.raw_get(&k)?;
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
                                    Some(Value::Table(idx_tab)) => {
                                        match index_table_chain(idx_tab, &k, 100)? {
                                            Some(v) => {
                                                frame.set(dst, v);
                                            }
                                            None => {
                                                // Chain ended at a function __index
                                                // — we fall through.
                                                // (Rare: mixed table/function chain)
                                                frame.set(dst, Value::Nil);
                                            }
                                        }
                                    }
                                    Some(Value::Function(mm_fn)) => {
                                        let mm_args = vec![Value::Table(tab), k];
                                        if let Some(CallFrame::Lua(caller)) = self.frames.last_mut()
                                        {
                                            caller.return_dst = dst as usize;
                                            caller.pending_nresults = 1;
                                        }
                                        match dispatch_metamethod(
                                            &mut self.frames,
                                            &self.global,
                                            &self.parent_stack,
                                            mm_fn,
                                            mm_args,
                                            1,
                                            dst as usize,
                                            false,
                                        )? {
                                            None => {}
                                            Some(fut) => {
                                                self.pending_kind = PendingKind::NativeCall;
                                                self.pending_nresults = 1;
                                                self.pending_dst = dst as usize;
                                                return Ok(Step::Yield(fut));
                                            }
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
                            let ctx = self.build_call_context(None);
                            let fut = Arc::clone(&ud).dispatch(ctx, "__index", args);
                            self.pending_kind = PendingKind::NativeCall;
                            self.pending_nresults = 1;
                            self.pending_dst = dst as usize;
                            if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                caller.return_dst = dst as usize;
                                caller.pending_nresults = 1;
                            }
                            self.frames.push(CallFrame::Native(NativeFrame {
                                signature: Arc::new(FunctionSignature {
                                    name: bytes::Bytes::from_static(b"__index"),
                                    type_params: vec![],
                                    params: vec![],
                                    variadic: true,
                                    arg_offset: 0,
                                    returns: None,
                                    lua_returns: None,
                                }),
                                call_site: None,
                            }));
                            return Ok(Step::Yield(Box::pin(fut)));
                        }
                        Value::String(_) => {
                            // Consult the shared string metatable so that
                            // method-call syntax like ("hello"):upper() works.
                            if let Some(mt) = self.global.get_string_metatable() {
                                let index_key = Value::string("__index");
                                let mm = mt.raw_get(&index_key).ok().filter(|v| !v.is_nil());
                                match mm {
                                    Some(Value::Table(idx_tab)) => {
                                        match index_table_chain(idx_tab, &k, 100)? {
                                            Some(v) => frame.set(dst, v),
                                            None => frame.set(dst, Value::Nil),
                                        }
                                    }
                                    Some(Value::Function(mm_fn)) => {
                                        let mm_args = vec![t, k];
                                        if let Some(CallFrame::Lua(caller)) = self.frames.last_mut()
                                        {
                                            caller.return_dst = dst as usize;
                                            caller.pending_nresults = 1;
                                        }
                                        match dispatch_metamethod(
                                            &mut self.frames,
                                            &self.global,
                                            &self.parent_stack,
                                            mm_fn,
                                            mm_args,
                                            1,
                                            dst as usize,
                                            false,
                                        )? {
                                            None => {}
                                            Some(fut) => {
                                                self.pending_kind = PendingKind::NativeCall;
                                                self.pending_nresults = 1;
                                                self.pending_dst = dst as usize;
                                                return Ok(Step::Yield(fut));
                                            }
                                        }
                                    }
                                    _ => frame.set(dst, Value::Nil),
                                }
                            } else {
                                return Err(VmError::IndexNonTable {
                                    type_name: "string",
                                    name: frame.register_name(table),
                                });
                            }
                        }
                        other => {
                            return Err(VmError::IndexNonTable {
                                type_name: other.type_name(),
                                name: frame.register_name(table),
                            });
                        }
                    }
                }
                Instruction::SetTable { table, key, src } => {
                    let t = frame.get(table);
                    let k = frame.get(key);
                    let v = frame.get(src);
                    match t {
                        Value::Table(tab) => {
                            // __newindex is only triggered when the key is absent.
                            let existing = tab.raw_get(&k)?;
                            if !existing.is_nil() {
                                // Key already exists — raw write, no metamethod.
                                tab.raw_set(k, v)?;
                            } else {
                                let mm = tab.get_metamethod("__newindex");
                                match mm {
                                    None => {
                                        tab.raw_set(k, v)?;
                                    }
                                    Some(Value::Table(dst_tab)) => {
                                        // __newindex is a table: write into it.
                                        dst_tab.raw_set(k, v)?;
                                    }
                                    Some(Value::Function(mm_fn)) => {
                                        let mm_args = vec![Value::Table(tab), k, v];
                                        // __newindex result is discarded (0 results).
                                        if let Some(CallFrame::Lua(caller)) = self.frames.last_mut()
                                        {
                                            caller.return_dst = 0;
                                            caller.pending_nresults = 0;
                                        }
                                        match dispatch_metamethod(
                                            &mut self.frames,
                                            &self.global,
                                            &self.parent_stack,
                                            mm_fn,
                                            mm_args,
                                            0,
                                            0,
                                            false,
                                        )? {
                                            None => {}
                                            Some(fut) => {
                                                self.pending_kind = PendingKind::NativeCall;
                                                self.pending_nresults = 0;
                                                self.pending_dst = 0;
                                                return Ok(Step::Yield(fut));
                                            }
                                        }
                                    }
                                    Some(_) => {
                                        // Unknown __newindex type: raw write.
                                        tab.raw_set(k, v)?;
                                    }
                                }
                            }
                        }
                        Value::Userdata(ud) => {
                            // Dispatch __newindex on userdata.
                            let args = vec![Value::Userdata(Arc::clone(&ud)), k, v];
                            let ctx = self.build_call_context(None);
                            let fut = Arc::clone(&ud).dispatch(ctx, "__newindex", args);
                            self.pending_kind = PendingKind::NativeCall;
                            self.pending_nresults = 0;
                            self.pending_dst = 0;
                            if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                caller.return_dst = 0;
                                caller.pending_nresults = 0;
                            }
                            self.frames.push(CallFrame::Native(NativeFrame {
                                signature: Arc::new(FunctionSignature {
                                    name: bytes::Bytes::from_static(b"__newindex"),
                                    type_params: vec![],
                                    params: vec![],
                                    variadic: true,
                                    arg_offset: 0,
                                    returns: None,
                                    lua_returns: None,
                                }),
                                call_site: None,
                            }));
                            return Ok(Step::Yield(Box::pin(fut)));
                        }
                        other => {
                            return Err(VmError::IndexNonTable {
                                type_name: other.type_name(),
                                name: frame.register_name(table),
                            });
                        }
                    }
                }
                Instruction::NewTable { dst, .. } => {
                    let t = Table::new();
                    self.global.track_table(&t);
                    frame.set(dst, Value::Table(t));
                }
                Instruction::SetList {
                    table,
                    src_base,
                    count,
                    array_start,
                } => {
                    let t = match frame.get(table) {
                        Value::Table(t) => t,
                        other => {
                            return Err(VmError::IndexNonTable {
                                type_name: other.type_name(),
                                name: None,
                            });
                        }
                    };
                    let n = if count < 0 {
                        frame.registers.len().saturating_sub(src_base as usize)
                    } else {
                        count as usize
                    };
                    for i in 0..n {
                        let v = frame
                            .registers
                            .get(src_base as usize + i)
                            .cloned()
                            .unwrap_or(Value::Nil);
                        t.raw_set(Value::Integer(array_start + i as i64), v)?;
                    }
                }
                Instruction::NewClosure { dst, proto_idx } => {
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
                            let cell = if let Some((_, c)) =
                                frame.open_upvalues.iter().find(|(s, _)| *s == slot)
                            {
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
                                    .unwrap_or_else(|| {
                                        Arc::new(parking_lot::RwLock::new(Value::Nil))
                                    }),
                            );
                        }
                    }
                    let func = Function::lua(child_proto, upvalues);
                    self.global.track_function(&func);
                    frame.set(dst, Value::Function(func));
                }
                Instruction::Concat { dst, base, count } => {
                    // Collect all operand values up front.
                    let vals: Vec<Value> = (0..count).map(|i| frame.get(base + i)).collect();
                    // Try the fast path: all operands are strings or numbers.
                    let mut buf = bytes::BytesMut::new();
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
                        frame.set(dst, Value::String(buf.freeze()));
                    } else {
                        // At least one operand isn't a string/number.
                        // The compiler always emits count=2; support __concat for that case.
                        let lhs = vals[0].clone();
                        let rhs = vals[1].clone();
                        match get_arith_metamethod(&lhs, &rhs, b"__concat") {
                            Some(mm_fn) => {
                                let d = dst as usize;
                                if let Some(CallFrame::Lua(c)) = self.frames.last_mut() {
                                    c.return_dst = d;
                                    c.pending_nresults = 1;
                                }
                                match dispatch_metamethod(
                                    &mut self.frames,
                                    &self.global,
                                    &self.parent_stack,
                                    mm_fn,
                                    vec![lhs, rhs],
                                    1,
                                    d,
                                    false,
                                )? {
                                    None => {}
                                    Some(fut) => {
                                        self.pending_kind = PendingKind::NativeCall;
                                        self.pending_nresults = 1;
                                        self.pending_dst = d;
                                        return Ok(Step::Yield(fut));
                                    }
                                }
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
                                let fail_idx =
                                    coerce_fail.expect("inside coerce_fail.is_some() branch");
                                let fail_slot = base + fail_idx as u8;
                                return Err(VmError::ConcatenationError {
                                    type_name,
                                    name: frame.register_name(fail_slot),
                                });
                            }
                        }
                    }
                }
                Instruction::CloseVar { slot } => {
                    let val = frame.get(slot);
                    // Nil the slot immediately to prevent double-close.
                    frame.set(slot, Value::Nil);
                    if let Some(fut) = close_future(val, &self.global, self.parent_stack.clone()) {
                        self.pending_kind = PendingKind::CloseVar;
                        return Ok(Step::Yield(fut));
                    }
                }
                // Labels are no-ops at runtime.
                Instruction::Label { .. } => {}
                // Goto must have been resolved to Jump during compilation.
                Instruction::Goto { .. } => {
                    return Err(VmError::ArithmeticOnNonNumber {
                        type_name: "unresolved Goto in bytecode (compiler bug)",
                        name: None,
                    });
                }
                Instruction::Len { dst, src } => {
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
                                    if let Some(CallFrame::Lua(caller)) = self.frames.last_mut() {
                                        caller.return_dst = dst as usize;
                                        caller.pending_nresults = 1;
                                    }
                                    match dispatch_metamethod(
                                        &mut self.frames,
                                        &self.global,
                                        &self.parent_stack,
                                        mm_fn,
                                        mm_args,
                                        1,
                                        dst as usize,
                                        false,
                                    )? {
                                        None => {}
                                        Some(fut) => {
                                            self.pending_kind = PendingKind::NativeCall;
                                            self.pending_nresults = 1;
                                            self.pending_dst = dst as usize;
                                            return Ok(Step::Yield(fut));
                                        }
                                    }
                                }
                                Some(_) => {
                                    let n = tab.raw_len();
                                    frame.set(dst, Value::Integer(n));
                                }
                            }
                        }
                        _ => {
                            return Err(VmError::IndexNonTable {
                                type_name: v.type_name(),
                                name: frame.register_name(src),
                            });
                        }
                    }
                }
                Instruction::Vararg { dst, nresults } => {
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
            }
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
                let frame = make_lua_frame(lf.proto.clone(), lf.upvalues.clone(), args);
                Task {
                    inner: TaskInner {
                        global,
                        frames: vec![CallFrame::Lua(frame)],
                        pending: None,
                        pending_kind: PendingKind::NativeCall,
                        pending_nresults: -1,
                        pending_dst: 0,
                        parent_stack,
                        unwind_error: validation_err,
                        unwind_close_vals: Vec::new(),
                    },
                }
            }
            FunctionState::Native(nf) => {
                // No Lua frames yet; build a context with the inherited parent
                // stack plus this native's own name.
                let ctx = CallContext {
                    global: global.clone(),
                    call_stack: parent_stack.clone(),
                    native_name: Some(nf.signature.name.clone()),
                };
                let fut = (nf.call)(ctx, args);
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
                    },
                }
            }
        }
    }
}

impl std::future::Future for Task {
    type Output = Result<Vec<Value>, VmError>;

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
/// Look up an arithmetic metamethod (`__add`, `__sub`, …) on either operand.
///
/// TODO: extend this to handle `Value::Userdata` operands by synchronously
/// returning a synthetic `NativeFunction` that delegates to `ud.dispatch()`.
/// Until then, arithmetic operators on userdata values always raise a runtime
/// error instead of consulting the metamethod.
fn get_arith_metamethod(
    lhs: &Value,
    rhs: &Value,
    event: &[u8],
) -> Option<crate::function::Function> {
    if let Value::Table(t) = lhs {
        if let Some(Value::Function(f)) = t.get_metamethod(event) {
            return Some(f);
        }
    }
    if let Value::Table(t) = rhs {
        if let Some(Value::Function(f)) = t.get_metamethod(event) {
            return Some(f);
        }
    }
    None
}

/// Follow the `__index` chain for purely-table metamethods, returning the
/// first non-nil value found or `Value::Nil`.  Stops when a function
/// `__index` is encountered — the caller must dispatch that asynchronously.
/// Returns `Err` if the chain exceeds the depth limit.
fn index_table_chain(
    mut table: crate::table::Table,
    key: &Value,
    depth: usize,
) -> Result<Option<Value>, VmError> {
    for _ in 0..depth {
        let v = table.raw_get(key)?;
        if !v.is_nil() {
            return Ok(Some(v));
        }
        match table.get_metamethod("__index") {
            None => return Ok(Some(Value::Nil)),
            Some(Value::Table(next)) => table = next,
            Some(_other) => {
                // Function (or other) __index — caller must dispatch.
                return Ok(None);
            }
        }
    }
    Err(VmError::ArithmeticOnNonNumber {
        type_name: "'__index' chain too long",
        name: None,
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
            let mut new_frame = make_lua_frame(lf.proto.clone(), lf.upvalues.clone(), args);
            new_frame.coerce_result_to_bool = coerce_to_bool;
            frames.push(CallFrame::Lua(new_frame));
            Ok(None)
        }
        FunctionState::Native(nf) => {
            validate_args(&nf.signature, &args)?;
            // Build CallContext from the current stack snapshot.
            let mut call_stack: Vec<crate::call_context::StackFrame> = (**parent_stack).clone();
            for cf in frames.iter() {
                if let CallFrame::Lua(f) = cf {
                    let source_location =
                        f.pc.checked_sub(1)
                            .and_then(|pc| f.proto.source_locations.get(pc))
                            .and_then(|s| s.clone());
                    call_stack.push(crate::call_context::StackFrame::Lua {
                        function_name: f.proto.signature.name.clone(),
                        source_location,
                        locals: vec![],
                    });
                }
            }
            let ctx = CallContext {
                global: global.clone(),
                call_stack: std::sync::Arc::new(call_stack),
                native_name: Some(nf.signature.name.clone()),
            };
            let raw_fut = (nf.call)(ctx, args);
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
                native_name: Some(bytes::Bytes::from_static(b"__close")),
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

/// Build a `LuaFrame` for the given proto, upvalues, and arguments.
///
/// The first `param_count` args are loaded into registers; any extras become
/// `varargs` (only when `proto.signature.variadic` is true).
fn make_lua_frame(proto: Arc<Proto>, upvalues: Vec<UpvalueCell>, args: Vec<Value>) -> LuaFrame {
    let param_count = proto.signature.params.len();
    let varargs = if proto.signature.variadic && args.len() > param_count {
        args[param_count..].to_vec()
    } else {
        vec![]
    };
    let mut regs = vec![Value::Nil; param_count];
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
        if let Some(rt) = &param.runtime_type {
            let v = args.get(offset + i).unwrap_or(&Value::Nil);
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
