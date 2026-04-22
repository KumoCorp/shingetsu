use std::sync::Arc;

use bytes::Bytes;

use crate::function::Function;
use crate::global_env::GlobalEnv;
use crate::proto::SourceLocation;
use crate::types::FunctionSignature;
use crate::value::Value;

/// A single frame in a Lua/native call stack snapshot.
#[derive(Clone, Debug)]
pub enum StackFrame {
    /// A Lua function frame.
    Lua {
        /// The signature of the running Lua function.  Carries the
        /// function's name, parameter names and types, return types,
        /// variadic flag, and generic type parameters — everything the
        /// debug library needs for `getinfo` / `info` / `traceback`
        /// signature-annotated rendering.
        function: Arc<FunctionSignature>,
        /// Source location of the instruction executing when the call was made.
        source_location: Option<SourceLocation>,
        /// Live local variables at the time of the call, in declaration order.
        /// Populated from `Proto::locals`; empty until the compiler emits debug
        /// info.
        locals: Vec<(Bytes, Value)>,
        /// Whether the most recent `Call` instruction from this frame used
        /// `:` syntax.  Used by `detect_hints` to suggest `.` vs `:` corrections.
        last_call_is_method: bool,
        /// Byte offset and length of the `.` or `:` token at the most recent
        /// call site.  Used by `detect_hints` to point the hint at the exact token.
        last_call_dot_colon: Option<(u32, u32)>,
        /// Byte offset of the start of the receiver expression at the most
        /// recent call site.  The receiver text is
        /// `source[receiver_offset..dot_colon_offset]`.
        last_call_receiver_offset: Option<u32>,
        /// Signature of the callee for the most recent `Call` instruction.
        /// Used by `detect_hints` when the callee frame is not on the stack
        /// (native functions, validate_args errors).
        last_call_callee_sig: Option<Arc<FunctionSignature>>,
    },
    /// A native (host) function frame.
    Native {
        /// Name of the native function.
        function_name: Bytes,
    },
}

/// Context passed to every native function call and userdata metamethod
/// dispatch.
///
/// All fields are cheaply cloneable, so `CallContext` can be moved into
/// `'static` async closures.
#[derive(Clone)]
pub struct CallContext {
    /// The shared global environment.
    pub global: GlobalEnv,
    /// Snapshot of the call stack at invocation time, outermost frame first.
    /// Includes frames inherited from any parent task (via `call_function`),
    /// followed by Lua frames from the current task.
    pub call_stack: Arc<Vec<StackFrame>>,
    /// Name of the native function currently executing, if known.  This is
    /// set by the VM when invoking a native and can be used to insert a
    /// `StackFrame::Native` entry when spawning a nested task via
    /// `call_function`.
    pub native_name: Option<Bytes>,
}

impl CallContext {
    /// Compute the length of a table, respecting `__len` metamethod.
    ///
    /// If the table has a `__len` metamethod, it is called and the first
    /// result is coerced to an integer.  Otherwise falls back to `raw_len`.
    pub async fn table_len(
        &self,
        table: &crate::table::Table,
    ) -> Result<i64, crate::error::VmError> {
        if let Some(Value::Function(mm)) = table.get_metamethod("__len") {
            let results = self
                .call_function(mm, vec![Value::Table(table.clone())])
                .await
                .map_err(|re| re.error)?;
            match results.into_iter().next().unwrap_or(Value::Nil) {
                Value::Integer(n) => Ok(n),
                Value::Float(f) => Ok(f as i64),
                other => {
                    let msg = format!(
                        "object length is not an integer (got {})",
                        other.type_name()
                    );
                    Err(crate::error::VmError::LuaError {
                        display: msg.clone(),
                        value: Value::string(msg),
                    })
                }
            }
        } else {
            Ok(table.raw_len())
        }
    }

    /// Read a value from a table by key, respecting `__index` metamethod.
    ///
    /// If the key is absent and the table has an `__index` metamethod, it
    /// is dispatched (table chain or function call).  Mirrors the VM's
    /// `GetTable` instruction semantics.
    pub async fn table_get(
        &self,
        table: &crate::table::Table,
        key: &Value,
    ) -> Result<Value, crate::error::VmError> {
        let v = table.raw_get(key)?;
        if !v.is_nil() {
            return Ok(v);
        }
        // Walk __index chain.
        let mut current = table.clone();
        for _ in 0..crate::METAMETHOD_CHAIN_LIMIT {
            match current.get_metamethod("__index") {
                None => return Ok(Value::Nil),
                Some(Value::Table(next)) => {
                    let v = next.raw_get(key)?;
                    if !v.is_nil() {
                        return Ok(v);
                    }
                    current = next;
                }
                Some(Value::Function(mm)) => {
                    let results = self
                        .call_function(mm, vec![Value::Table(current), key.clone()])
                        .await
                        .map_err(|re| re.error)?;
                    return Ok(results.into_iter().next().unwrap_or(Value::Nil));
                }
                Some(_) => return Ok(Value::Nil),
            }
        }
        Err(crate::error::VmError::LuaError {
            display: "'__index' chain too long".to_owned(),
            value: Value::string("'__index' chain too long"),
        })
    }

    /// Write a value to a table by key, respecting `__newindex` metamethod.
    ///
    /// If the key is absent and the table has a `__newindex` metamethod, it
    /// is dispatched (table chain or function call).  Mirrors the VM's
    /// `SetTable` instruction semantics.
    pub async fn table_set(
        &self,
        table: &crate::table::Table,
        key: Value,
        value: Value,
    ) -> Result<(), crate::error::VmError> {
        // __newindex only triggers when the key is absent.
        let existing = table.raw_get(&key)?;
        if !existing.is_nil() {
            table.raw_set(key, value)?;
            return Ok(());
        }
        let mut current = table.clone();
        for _ in 0..crate::METAMETHOD_CHAIN_LIMIT {
            match current.get_metamethod("__newindex") {
                None => {
                    current.raw_set(key, value)?;
                    return Ok(());
                }
                Some(Value::Table(next)) => {
                    let existing = next.raw_get(&key)?;
                    if !existing.is_nil() {
                        next.raw_set(key, value)?;
                        return Ok(());
                    }
                    current = next;
                }
                Some(Value::Function(mm)) => {
                    self.call_function(mm, vec![Value::Table(current), key, value])
                        .await
                        .map_err(|re| re.error)?;
                    return Ok(());
                }
                Some(_) => {
                    current.raw_set(key, value)?;
                    return Ok(());
                }
            }
        }
        Err(crate::error::VmError::LuaError {
            display: "'__newindex' chain too long".to_owned(),
            value: Value::string("'__newindex' chain too long"),
        })
    }

    /// Call a Lua or native `Function`, propagating the current call stack
    /// into the nested task so that errors and stack traces reflect the full
    /// chain.
    ///
    /// A `StackFrame::Native` entry for the current function is inserted
    /// between the outer frames and the inner task's frames when
    /// `self.native_name` is set.
    pub async fn call_function(
        &self,
        func: Function,
        args: Vec<Value>,
    ) -> Result<Vec<Value>, crate::error::RuntimeError> {
        use crate::task::Task;
        // Build the parent stack: everything visible so far, plus a Native
        // frame for the current function if it has a name.
        let parent_stack: Arc<Vec<StackFrame>> = if let Some(name) = &self.native_name {
            let mut v: Vec<StackFrame> = (*self.call_stack).clone();
            v.push(StackFrame::Native {
                function_name: name.clone(),
            });
            Arc::new(v)
        } else {
            self.call_stack.clone()
        };
        Task::new_with_parent(self.global.clone(), func, args, parent_stack).await
    }
}
