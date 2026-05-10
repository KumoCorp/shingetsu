//! Verify that values held in non-`<close>` locals are dropped at scope
//! exit (Phase B1 of notes/SYNC.md).
//!
//! Each test installs a `DropTracker` userdata whose `Drop` impl
//! increments a shared counter, plus a `get_drop_count()` native that
//! reads it.  The Lua scripts then assert about when the counter
//! advances.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::run_with_env;
use shingetsu_vm::{valuevec, Function, GlobalEnv, Userdata, Value, VmError};

struct DropTracker {
    counter: Arc<AtomicUsize>,
}

impl Userdata for DropTracker {
    fn type_name(&self) -> &'static str {
        "DropTracker"
    }
}

impl Drop for DropTracker {
    fn drop(&mut self) {
        self.counter.fetch_add(1, Ordering::SeqCst);
    }
}

/// Build an env with `make_tracker()` and `get_drop_count()` registered
/// against a freshly-allocated counter.
fn env_with_tracker() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("register builtins");
    let counter = Arc::new(AtomicUsize::new(0));
    let make_counter = counter.clone();
    env.register_function(Function::wrap(
        "make_tracker",
        move || -> Result<Value, VmError> {
            Ok(Value::userdata(Arc::new(DropTracker {
                counter: make_counter.clone(),
            })))
        },
    ));
    let read_counter = counter.clone();
    env.register_function(Function::wrap(
        "get_drop_count",
        move || -> Result<i64, VmError> { Ok(read_counter.load(Ordering::SeqCst) as i64) },
    ));
    env
}

#[tokio::test]
async fn drop_fires_at_end_of_do_end_block() {
    let env = env_with_tracker();
    let results = run_with_env(
        env,
        r#"
        local function f()
            local before, after
            do
                local _t = make_tracker()
                before = get_drop_count()
            end
            after = get_drop_count()
            return before, after
        end
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(0), Value::Integer(1)]);
}

#[tokio::test]
async fn for_body_locals_drop_via_reassignment() {
    // For-body scopes intentionally do not emit per-iteration slot
    // clears (per-iteration `LoadNil` would have measurable cost with
    // no observable benefit, since the next iteration's writes drop
    // the previous values anyway).  This test pins down the actual
    // behaviour: the previous iteration's tracker is dropped when the
    // current iteration reassigns the slot, and the final iteration's
    // tracker lingers in the slot until an enclosing scope ends.
    let env = env_with_tracker();
    let results = run_with_env(
        env,
        r#"
        local function f()
            local mid_iter
            for i = 1, 3 do
                local _t = make_tracker()
                if i == 2 then
                    -- iter 1's tracker was dropped when iter 2's `local _t`
                    -- reassigned the slot, so count is 1 here.
                    mid_iter = get_drop_count()
                end
            end
            -- iter 3's tracker is still alive in the for-body slot; only
            -- iter 1 and iter 2 have been dropped via reassignment.
            return mid_iter, get_drop_count()
        end
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(1), Value::Integer(2)]);
}

#[tokio::test]
async fn drop_fires_when_breaking_out_of_loop() {
    let env = env_with_tracker();
    let results = run_with_env(
        env,
        r#"
        local function f()
            while true do
                local _t = make_tracker()
                break
            end
            return get_drop_count()
        end
        return f()
    "#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(1)]);
}

#[tokio::test]
async fn drop_does_not_fire_while_value_held_by_upvalue() {
    let env = env_with_tracker();
    let results = run_with_env(
        env,
        r#"
        local function f()
            local saved
            do
                local t = make_tracker()
                saved = function() return t end
            end
            -- t's slot is cleared, but the closure captured it via an
            -- upvalue, so the value is still alive.
            return get_drop_count(), saved
        end
        local count_after_block, _fn = f()
        return count_after_block
    "#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(0)]);
}

#[tokio::test]
async fn drop_fires_at_function_return_for_top_level_locals() {
    // B2: a function-body local that is not in any nested scope is
    // released when the function returns, via the recycle-time slot
    // clear in `recycle_registers`.  Without B2 the value would
    // linger in the pooled register box until a future frame
    // acquired it.
    let env = env_with_tracker();
    let results = run_with_env(
        env,
        r#"
        local function inner()
            local _t = make_tracker()
            return get_drop_count()
        end
        local count_inside = inner()
        -- After inner() returns, _t was a top-level local of inner's
        -- frame.  B1 does not emit a clear for it (function-body root
        -- scope).  B2 clears the slot when the frame's register box
        -- is returned to the recycle pool.
        return count_inside, get_drop_count()
    "#,
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(0), Value::Integer(1)]);
}

#[tokio::test]
async fn drop_does_not_fire_when_value_returned() {
    let env = env_with_tracker();
    let results = run_with_env(
        env,
        r#"
        local function f()
            local t = make_tracker()
            return t, get_drop_count()
        end
        local _t, count_at_return = f()
        return count_at_return, get_drop_count()
    "#,
    )
    .await;
    // Inside f(), before its locals get cleared, count is 0.  After
    // f() returns, _t holds the tracker, so count is still 0.
    k9::assert_equal!(results, valuevec![Value::Integer(0), Value::Integer(0)]);
}
