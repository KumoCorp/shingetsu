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
        /// info (Phase 3 / debug-build flag).
        locals: Vec<(Bytes, Value)>,
        /// Whether the most recent `Call` instruction from this frame used
        /// `:` syntax.  Used by `detect_hints` to suggest `.` vs `:` corrections.
        last_call_is_method: bool,
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
