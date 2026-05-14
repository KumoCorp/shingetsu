//! Integration tests for `task.oneshot`.

mod common;

use common::{run_in_env, task_env};
use shingetsu::{valuevec, Value};

#[tokio::test]
async fn send_then_recv_round_trip() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local tx, rx = task.oneshot()
        task.spawn(function() tx:send(42) end)
        return rx:recv()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(42)]);
}

#[tokio::test]
async fn close_wakes_receiver_with_nil() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local tx, rx = task.oneshot()
        task.spawn(function() tx:close() end)
        return rx:recv()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

#[tokio::test]
async fn double_send_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local tx, rx = task.oneshot()
        tx:send(1)
        tx:send(2)
    "#,
        "error: oneshot sender has already been consumed
 --> test.lua:4:9
  |
4 |         tx:send(2)
  |         ^^^^^^^ oneshot sender has already been consumed
stack traceback:
\ttest.lua:4: in main chunk",
    );
}

#[tokio::test]
async fn send_after_close_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local tx, rx = task.oneshot()
        tx:close()
        tx:send(1)
    "#,
        "error: oneshot sender has already been consumed
 --> test.lua:4:9
  |
4 |         tx:send(1)
  |         ^^^^^^^ oneshot sender has already been consumed
stack traceback:
\ttest.lua:4: in main chunk",
    );
}

#[tokio::test]
async fn close_is_idempotent() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local tx, rx = task.oneshot()
        tx:close()
        tx:close()
        return rx:recv()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

#[tokio::test]
async fn double_recv_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local tx, rx = task.oneshot()
        tx:send(1)
        rx:recv()
        rx:recv()
    "#,
        "error: oneshot receiver has already been consumed
 --> test.lua:5:9
  |
5 |         rx:recv()
  |         ^^^^^^^ oneshot receiver has already been consumed
stack traceback:
\ttest.lua:5: in main chunk",
    );
}

#[tokio::test]
async fn passes_table_value_by_alias() {
    // Anonymous oneshot uses Value (not SnapshotValue), so the
    // receiver gets the same Lua table the sender sent \u2014 this is
    // safe because anon oneshot cannot leave the GlobalEnv.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local tx, rx = task.oneshot()
        local sent = { count = 0 }
        task.spawn(function() tx:send(sent) end)
        local received = rx:recv()
        received.count = 99
        -- Same Arc-backed table, so the producer-side reference sees the change.
        return rawequal(sent, received), sent.count, received.count
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Boolean(true), Value::Integer(99), Value::Integer(99)]
    );
}
