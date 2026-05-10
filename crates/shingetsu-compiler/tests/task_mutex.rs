//! Integration tests for `task.mutex`.

mod common;

use common::{run_err_with_env, run_in_env, task_env};
use shingetsu::{valuevec, Value};

#[tokio::test]
async fn anon_lock_and_unlock() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local m = task.mutex()
        do
            local g = m:lock()
            -- guard released at end of block via Drop
        end
        -- can re-acquire after release
        do
            local g = m:lock()
        end
        return "ok"
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::string("ok")]);
}

#[tokio::test]
async fn try_lock_returns_nil_when_held() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local m = task.mutex()
        local g = m:lock()
        local g2 = m:try_lock()
        return g2
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

#[tokio::test]
async fn try_lock_succeeds_when_free() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local m = task.mutex()
        local g = m:try_lock()
        return g ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn explicit_unlock_releases_lock() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local m = task.mutex()
        local g = m:lock()
        g:unlock()
        local g2 = m:try_lock()
        return g2 ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn double_unlock_is_an_error() {
    let env = task_env();
    k9::assert_equal!(
        run_err_with_env(
            env,
            r#"
        local m = task.mutex()
        local g = m:lock()
        g:unlock()
        g:unlock()
    "#,
        )
        .await,
        "error: mutex guard has already been released
 --> test.lua:5:9
  |
5 |         g:unlock()
  |         ^^^^^^^^ mutex guard has already been released
stack traceback:
	test.lua:5: in main chunk"
    );
}

#[tokio::test]
async fn named_returns_same_mutex() {
    let env = task_env();
    // Use a unique name to avoid colliding with the process-global registry
    // entries from other tests running in parallel.
    let results = run_in_env(
        &env,
        r#"
        local a = task.mutex("a")
        local b = task.mutex("a")
        local c = task.mutex("b")
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
async fn named_lock_visible_across_tasks() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local m = task.mutex("shared")
        local g = m:lock()
        -- spawn a task that tries to lock the same named mutex
        local t = task.spawn(function()
            local m2 = task.mutex("shared")
            local g2 = m2:try_lock()
            return g2 == nil
        end)
        local sees_held = t:await()
        g:unlock()
        return sees_held
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn aborted_lock_await_does_not_block_others() {
    // Cancellation safety: a task awaiting `:lock()` whose future is
    // dropped (via `:abort()`) must release its slot in the wait queue
    // so other waiters can acquire when the lock is released.  tokio's
    // `lock_owned()` future provides this; this test pins the contract.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local m = task.mutex()
        local g = m:lock()
        -- This task will block waiting for the lock.
        local blocked = task.spawn(function()
            local g2 = m:lock()
        end)
        task.yield()  -- let the task reach its lock-await
        blocked:abort()
        -- After abort, releasing the lock should let a fresh acquire succeed.
        g:unlock()
        local g3 = m:try_lock()
        return g3 ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn lock_awaits_until_released() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local m = task.mutex()
        local order = {}
        local g = m:lock()
        local t = task.spawn(function()
            -- this :lock() must await until the outer code unlocks
            local g2 = m:lock()
            table.insert(order, "task")
        end)
        task.yield()
        table.insert(order, "main")
        g:unlock()
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
