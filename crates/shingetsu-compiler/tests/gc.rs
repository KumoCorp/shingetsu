mod common;

use common::{new_env, run_one};
use shingetsu_compiler::{compile, CompileOptions};
use shingetsu_vm::{Function, Task, Value};

// ---------------------------------------------------------------------------
// GC: collectgarbage + __gc metamethod
// ---------------------------------------------------------------------------

// A minimal Userdata used to observe when a Value is dropped by the GC.
// Holds a clone of a shared Arc; when the Userdata is dropped (because the
// GC cleared the table that contained it) the Arc's strong_count falls.
struct MarkerUserdata;

#[async_trait::async_trait]
impl shingetsu_vm::Userdata for MarkerUserdata {
    fn type_name(&self) -> &'static str {
        "MarkerUserdata"
    }
}

#[test]
fn gc_collect_unreachable_no_finalizer() {
    // An unreachable table with no __gc must have its contents cleared by the
    // GC sweep.  We verify the sweep actually ran — not just that no error
    // occurred — by storing a Userdata in the table and checking that the
    // shared Arc's strong_count drops back to 1 once the table is collected.
    use shingetsu_vm::{Task, Value};
    use std::sync::Arc;

    let env = new_env();
    let marker = Arc::new(MarkerUserdata) as Arc<dyn shingetsu_vm::Userdata + Send + Sync>;
    // Register the marker as a global so the Lua script can read it.
    env.set_global("_marker", Value::Userdata(marker.clone()));
    // Arc refs: test `marker` (1) + env global (1) = 2.

    let src = r#"
local t = { ud = _marker }  -- table holds a ref to the marker
_marker = nil               -- remove global ref; only table holds it now
t = nil                     -- drop the table ref (table becomes unreachable)
collectgarbage("collect")   -- sweep must clear the table contents
return 1
"#;
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env.clone(), func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed");

    // After collection the table contents were cleared, dropping the
    // Value::Userdata inside.  Only our `marker` handle remains.
    k9::assert_equal!(Arc::strong_count(&marker), 1);
}

#[test]
fn gc_gc_metamethod_called() {
    // A table with __gc should have its finalizer called during collect.
    k9::assert_equal!(
        run_one(
            r#"
local finalized = 0
local t = setmetatable({}, {
    __gc = function(self)
        finalized = finalized + 1
    end
})
t = nil
collectgarbage("collect")
return finalized
"#
        ),
        Value::Integer(1)
    );
}

#[test]
fn gc_gc_metamethod_receives_table() {
    // The finalizer receives the table as its argument.
    k9::assert_equal!(
        run_one(
            r#"
local got_type = ""
local t = setmetatable({value = 42}, {
    __gc = function(self)
        got_type = type(self)
    end
})
t = nil
collectgarbage("collect")
return got_type
"#
        ),
        Value::string("table")
    );
}

#[test]
fn gc_reachable_table_not_collected() {
    // A table that is still reachable must NOT be collected.
    k9::assert_equal!(
        run_one(
            r#"
local finalized = 0
local t = setmetatable({}, {
    __gc = function(self)
        finalized = finalized + 1
    end
})
collectgarbage("collect")   -- t is still live
return finalized
"#
        ),
        Value::Integer(0)
    );
}

#[test]
fn gc_dispose_runs_gc_finalizers() {
    // dispose() must finalize every tracked table that has a __gc metamethod,
    // even if collectgarbage() was never called explicitly.
    //
    // We can't read a Lua global after dispose() because dispose() clears
    // globals before collecting.  Instead we register a native that closes
    // over a Rust-side AtomicBool; the __gc handler calls that native, and
    // we inspect the flag after dispose() returns.
    use shingetsu_vm::types::FunctionSignature;
    use shingetsu_vm::{NativeFunction, Task, Value, VmError};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let finalized = Arc::new(AtomicBool::new(false));
    let env = new_env();

    // Register a native that flips the Rust-side flag when called.
    {
        let flag = finalized.clone();
        env.register_native(NativeFunction {
            signature: Arc::new(FunctionSignature {
                name: bytes::Bytes::from_static(b"mark_gc_ran"),
                source: bytes::Bytes::new(),
                type_params: vec![],
                params: vec![],
                variadic: true,
                arg_offset: 0,
                returns: None,
                lua_returns: None,
            }),
            call: Arc::new(move |_, _| {
                flag.store(true, Ordering::SeqCst);
                Box::pin(async { Ok::<Vec<Value>, VmError>(vec![]) })
            }),
        });
    }

    // The __gc handler calls mark_gc_ran(); no explicit collectgarbage().
    let src = r#"
local t = setmetatable({}, {
    __gc = function(self) mark_gc_ran() end
})
t = nil
"#;
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env.clone(), func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task).expect("task failed");

    // __gc has not fired yet — no collect was called.
    k9::assert_equal!(finalized.load(Ordering::SeqCst), false);

    // dispose() clears globals, collects, and runs pending __gc finalizers.
    rt.block_on(env.dispose());

    // The __gc handler must have called mark_gc_ran().
    k9::assert_equal!(finalized.load(Ordering::SeqCst), true);
}

// ---------------------------------------------------------------------------
// Task::dispose()
// ---------------------------------------------------------------------------

#[test]
fn task_dispose_calls_close_on_cancel() {
    use shingetsu_vm::types::FunctionSignature;
    use shingetsu_vm::{NativeFunction, Task, Value, VmError};
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake};

    struct NoopWaker;
    impl Wake for NoopWaker {
        fn wake(self: Arc<Self>) {}
    }

    // Register a native that blocks forever (simulates I/O or sleep).
    let env = new_env();
    env.register_native(NativeFunction {
        signature: Arc::new(FunctionSignature {
            name: bytes::Bytes::from_static(b"block_forever"),
            source: bytes::Bytes::new(),
            type_params: vec![],
            params: vec![],
            variadic: true,
            arg_offset: 0,
            returns: None,
            lua_returns: None,
        }),
        call: Arc::new(|_, _| {
            Box::pin(async {
                // Never resolves.
                std::future::pending::<Result<Vec<Value>, VmError>>().await
            })
        }),
    });

    // Script: initialise a <close> variable, then block.
    // The __close handler increments the global `closed`.
    let src = r#"
closed = 0
local x <close> = setmetatable({}, {
    __close = function(self, err)
        closed = closed + 1
    end
})
block_forever()
"#;
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let mut task = Task::new(env.clone(), func, vec![]);

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async move {
        // Poll the task once with a noop waker to run it up to (and into)
        // the blocking native call.  The task must come back as Pending.
        {
            let waker = std::task::Waker::from(Arc::new(NoopWaker));
            let mut cx = Context::from_waker(&waker);
            // Task: Unpin (BoxFuture is Unpin), so Pin::new is safe.
            let result = std::pin::Pin::new(&mut task).poll(&mut cx);
            assert!(
                matches!(result, Poll::Pending),
                "expected task to be pending while blocking native is active"
            );
        }

        // Simulate cancellation: dispose() must call the __close handler.
        task.dispose().await;

        // The __close handler should have fired and set closed = 1.
        k9::assert_equal!(
            env.get_global("closed").unwrap_or(Value::Nil),
            Value::Integer(1)
        );
    });
}

#[test]
fn task_dispose_no_close_vars_is_noop() {
    // dispose() on a task with no <close> variables should complete cleanly.
    let env = new_env();
    let src = r#"
x = 42
"#;
    let opts = CompileOptions::default();
    let bc = compile(src, &opts).expect("compile failed");
    let func = Function::lua(bc.top_level, vec![]);
    let task = Task::new(env.clone(), func, vec![]);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(task.dispose()); // must not hang or panic
}

// ---------------------------------------------------------------------------
