//! Integration tests for `task.semaphore`.

mod common;

use common::{run_in_env, task_env};
use shingetsu::{valuevec, Value};

#[tokio::test]
async fn anon_acquire_and_release_via_drop() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local s = task.semaphore(2)
        do
            local p = s:acquire()
        end
        return s:available()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(2)]);
}

#[tokio::test]
async fn permits_and_available_track_acquisitions() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local s = task.semaphore(3)
        local p1 = s:acquire()
        local p2 = s:acquire()
        return s:permits(), s:available()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(3), Value::Integer(1)]);
}

#[tokio::test]
async fn try_acquire_returns_nil_at_capacity() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local s = task.semaphore(1)
        local p = s:acquire()
        local p2 = s:try_acquire()
        return p2
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

#[tokio::test]
async fn explicit_release_returns_permit() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local s = task.semaphore(1)
        local p = s:acquire()
        p:release()
        local p2 = s:try_acquire()
        return p2 ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn double_release_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local s = task.semaphore(1)
        local p = s:acquire()
        p:release()
        p:release()
    "#,
        "error: semaphore permit has already been released
 --> test.lua:5:9
  |
5 |         p:release()
  |         ^^^^^^^^^ semaphore permit has already been released
stack traceback:
\ttest.lua:5: in main chunk",
    );
}

#[tokio::test]
async fn negative_permits_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local s = task.semaphore(-1)
    "#,
        "error: bad argument #1 to 'semaphore' (permits must be non-negative, got -1)
 --> test.lua:2:34
  |
2 |         local s = task.semaphore(-1)
  |                                  ^^ bad argument #1 to 'semaphore' (permits must be non-negative, got -1)
stack traceback:
\ttest.lua:2: in main chunk",
    );
}

#[tokio::test]
async fn named_returns_same_semaphore() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.semaphore(3, "a")
        local b = task.semaphore(3, "a")
        local c = task.semaphore(3, "b")
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
async fn named_permit_grow_adds_permits() {
    // Reload-friendly: a later call requesting more permits than the
    // current configuration silently grows the existing semaphore.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.semaphore(3, "shared")
        local b = task.semaphore(5, "shared")
        return a:permits(), b:permits(), a:available(), b:available()
    "#,
    )
    .await
    .expect("run");
    // Same Arc, so a and b report identical counts.  Configured
    // permits grew from 3 to 5; available also grew to 5 since no
    // permits are held.
    k9::assert_equal!(
        results,
        valuevec![
            Value::Integer(5),
            Value::Integer(5),
            Value::Integer(5),
            Value::Integer(5)
        ]
    );
}

#[tokio::test]
async fn named_permit_shrink_keeps_existing() {
    // Reload-friendly: a later call requesting fewer permits than
    // the current configuration logs a warning and keeps the
    // existing semaphore unchanged.  tokio's Semaphore cannot shrink.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.semaphore(5, "shared")
        local b = task.semaphore(3, "shared")
        return a:permits(), b:permits()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(5), Value::Integer(5)]);
}

#[tokio::test]
async fn named_visible_across_tasks() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local s = task.semaphore(1, "shared")
        local p = s:acquire()
        local t = task.spawn(function()
            local s2 = task.semaphore(1, "shared")
            local p2 = s2:try_acquire()
            return p2 == nil
        end)
        local sees_held = t:await()
        p:release()
        return sees_held
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn acquire_awaits_until_released() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local s = task.semaphore(1)
        local order = {}
        local p = s:acquire()
        local t = task.spawn(function()
            local p2 = s:acquire()
            table.insert(order, "task")
        end)
        task.yield()
        table.insert(order, "main")
        p:release()
        t:await()
        return order[1], order[2]
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::string("main"), Value::string("task")]
    );
}

#[tokio::test]
async fn aborted_acquire_does_not_block_others() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local s = task.semaphore(1)
        local p = s:acquire()
        local blocked = task.spawn(function()
            local p2 = s:acquire()
        end)
        task.yield()
        blocked:abort()
        p:release()
        local p3 = s:try_acquire()
        return p3 ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}
