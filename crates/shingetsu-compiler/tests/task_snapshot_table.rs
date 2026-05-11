//! Integration tests for the lazy snapshot table proxy returned by
//! `task.watch:get()`, `task.channel:recv()`, and friends, plus the
//! `task.materialize` free function.

mod common;

use common::{run_err_with_env, run_in_env, task_env};
use shingetsu::{valuevec, Value};

#[tokio::test]
async fn watch_get_returns_snapshot_map_for_table_payload() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ a = 1, b = 2 })
        local snap = w:get()
        return typeof(snap), snap.a, snap.b
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("snapshot_map"),
            Value::Integer(1),
            Value::Integer(2),
        ]
    );
}

#[tokio::test]
async fn watch_get_returns_snapshot_vec_for_array_payload() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ "a", "b", "c" })
        local snap = w:get()
        return typeof(snap), snap[1], snap[2], snap[3], #snap
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("snapshot_vec"),
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
            Value::Integer(3),
        ]
    );
}

#[tokio::test]
async fn writing_to_snapshot_map_raises() {
    let env = task_env();
    k9::assert_equal!(
        run_err_with_env(
            env,
            r#"
        local w = task.watch({ a = 1 })
        local snap = w:get()
        snap.a = 99
    "#,
        )
        .await,
        "error: attempt to modify a snapshot table
 --> test.lua:4:9
  |
4 |         snap.a = 99
  |         ^^^^^^ attempt to modify a snapshot table
help: snapshot tables are read-only; pass through `task.materialize(...)` to obtain a mutable copy
stack traceback:
\ttest.lua:4: in main chunk"
    );
}

#[tokio::test]
async fn writing_to_snapshot_vec_raises() {
    let env = task_env();
    k9::assert_equal!(
        run_err_with_env(
            env,
            r#"
        local w = task.watch({ "a", "b" })
        local snap = w:get()
        snap[1] = "x"
    "#,
        )
        .await,
        "error: attempt to modify a snapshot table
 --> test.lua:4:9
  |
4 |         snap[1] = \"x\"
  |         ^^^^^^ attempt to modify a snapshot table
help: snapshot tables are read-only; pass through `task.materialize(...)` to obtain a mutable copy
stack traceback:
\ttest.lua:4: in main chunk"
    );
}

#[tokio::test]
async fn nested_access_returns_inner_proxy() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ inner = { deep = { val = 42 } } })
        local snap = w:get()
        local inner = snap.inner
        local deep = inner.deep
        return typeof(inner), typeof(deep), deep.val
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("snapshot_map"),
            Value::string("snapshot_map"),
            Value::Integer(42),
        ]
    );
}

#[tokio::test]
async fn materialize_proxy_produces_mutable_table() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ a = 1 })
        local snap = w:get()
        local copy = task.materialize(snap)
        copy.a = 99
        copy.b = "new"
        return typeof(copy), copy.a, copy.b
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("table"),
            Value::Integer(99),
            Value::string("new"),
        ]
    );
}

#[tokio::test]
async fn materialize_vec_proxy_produces_mutable_table() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ 10, 20, 30 })
        local snap = w:get()
        local copy = task.materialize(snap)
        copy[1] = 100
        return typeof(copy), copy[1], copy[2], copy[3]
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("table"),
            Value::Integer(100),
            Value::Integer(20),
            Value::Integer(30),
        ]
    );
}

#[tokio::test]
async fn materialize_passes_through_plain_table() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local t = { foo = "bar" }
        local result = task.materialize(t)
        return rawequal(t, result), result.foo
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Boolean(true), Value::string("bar")]
    );
}

#[tokio::test]
async fn materialize_passes_through_primitives() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        return task.materialize(42), task.materialize("hi"), task.materialize(nil)
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Integer(42), Value::string("hi"), Value::Nil]
    );
}

#[tokio::test]
async fn deep_materialize_only_walks_requested_subtree() {
    // Pulling out a deep value via the proxy and materializing it
    // produces a Lua value equal to the original, without forcing
    // materialization of peer subtrees.  This test pins the
    // behaviour at the result level; the perf claim is in the spec.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({
            a = { x = 1 },
            b = { y = { deep = 42 } },
            c = { z = 3 },
        })
        local snap = w:get()
        local just_deep = task.materialize(snap.b.y)
        return typeof(just_deep), just_deep.deep
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::string("table"), Value::Integer(42)]
    );
}

#[tokio::test]
async fn snapshot_map_len_reports_entry_count() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ a = 1, b = 2, c = 3 })
        return #(w:get())
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(3)]);
}

#[tokio::test]
async fn snapshot_vec_len_reports_element_count() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ "a", "b", "c", "d", "e" })
        return #(w:get())
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(5)]);
}

#[tokio::test]
async fn snapshot_vec_out_of_range_returns_nil() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ 10, 20 })
        local snap = w:get()
        return snap[0], snap[1], snap[2], snap[3]
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::Nil,
            Value::Integer(10),
            Value::Integer(20),
            Value::Nil
        ]
    );
}

#[tokio::test]
async fn pairs_iterates_snapshot_map() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ a = 1, b = 2, c = 3 })
        local snap = w:get()
        local seen = {}
        for k, v in pairs(snap) do
            seen[k] = v
        end
        return seen.a, seen.b, seen.c
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Integer(1), Value::Integer(2), Value::Integer(3)]
    );
}

#[tokio::test]
async fn pairs_iterates_snapshot_map_in_insertion_order() {
    // IndexMap-backed snapshots iterate in insertion order, so the
    // iteration is deterministic.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ first = 1, second = 2, third = 3 })
        local order = {}
        for k, _ in pairs(w:get()) do
            table.insert(order, k)
        end
        return order[1], order[2], order[3]
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("first"),
            Value::string("second"),
            Value::string("third")
        ]
    );
}

#[tokio::test]
async fn pairs_iterates_snapshot_vec() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ 10, 20, 30 })
        local total = 0
        for i, v in pairs(w:get()) do
            total = total + i * v
        end
        return total  -- 1*10 + 2*20 + 3*30 = 140
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(140)]);
}

#[tokio::test]
async fn ipairs_iterates_snapshot_vec() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ "a", "b", "c" })
        local out = ""
        for i, v in ipairs(w:get()) do
            out = out .. tostring(i) .. "=" .. v .. ";"
        end
        return out
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::string("1=a;2=b;3=c;")]);
}

#[tokio::test]
async fn ipairs_terminates_on_snapshot_map_with_no_array_part() {
    // `ipairs` on a string-keyed map terminates immediately; the
    // loop body never executes.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ a = 1, b = 2 })
        local count = 0
        for _ in ipairs(w:get()) do count = count + 1 end
        return count
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(0)]);
}

#[tokio::test]
async fn pairs_yields_proxies_for_nested_tables() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ a = { x = 1 }, b = { x = 2 } })
        local sum = 0
        for _, v in pairs(w:get()) do
            assert(typeof(v) == "snapshot_map")
            sum = sum + v.x
        end
        return sum
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(3)]);
}

#[tokio::test]
async fn pretty_print_renders_snapshot_map_contents() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ a = 1, b = 2 })
        return debug.pretty_print(w:get())
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::string("snapshot_map { a = 1, b = 2 }")]
    );
}

#[tokio::test]
async fn pretty_print_renders_snapshot_vec_contents() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ 10, 20, 30 })
        return debug.pretty_print(w:get())
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::string("snapshot_vec { 10, 20, 30 }")]
    );
}

#[tokio::test]
async fn pretty_print_recurses_into_nested_snapshot_tables() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local w = task.watch({ name = "cfg", values = { 1, 2, 3 } })
        return debug.pretty_print(w:get())
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::string(
            "snapshot_map { name = \"cfg\", values = snapshot_vec { 1, 2, 3 } }"
        )]
    );
}

#[tokio::test]
async fn channel_recv_returns_snapshot_proxy() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local ch = task.bounded_channel(4)
        ch:send({ kind = "msg", value = 42 })
        local got = ch:recv()
        return typeof(got), got.kind, got.value
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("snapshot_map"),
            Value::string("msg"),
            Value::Integer(42),
        ]
    );
}
