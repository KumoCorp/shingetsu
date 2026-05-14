//! Integration tests for `task.bounded_channel` and
//! `task.unbounded_channel`.

mod common;

use common::{run_in_env, task_env};
use shingetsu::{valuevec, Value};

// ---------------------------------------------------------------------------
// Bounded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bounded_send_then_recv_round_trip() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(4)
        ch:send(7)
        return ch:recv()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(7)]);
}

#[tokio::test]
async fn bounded_send_takes_a_snapshot_at_send_time() {
    // The producer's table is captured when :send is called, so
    // post-send mutations to the producer's table are not visible to
    // the consumer.  Verifies the cross-VM isolation contract from
    // the producer side.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(4)
        local sent = { k = 1 }
        ch:send(sent)
        sent.k = 99               -- mutated after send
        local received = ch:recv()
        return received.k         -- still 1: snapshot captured at send
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(1)]);
}

#[tokio::test]
async fn bounded_try_send_full_returns_false() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(2)
        local a = ch:try_send(1)
        local b = ch:try_send(2)
        local c = ch:try_send(3)
        return a, b, c
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::Boolean(true),
            Value::Boolean(true),
            Value::Boolean(false),
        ]
    );
}

#[tokio::test]
async fn bounded_try_recv_empty_returns_nil() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(2)
        return ch:try_recv()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

#[tokio::test]
async fn bounded_send_awaits_until_recv_drains() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(1)
        ch:send(1)
        local order = {}
        local sender = task.spawn(function()
            ch:send(2)
            table.insert(order, "send-completed")
        end)
        task.yield()
        table.insert(order, "main")
        local first = ch:recv()
        sender:await()
        local second = ch:recv()
        return first, second, order[1], order[2]
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::Integer(1),
            Value::Integer(2),
            Value::string("main"),
            Value::string("send-completed"),
        ]
    );
}

#[tokio::test]
async fn bounded_close_drains_then_returns_nil() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(4)
        ch:send(1)
        ch:send(2)
        ch:close()
        local a = ch:recv()
        local b = ch:recv()
        local c = ch:recv()
        return a, b, c
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Integer(1), Value::Integer(2), Value::Nil]
    );
}

#[tokio::test]
async fn bounded_send_after_close_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local ch = task.bounded_channel(4)
        ch:close()
        ch:send(1)
    "#,
        "error: channel is closed
 --> test.lua:4:9
  |
4 |         ch:send(1)
  |         ^^^^^^^ channel is closed
stack traceback:
\ttest.lua:4: in main chunk",
    );
}

#[tokio::test]
async fn bounded_send_rejects_function_value() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local ch = task.bounded_channel(4)
        ch:send(function() return 1 end)
    "#,
        "error: error in 'snapshot': function values cannot be snapshotted \
         (functions capture upvalues bound to a specific environment)
 --> test.lua:3:9
  |
3 |         ch:send(function() return 1 end)
  |         ^^^^^^^ error in 'snapshot': function values cannot be snapshotted \
         (functions capture upvalues bound to a specific environment)
stack traceback:
\ttest.lua:3: in main chunk",
    );
}

#[tokio::test]
async fn bounded_zero_capacity_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local ch = task.bounded_channel(0)
    "#,
        "error: bad argument #1 to 'bounded_channel' (capacity must be positive, got 0)
 --> test.lua:2:41
  |
2 |         local ch = task.bounded_channel(0)
  |                                         ^ bad argument #1 to 'bounded_channel' (capacity must be positive, got 0)
stack traceback:
\ttest.lua:2: in main chunk",
    );
}

#[tokio::test]
async fn bounded_named_returns_same_channel() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.bounded_channel(4, "shared")
        local b = task.bounded_channel(4, "shared")
        local c = task.bounded_channel(4, "other")
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
async fn bounded_named_capacity_mismatch_keeps_existing() {
    // Reload-friendly: capacity is fixed at construction; a later
    // call with a different capacity keeps the existing channel.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.bounded_channel(4, "shared_cap")
        local b = task.bounded_channel(8, "shared_cap")
        return a:capacity(), b:capacity()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(4), Value::Integer(4)]);
}

#[tokio::test]
async fn bounded_named_visible_across_tasks() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(4, "cross")
        local sender = task.spawn(function()
            local ch2 = task.bounded_channel(4, "cross")
            ch2:send("from-task")
        end)
        sender:await()
        return ch:recv()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::string("from-task")]);
}

#[tokio::test]
async fn bounded_is_closed_reflects_state() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(4)
        local before = ch:is_closed()
        ch:close()
        local after = ch:is_closed()
        return before, after
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Boolean(false), Value::Boolean(true)]
    );
}

// ---------------------------------------------------------------------------
// Unbounded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unbounded_send_then_recv_round_trip() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.unbounded_channel()
        ch:send(7)
        return ch:recv()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(7)]);
}

#[tokio::test]
async fn unbounded_send_does_not_await() {
    // Unbounded send accepts arbitrarily many values without blocking.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.unbounded_channel()
        for i = 1, 100 do ch:send(i) end
        local sum = 0
        for i = 1, 100 do sum = sum + ch:recv() end
        return sum
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(5050)]);
}

#[tokio::test]
async fn unbounded_close_drains_then_returns_nil() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.unbounded_channel()
        ch:send(1)
        ch:send(2)
        ch:close()
        local a = ch:recv()
        local b = ch:recv()
        local c = ch:recv()
        return a, b, c
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Integer(1), Value::Integer(2), Value::Nil]
    );
}

#[tokio::test]
async fn unbounded_send_after_close_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local ch = task.unbounded_channel()
        ch:close()
        ch:send(1)
    "#,
        "error: channel is closed
 --> test.lua:4:9
  |
4 |         ch:send(1)
  |         ^^^^^^^ channel is closed
stack traceback:
\ttest.lua:4: in main chunk",
    );
}

#[tokio::test]
async fn unbounded_named_returns_same_channel() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.unbounded_channel("shared_unbounded")
        local b = task.unbounded_channel("shared_unbounded")
        return rawequal(a, b)
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn aborted_recv_does_not_block_others() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(4)
        local doomed = task.spawn(function() ch:recv() end)
        local survivor = task.spawn(function() return ch:recv() end)
        task.yield()
        doomed:abort()
        ch:send(42)
        return survivor:await()
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(42)]);
}
