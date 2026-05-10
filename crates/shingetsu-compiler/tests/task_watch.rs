//! Integration tests for `task.watch`.

mod common;

use common::{run_err_with_env, run_in_env, task_env};
use shingetsu::{valuevec, Value};

#[tokio::test]
async fn anon_get_returns_initial() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(7)
        return w:get()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(7)]);
}

#[tokio::test]
async fn set_then_get_returns_new_value() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(0)
        w:set(42)
        return w:get()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(42)]);
}

#[tokio::test]
async fn get_returns_independent_table_copies() {
    // Mutating one rebuilt copy must not affect another.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ k = 1 })
        local a = w:get()
        local b = w:get()
        a.k = 99
        return a.k, b.k
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(99), Value::Integer(1)]);
}

#[tokio::test]
async fn set_rejects_function_value() {
    let env = task_env();
    k9::assert_equal!(
        run_err_with_env(
            env,
            r#"
        local w = task.watch(0)
        w:set(function() return 1 end)
    "#,
        )
        .await,
        "error: error in 'snapshot': function values cannot be snapshotted \
         (functions capture upvalues bound to a specific environment)
 --> test.lua:3:9
  |
3 |         w:set(function() return 1 end)
  |         ^^^^^ error in 'snapshot': function values cannot be snapshotted \
         (functions capture upvalues bound to a specific environment)
stack traceback:
\ttest.lua:3: in main chunk"
    );
}

#[tokio::test]
async fn wait_change_wakes_on_next_set() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(0)
        local waiter = task.spawn(function()
            return w:wait_change()
        end)
        task.yield()
        w:set(11)
        return waiter:await()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(11)]);
}

#[tokio::test]
async fn wait_change_only_observes_changes_after_call() {
    // wait_change is edge-triggered: it must not fire on changes
    // that happened before the wait began.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(0)
        w:set(1)
        w:set(2)
        local fired = false
        local waiter = task.spawn(function()
            local v = w:wait_change()
            fired = true
            return v
        end)
        task.yield()
        local fired_before = fired
        -- waiter is still parked; only a fresh set wakes it
        w:set(3)
        local v = waiter:await()
        return fired_before, v
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(false), Value::Integer(3)]);
}

#[tokio::test]
async fn wait_for_returns_immediately_when_predicate_initially_true() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(10)
        local checks = 0
        local v = w:wait_for(function(x)
            checks = checks + 1
            return x >= 5
        end)
        return v, checks
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(10), Value::Integer(1)]);
}

#[tokio::test]
async fn wait_for_loops_until_predicate_true() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(0)
        local waiter = task.spawn(function()
            return w:wait_for(function(x) return x >= 3 end)
        end)
        for i = 1, 3 do
            task.yield()
            w:set(i)
        end
        return waiter:await()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(3)]);
}

#[tokio::test]
async fn named_returns_same_watch() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.watch(1, "shared")
        local b = task.watch(1, "shared")
        local c = task.watch(1, "other")
        return rawequal(a, b), rawequal(a, c)
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Boolean(true), Value::Boolean(false)]
    );
}

#[tokio::test]
async fn named_initial_ignored_on_hit() {
    // Existing named watch wins; the new initial is not used.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.watch(7, "shared_init")
        local b = task.watch(99, "shared_init")
        return a:get(), b:get()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(7), Value::Integer(7)]);
}

#[tokio::test]
async fn named_visible_across_tasks() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(0, "cross_task")
        local observer = task.spawn(function()
            local w2 = task.watch(0, "cross_task")
            return w2:wait_change()
        end)
        task.yield()
        w:set(123)
        return observer:await()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(123)]);
}

#[tokio::test]
async fn function_form_initial_called_once_for_named() {
    // Pin down: when a named watch is created with a function-form
    // initial, the function is called exactly once across the
    // lifetime of the named entry, even from concurrent attempts.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local calls = 0
        local function init() calls = calls + 1; return calls end
        local a = task.watch(init, "lazy_init")
        local b = task.watch(init, "lazy_init")
        return a:get(), b:get(), calls
    "#,
    )
    .await
    .expect("run");
    // First call invokes init (calls -> 1, value -> 1).  Second
    // call hits the existing entry; init is NOT invoked.  So the
    // value stored is 1 and `calls` is 1.
    k9::assert_equal!(
        results,
        valuevec![Value::Integer(1), Value::Integer(1), Value::Integer(1)]
    );
}

#[tokio::test]
async fn function_form_initial_invoked_for_anon() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(function() return "computed" end)
        return w:get()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::string("computed")]);
}

#[tokio::test]
async fn function_form_initial_propagates_error() {
    let env = task_env();
    k9::assert_equal!(
        run_err_with_env(
            env,
            r#"
        local w = task.watch(function() error("boom") end)
    "#,
        )
        .await,
        "error: test.lua:2: boom
 --> test.lua:2:19
  |
2 |         local w = task.watch(function() error(\"boom\") end)
  |                   ^^^^^^^^^^ test.lua:2: boom
stack traceback:
\ttest.lua:2: in main chunk"
    );
}

#[tokio::test]
async fn aborted_wait_does_not_block_others() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch(0)
        local doomed = task.spawn(function() w:wait_change() end)
        local survivor = task.spawn(function() return w:wait_change() end)
        task.yield()
        doomed:abort()
        w:set(7)
        return survivor:await()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(7)]);
}
