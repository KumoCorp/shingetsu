//! Integration tests for `task.notify` (Phase F of notes/SYNC.md).

mod common;

use common::{run_err_with_env, run_in_env, task_env};
use shingetsu::{valuevec, Value};

#[tokio::test]
async fn notify_one_wakes_a_single_waiter() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local n = task.notify()
        local order = {}
        local w1 = task.spawn(function()
            n:wait_notified()
            table.insert(order, "w1")
        end)
        local w2 = task.spawn(function()
            n:wait_notified()
            table.insert(order, "w2")
        end)
        task.yield()
        n:notify_one()
        task.yield()
        local count_after_one = #order
        n:notify_one()
        task.await_all({w1, w2})
        return count_after_one, #order
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(1), Value::Integer(2)]);
}

#[tokio::test]
async fn notify_last_wakes_the_most_recent_waiter() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local n = task.notify()
        local order = {}
        local w1 = task.spawn(function()
            n:wait_notified()
            table.insert(order, "w1")
        end)
        task.yield()
        local w2 = task.spawn(function()
            n:wait_notified()
            table.insert(order, "w2")
        end)
        task.yield()
        n:notify_last()
        task.yield()
        local first = order[1]
        n:notify_one()
        task.await_all({w1, w2})
        return first, order[2]
    "#,
    )
    .await
    .expect("run");
    // notify_last wakes w2 (most recent); the subsequent notify_one
    // wakes w1.
    k9::assert_equal!(results, valuevec![Value::string("w2"), Value::string("w1")]);
}

#[tokio::test]
async fn notify_all_wakes_every_waiter() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local n = task.notify()
        local woken = 0
        local tasks = {}
        for i = 1, 4 do
            tasks[i] = task.spawn(function()
                n:wait_notified()
                woken = woken + 1
            end)
        end
        task.yield()
        n:notify_all()
        task.await_all(tasks)
        return woken
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(4)]);
}

#[tokio::test]
async fn wait_until_returns_immediately_when_predicate_true() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local n = task.notify()
        local checks = 0
        n:wait_until(function()
            checks = checks + 1
            return true
        end)
        return checks
    "#,
    )
    .await
    .expect("run");
    // Predicate should be evaluated exactly once.
    k9::assert_equal!(results, valuevec![Value::Integer(1)]);
}

#[tokio::test]
async fn wait_until_loops_until_predicate_true() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local n = task.notify()
        local counter = 0
        local checks = 0
        local waiter = task.spawn(function()
            n:wait_until(function()
                checks = checks + 1
                return counter >= 3
            end)
        end)
        for i = 1, 3 do
            task.yield()
            counter = counter + 1
            n:notify_one()
        end
        waiter:await()
        return counter, checks
    "#,
    )
    .await
    .expect("run");
    // Predicate runs once initially (false at 0), then once per
    // notify (4 checks total: initial + 3 notifies, last one true).
    k9::assert_equal!(results, valuevec![Value::Integer(3), Value::Integer(4)]);
}

#[tokio::test]
async fn wait_until_does_not_lose_wakeup_via_register_before_check() {
    // Contract: a notification raced against the predicate evaluation
    // must not be lost.  The implementation registers interest (via
    // tokio's `Notified::enable`) BEFORE evaluating the predicate, so
    // a notify that arrives during the check still satisfies the
    // subsequent await.  Hard to trigger the race deterministically
    // from the cooperative scheduler, so we instead verify the steady
    // contract: the waiter eventually completes when the condition
    // becomes true and a notify is sent.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local n = task.notify()
        local ready = false
        local waiter = task.spawn(function()
            n:wait_until(function() return ready end)
            return "done"
        end)
        task.yield()
        ready = true
        n:notify_one()
        return waiter:await()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::string("done")]);
}

#[tokio::test]
async fn wait_until_propagates_predicate_errors() {
    let env = task_env();
    k9::assert_equal!(
        run_err_with_env(
            env,
            r#"
        local n = task.notify()
        n:wait_until(function() error("predicate failed") end)
    "#,
        )
        .await,
        "error: test.lua:3: predicate failed
 --> test.lua:3:9
  |
3 |         n:wait_until(function() error(\"predicate failed\") end)
  |         ^^^^^^^^^^^^ test.lua:3: predicate failed
stack traceback:
\ttest.lua:3: in main chunk"
    );
}

#[tokio::test]
async fn named_returns_same_notify() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.notify("a")
        local b = task.notify("a")
        local c = task.notify("b")
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
async fn named_visible_across_tasks() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local n = task.notify("shared")
        local woken = false
        local waiter = task.spawn(function()
            local n2 = task.notify("shared")
            n2:wait_notified()
            woken = true
        end)
        task.yield()
        n:notify_one()
        waiter:await()
        return woken
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn aborted_wait_does_not_block_others() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local n = task.notify()
        local woken = false
        local doomed = task.spawn(function() n:wait_notified() end)
        local survivor = task.spawn(function()
            n:wait_notified()
            woken = true
        end)
        task.yield()
        doomed:abort()
        n:notify_one()
        survivor:await()
        return woken
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}
