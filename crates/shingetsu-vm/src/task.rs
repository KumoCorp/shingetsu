use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use futures::future::BoxFuture;

use crate::{
    error::VmError,
    function::{Function, FunctionState, UpvalueCell},
    global_env::GlobalEnv,
    ir::Instruction,
    proto::{Proto, SourceLocation},
    table::Table,
    types::{FunctionSignature, ValueType},
    value::Value,
};

// ---------------------------------------------------------------------------
// Call frames
// ---------------------------------------------------------------------------

pub struct LuaFrame {
    pub proto: Arc<Proto>,
    pub pc: usize,
    pub registers: Vec<Value>,
    pub upvalues: Vec<UpvalueCell>,
    pub call_site: Option<SourceLocation>,
    /// Register slot where call results should be written when this frame
    /// returns (set by the parent frame's `Call` handler).
    pub return_dst: usize,
    /// Number of results the caller expects (-1 = all).
    pub pending_nresults: i32,
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
    /// A `__close` metamethod dispatch; results are discarded.
    CloseVar,
}

struct TaskInner {
    global: GlobalEnv,
    frames: Vec<CallFrame>,
    pending: Option<BoxFuture<'static, Result<Vec<Value>, VmError>>>,
    pending_kind: PendingKind,
    /// nresults expected by the frame that launched the currently-pending
    /// native call (unused for CloseVar).
    pending_nresults: i32,
    /// Return-register slot in the Lua caller frame for the current pending
    /// native call (unused for CloseVar).
    pending_dst: usize,
}

const MAX_STACK_DEPTH: usize = 200;

impl TaskInner {
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
        // Resize to exactly dst + n when nresults is variable (-1), so that
        // a subsequent `Return { nresults: -1 }` can use registers.len() as
        // the upper bound to collect exactly the right number of values.
        let needed = dst + n;
        caller.registers.resize(needed, Value::Nil);
        for (i, v) in values.into_iter().enumerate().take(n) {
            caller.registers[dst + i] = v;
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
                let (return_dst, pending_nresults) =
                    match self.frames.last() {
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
                    set_reg(&mut frame.registers, dst, Value::Nil);
                }
                Instruction::LoadBool { dst, value } => {
                    set_reg(&mut frame.registers, dst, Value::Boolean(value));
                }
                Instruction::LoadInt { dst, value } => {
                    set_reg(&mut frame.registers, dst, Value::Integer(value));
                }
                Instruction::LoadFloat { dst, value } => {
                    set_reg(&mut frame.registers, dst, Value::Float(value));
                }
                Instruction::LoadK { dst, idx } => {
                    let c = frame.proto.constants[idx as usize].clone();
                    set_reg(&mut frame.registers, dst, Value::String(c));
                }
                Instruction::Move { dst, src } => {
                    let v = get_reg(&frame.registers, src);
                    set_reg(&mut frame.registers, dst, v);
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
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::SetGlobal { name, src } => {
                    let key = frame.proto.constants[name as usize].clone();
                    let v = get_reg(&frame.registers, src);
                    self.global.0.globals.insert(key, v);
                }
                Instruction::Jump { offset } => {
                    apply_offset(&mut frame.pc, offset);
                }
                Instruction::BranchFalse { src, offset } => {
                    if !get_reg(&frame.registers, src).is_truthy() {
                        apply_offset(&mut frame.pc, offset);
                    }
                }
                Instruction::BranchTrue { src, offset } => {
                    if get_reg(&frame.registers, src).is_truthy() {
                        apply_offset(&mut frame.pc, offset);
                    }
                }

                // Arithmetic
                Instruction::Add { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_add(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Sub { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_sub(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Mul { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_mul(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Div { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_div(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::IDiv { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_idiv(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Mod { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_mod(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Pow { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_pow(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Neg { dst, src } => {
                    let v = get_reg(&frame.registers, src).arith_neg()?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::BAnd { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_band(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::BOr { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_bor(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::BXor { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_bxor(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::BNot { dst, src } => {
                    let v = get_reg(&frame.registers, src).arith_bnot()?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Shl { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_shl(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Shr { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        .arith_shr(&get_reg(&frame.registers, rhs))?;
                    set_reg(&mut frame.registers, dst, v);
                }
                Instruction::Not { dst, src } => {
                    let v = !get_reg(&frame.registers, src).is_truthy();
                    set_reg(&mut frame.registers, dst, Value::Boolean(v));
                }

                // Comparison
                Instruction::Eq { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        == get_reg(&frame.registers, rhs);
                    set_reg(&mut frame.registers, dst, Value::Boolean(v));
                }
                Instruction::Ne { dst, lhs, rhs } => {
                    let v = get_reg(&frame.registers, lhs)
                        != get_reg(&frame.registers, rhs);
                    set_reg(&mut frame.registers, dst, Value::Boolean(v));
                }
                Instruction::Lt { dst, lhs, rhs } => {
                    let v = compare_lt(
                        &get_reg(&frame.registers, lhs),
                        &get_reg(&frame.registers, rhs),
                    )?;
                    set_reg(&mut frame.registers, dst, Value::Boolean(v));
                }
                Instruction::Le { dst, lhs, rhs } => {
                    let v = compare_le(
                        &get_reg(&frame.registers, lhs),
                        &get_reg(&frame.registers, rhs),
                    )?;
                    set_reg(&mut frame.registers, dst, Value::Boolean(v));
                }
                Instruction::Gt { dst, lhs, rhs } => {
                    let v = compare_lt(
                        &get_reg(&frame.registers, rhs),
                        &get_reg(&frame.registers, lhs),
                    )?;
                    set_reg(&mut frame.registers, dst, Value::Boolean(v));
                }
                Instruction::Ge { dst, lhs, rhs } => {
                    let v = compare_le(
                        &get_reg(&frame.registers, rhs),
                        &get_reg(&frame.registers, lhs),
                    )?;
                    set_reg(&mut frame.registers, dst, Value::Boolean(v));
                }

                // Numeric for
                Instruction::ForPrep {
                    counter,
                    limit,
                    step,
                    exit_offset,
                } => {
                    if for_prep(&mut frame.registers, counter, limit, step)? {
                        apply_offset(&mut frame.pc, exit_offset);
                    }
                }
                Instruction::ForStep {
                    counter,
                    limit,
                    step,
                    body_offset,
                } => {
                    if for_step(&mut frame.registers, counter, limit, step)? {
                        apply_offset(&mut frame.pc, body_offset);
                    }
                }

                // Function call
                Instruction::Call {
                    func,
                    nargs,
                    nresults,
                } => {
                    let func_val = get_reg(&frame.registers, func);
                    let args: Vec<Value> = (0..nargs)
                        .map(|i| get_reg(&frame.registers, func + 1 + i as u8))
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
                                if let Some(CallFrame::Lua(caller)) =
                                    self.frames.last_mut()
                                {
                                    caller.return_dst = return_dst;
                                    caller.pending_nresults = nresults;
                                }
                                let param_count =
                                    lf.proto.signature.params.len();
                                let mut regs =
                                    Vec::with_capacity(args.len().max(param_count));
                                regs.resize(args.len().max(param_count), Value::Nil);
                                for (i, a) in args.into_iter().enumerate() {
                                    regs[i] = a;
                                }
                                self.frames.push(CallFrame::Lua(LuaFrame {
                                    proto: lf.proto.clone(),
                                    pc: 0,
                                    registers: regs,
                                    upvalues: lf.upvalues.clone(),
                                    call_site: None,
                                    return_dst: 0,
                                    pending_nresults: -1,
                                }));
                            }
                            FunctionState::Native(nf) => {
                                validate_args(&nf.signature, &args)?;
                                // Record return info on the caller.
                                if let Some(CallFrame::Lua(caller)) =
                                    self.frames.last_mut()
                                {
                                    caller.return_dst = return_dst;
                                    caller.pending_nresults = nresults;
                                }
                                let fut = (nf.call)(args);
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
                        other => {
                            return Err(VmError::CallNonFunction {
                                type_name: other.type_name(),
                            });
                        }
                    }
                }

                Instruction::Return { base, nresults } => {
                    let results: Vec<Value> = if nresults < 0 {
                        (base as usize..frame.registers.len())
                            .map(|i| {
                                frame
                                    .registers
                                    .get(i)
                                    .cloned()
                                    .unwrap_or(Value::Nil)
                            })
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

                    // Pop the callee frame.
                    self.frames.pop();

                    if self.frames.is_empty() {
                        return Ok(Step::Done(results));
                    }

                    // Read return coordinates from the CALLER frame (now on top).
                    let (return_dst, pending_nresults) =
                        match self.frames.last() {
                            Some(CallFrame::Lua(f)) => (f.return_dst, f.pending_nresults),
                            _ => (0, -1),
                        };
                    self.write_return_values(results, return_dst, pending_nresults);
                }

                Instruction::CollectGarbage => {
                    self.global.collect_cycles();
                }

                // Upvalues (Phase 3).
                Instruction::GetUpval { dst, upval: _ } => {
                    set_reg(&mut frame.registers, dst, Value::Nil);
                }
                Instruction::SetUpval { .. } => {}

                Instruction::GetTable { dst, table, key } => {
                    let t = get_reg(&frame.registers, table);
                    let k = get_reg(&frame.registers, key);
                    match t {
                        Value::Table(tab) => {
                            let v = tab.raw_get(&k)?;
                            set_reg(&mut frame.registers, dst, v);
                        }
                        other => {
                            return Err(VmError::IndexNonTable {
                                type_name: other.type_name(),
                            });
                        }
                    }
                }
                Instruction::SetTable { table, key, src } => {
                    let t = get_reg(&frame.registers, table);
                    let k = get_reg(&frame.registers, key);
                    let v = get_reg(&frame.registers, src);
                    match t {
                        Value::Table(tab) => {
                            tab.raw_set(k, v)?;
                        }
                        other => {
                            return Err(VmError::IndexNonTable {
                                type_name: other.type_name(),
                            });
                        }
                    }
                }
                Instruction::NewTable { dst, .. } => {
                    set_reg(
                        &mut frame.registers,
                        dst,
                        Value::Table(Table::new()),
                    );
                }
                Instruction::NewClosure { dst, proto_idx } => {
                    let child_proto = frame
                        .proto
                        .protos
                        .get(proto_idx as usize)
                        .cloned()
                        .unwrap_or_else(|| frame.proto.clone());
                    let func = Function::lua(child_proto, vec![]);
                    set_reg(&mut frame.registers, dst, Value::Function(func));
                }
                Instruction::Concat { dst, base, count } => {
                    let mut buf = bytes::BytesMut::new();
                    for i in 0..count {
                        let v = get_reg(&frame.registers, base + i);
                        buf.extend_from_slice(v.to_string().as_bytes());
                    }
                    set_reg(
                        &mut frame.registers,
                        dst,
                        Value::String(buf.freeze()),
                    );
                }
                Instruction::CloseVar { slot } => {
                    let val = get_reg(&frame.registers, slot);
                    // Nil the slot immediately to prevent double-close.
                    set_reg(&mut frame.registers, slot, Value::Nil);
                    if let Value::Userdata(ud) = val {
                        let ud2 = ud.clone();
                        let fut = ud.dispatch("__close", vec![Value::Userdata(ud2)]);
                        self.pending_kind = PendingKind::CloseVar;
                        return Ok(Step::Yield(fut));
                    }
                    // Non-userdata <close> values (tables with __close
                    // metamethods, Phase 3): nothing to do yet.
                }
                // Labels are no-ops at runtime.
                Instruction::Label { .. } => {}
                // Goto must have been resolved to Jump during compilation.
                Instruction::Goto { .. } => {
                    return Err(VmError::ArithmeticOnNonNumber {
                        type_name: "unresolved Goto in bytecode (compiler bug)",
                    });
                }
                Instruction::Len { dst, src } => {
                    let v = get_reg(&frame.registers, src);
                    let n = match &v {
                        Value::String(s) => s.len() as i64,
                        Value::Table(t) => t.raw_len(),
                        _ => {
                            return Err(VmError::IndexNonTable {
                                type_name: v.type_name(),
                            });
                        }
                    };
                    set_reg(&mut frame.registers, dst, Value::Integer(n));
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
    /// Create a new task.  This is `pub` so the CLI and host code can construct tasks
    /// without going through `GlobalEnv::task` (which requires a global function name).
    pub fn new(global: GlobalEnv, func: Function, args: Vec<Value>) -> Self {
        match func.state() {
            FunctionState::Lua(lf) => {
                let param_count = lf.proto.signature.params.len();
                let cap = args.len().max(param_count);
                let mut regs = vec![Value::Nil; cap];
                for (i, a) in args.into_iter().enumerate() {
                    if i < regs.len() {
                        regs[i] = a;
                    }
                }
                Task {
                    inner: TaskInner {
                        global,
                        frames: vec![CallFrame::Lua(LuaFrame {
                            proto: lf.proto.clone(),
                            pc: 0,
                            registers: regs,
                            upvalues: lf.upvalues.clone(),
                            call_site: None,
                            return_dst: 0,
                            pending_nresults: -1,
                        })],
                        pending: None,
                        pending_kind: PendingKind::NativeCall,
                        pending_nresults: -1,
                        pending_dst: 0,
                    },
                }
            }
            FunctionState::Native(nf) => {
                let fut = (nf.call)(args);
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
            if let Some(fut) = &mut self.inner.pending {
                match fut.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(result) => {
                        let values = match result {
                            Ok(v) => v,
                            Err(e) => return Poll::Ready(Err(e)),
                        };
                        self.inner.pending = None;
                        match self.inner.pending_kind {
                            PendingKind::NativeCall => {
                                // Pop NativeFrame and write results back.
                                self.inner.frames.pop();
                                if self.inner.frames.is_empty() {
                                    return Poll::Ready(Ok(values));
                                }
                                let dst = self.inner.pending_dst;
                                let nresults = self.inner.pending_nresults;
                                self.inner.write_return_values(values, dst, nresults);
                            }
                            PendingKind::CloseVar => {
                                // __close results are discarded; no frame to pop.
                            }
                        }
                    }
                }
            }

            match self.inner.step() {
                Ok(Step::Done(v)) => return Poll::Ready(Ok(v)),
                Ok(Step::Yield(fut)) => {
                    self.inner.pending = Some(fut);
                    // Loop to poll the new future immediately.
                }
                Err(e) => return Poll::Ready(Err(e)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_reg(regs: &[Value], idx: u8) -> Value {
    regs.get(idx as usize).cloned().unwrap_or(Value::Nil)
}

fn set_reg(regs: &mut Vec<Value>, idx: u8, v: Value) {
    let i = idx as usize;
    if regs.len() <= i {
        regs.resize(i + 1, Value::Nil);
    }
    regs[i] = v;
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
            rhs: b.type_name(),
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
            rhs: b.type_name(),
        }),
    }
}

/// Returns `true` if the loop should be skipped (counter already past limit).
fn for_prep(
    regs: &mut Vec<Value>,
    counter: u8,
    limit: u8,
    step: u8,
) -> Result<bool, VmError> {
    let c = get_reg(regs, counter);
    let l = get_reg(regs, limit);
    let s = get_reg(regs, step);

    if let (Value::Integer(ci), Value::Integer(li), Value::Integer(si)) = (&c, &l, &s) {
        if *si == 0 {
            return Err(VmError::ArithmeticOnNonNumber {
                type_name: "zero step in numeric for",
            });
        }
        return Ok(if *si > 0 { ci > li } else { ci < li });
    }

    let (cf, lf, sf) = match (c.to_float(), l.to_float(), s.to_float()) {
        (Some(c), Some(l), Some(s)) => (c, l, s),
        _ => {
            return Err(VmError::ArithmeticOnNonNumber {
                type_name: "non-numeric for loop bound",
            });
        }
    };
    if sf == 0.0 {
        return Err(VmError::ArithmeticOnNonNumber {
            type_name: "zero step in numeric for",
        });
    }
    set_reg(regs, counter, Value::Float(cf));
    set_reg(regs, limit, Value::Float(lf));
    set_reg(regs, step, Value::Float(sf));
    Ok(if sf > 0.0 { cf > lf } else { cf < lf })
}

/// Returns `true` if the loop should continue (counter still in range).
fn for_step(
    regs: &mut Vec<Value>,
    counter: u8,
    limit: u8,
    step: u8,
) -> Result<bool, VmError> {
    match (
        get_reg(regs, counter),
        get_reg(regs, limit),
        get_reg(regs, step),
    ) {
        (Value::Integer(ci), Value::Integer(li), Value::Integer(si)) => {
            let next = ci.wrapping_add(si);
            set_reg(regs, counter, Value::Integer(next));
            Ok(if si > 0 { next <= li } else { next >= li })
        }
        (c, l, s) => {
            let cf = c.to_float().expect("float counter");
            let lf = l.to_float().expect("float limit");
            let sf = s.to_float().expect("float step");
            let next = cf + sf;
            set_reg(regs, counter, Value::Float(next));
            Ok(if sf > 0.0 { next <= lf } else { next >= lf })
        }
    }
}

/// Validate `args` against the runtime-typed parameters declared in `sig`.
/// Parameters with no `runtime_type` annotation are unconstrained and skipped.
/// A signature with no annotated parameters passes without any checks.
fn validate_args(sig: &FunctionSignature, args: &[Value]) -> Result<(), VmError> {
    for (i, param) in sig.params.iter().enumerate() {
        if let Some(rt) = &param.runtime_type {
            let v = args.get(i).unwrap_or(&Value::Nil);
            if !value_matches_type(v, rt) {
                return Err(VmError::BadArgument {
                    position: i + 1,
                    function: String::from_utf8_lossy(&sig.name).into_owned(),
                    expected: rt.type_name(),
                    got: v.type_name(),
                });
            }
        }
    }
    Ok(())
}

fn value_matches_type(v: &Value, rt: &ValueType) -> bool {
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
