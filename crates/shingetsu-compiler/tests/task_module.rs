//! Integration tests for the `task` module's Lua-visible surface.

mod common;

use common::run_in_env;
use parking_lot::Mutex;
use shingetsu::task::{add_observer, register, TaskId, TaskInfo, TaskObserver, TaskOutcome};
use shingetsu::{valuevec, GlobalEnv, Libraries, Value};
use std::sync::Arc;

fn task_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::BUILTINS).expect("register libs");
    register(&env).expect("register task");
    env
}

#[tokio::test]
async fn spawn_unnamed_and_await() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local t = task.spawn(function(a, b) return a + b end, 2, 3)
        return t:await()
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(5)]);
}

#[tokio::test]
async fn spawn_named_records_metadata() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local t = task.spawn("worker", function() return 42 end)
        local got = t:await()
        return got, t:name(), t:is_finished(), t:id()
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::Integer(42),
            Value::string("worker"),
            Value::Boolean(true),
            Value::Integer(1),
        ]
    );
}

#[tokio::test]
async fn pawait_success_returns_true_and_results() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local t = task.spawn(function() return 1, 2, 3 end)
        return t:pawait()
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::Boolean(true),
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
        ]
    );
}

#[tokio::test]
async fn pawait_failure_returns_runtime_error_userdata() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local t = task.spawn(function() error("boom") end)
        local ok, err = t:pawait()
        return ok, type(err), typeof(err), err:message()
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::Boolean(false),
            Value::string("userdata"),
            Value::string("RuntimeError"),
            Value::string("test.lua:2: boom"),
        ]
    );
}

#[tokio::test]
async fn await_propagates_failure() {
    let env = task_env();
    let err = run_in_env(
        &env,
        r#"
        local t = task.spawn(function() error("boom") end)
        return t:await()
        "#,
    )
    .await
    .expect_err("should have raised");
    k9::assert_equal!(
        shingetsu::diagnostic::render_runtime_error(
            &err,
            shingetsu::diagnostic::RenderStyle::Plain,
        ),
        r#"error: test.lua:2: boom
 --> test.lua:3:16
  |
3 |         return t:await()
  |                ^^^^^^^ test.lua:2: boom
stack traceback:
	test.lua:3: in main chunk"#
    );
}

#[tokio::test]
async fn try_result_pending_then_finished() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local t = task.spawn(function() return "done" end)
        local before = t:try_result()
        t:await()
        local ok, val = t:try_result()
        return before, ok, val
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Nil, Value::Boolean(true), Value::string("done")]
    );
}

// ---------------------------------------------------------------------------
// Observer-based tests
// ---------------------------------------------------------------------------

struct EventRecorder {
    events: Mutex<Vec<String>>,
}

impl EventRecorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
        })
    }

    fn snapshot(&self) -> Vec<String> {
        self.events.lock().clone()
    }
}

impl TaskObserver for EventRecorder {
    fn on_spawn(&self, _env: &GlobalEnv, info: &TaskInfo) {
        self.events.lock().push(format!("spawn:{}", info.id));
    }

    fn on_complete(&self, _env: &GlobalEnv, info: &TaskInfo, outcome: &TaskOutcome<'_>) {
        let kind = match outcome {
            TaskOutcome::Success { .. } => "success",
            TaskOutcome::Failure { .. } => "failure",
            TaskOutcome::Cancelled { .. } => "cancelled",
            TaskOutcome::Aborted { .. } => "aborted",
        };
        self.events
            .lock()
            .push(format!("complete:{}:{}", info.id, kind));
    }

    fn on_handle_abandoned(&self, _env: &GlobalEnv, info: &TaskInfo) {
        self.events.lock().push(format!("abandoned:{}", info.id));
    }
}

#[tokio::test]
async fn observer_fires_for_success_and_failure() {
    let env = task_env();
    let recorder = EventRecorder::new();
    add_observer(&env, recorder.clone());

    run_in_env(
        &env,
        r#"
        local a = task.spawn("a", function() return 1 end)
        local b = task.spawn("b", function() error("x") end)
        a:await()
        b:pawait()
        "#,
    )
    .await
    .expect("run");

    k9::assert_equal!(
        recorder.snapshot(),
        vec![
            "spawn:1".to_owned(),
            "spawn:2".to_owned(),
            "complete:1:success".to_owned(),
            "complete:2:failure".to_owned(),
        ]
    );
}

#[tokio::test]
async fn observer_records_parent_for_nested_spawn() {
    struct ParentRecorder {
        parents: Mutex<Vec<(TaskId, Option<TaskId>)>>,
    }
    impl TaskObserver for ParentRecorder {
        fn on_spawn(&self, _env: &GlobalEnv, info: &TaskInfo) {
            self.parents
                .lock()
                .push((info.id, info.parent.as_ref().map(|p| p.id)));
        }
    }

    let env = task_env();
    let recorder = Arc::new(ParentRecorder {
        parents: Mutex::new(Vec::new()),
    });
    add_observer(&env, recorder.clone());

    run_in_env(
        &env,
        r#"
        local outer = task.spawn("outer", function()
            local inner = task.spawn("inner", function() return 1 end)
            inner:await()
        end)
        outer:await()
        "#,
    )
    .await
    .expect("run");

    k9::assert_equal!(
        recorder.parents.lock().clone(),
        vec![(1u64, None), (2u64, Some(1u64))]
    );
}

#[tokio::test]
async fn join_returns_each_tasks_results_as_sub_arrays() {
    // Each task's return list is preserved verbatim as a sub-array,
    // distinguishing no-return tasks (empty sub-array) from
    // one-return tasks holding a single value.
    use shingetsu::FromLuaMulti;
    let env = task_env();
    let raw = run_in_env(
        &env,
        r#"
        local a = task.spawn(function() return 1 end)
        local b = task.spawn(function() return 2, 3 end)
        local c = task.spawn(function() end)
        return task.join({a, b, c})
        "#,
    )
    .await
    .expect("run");
    let results: Vec<Vec<i64>> = FromLuaMulti::from_lua_multi(raw).expect("convert");
    k9::assert_equal!(results, vec![vec![1i64], vec![2, 3], vec![]]);
}

#[tokio::test]
async fn await_all_raises_on_first_failure() {
    let env = task_env();
    let err = run_in_env(
        &env,
        r#"
        local a = task.spawn(function() return 1 end)
        local b = task.spawn(function() error("nope") end)
        task.await_all({a, b})
        "#,
    )
    .await
    .expect_err("should raise");
    k9::assert_equal!(
        shingetsu::diagnostic::render_runtime_error(
            &err,
            shingetsu::diagnostic::RenderStyle::Plain,
        ),
        r#"error: test.lua:3: nope
 --> test.lua:4:9
  |
4 |         task.await_all({a, b})
  |         ^^^^^^^^^^^^^^ test.lua:3: nope
stack traceback:
	test.lua:4: in main chunk"#
    );
}

#[tokio::test]
async fn taskset_yields_completions_with_originating_task() {
    // Each task carries a distinct name; we read the name back
    // from the Task userdata that `:next()` returns alongside the
    // result, then sort the collected names so the assertion is
    // deterministic regardless of completion order.
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local set = task.taskset({
            task.spawn("a", function() return 1 end),
            task.spawn("b", function() return 2 end),
            task.spawn("c", function() return 3 end),
        })
        local names_and_values = {}
        while not set:is_empty() do
            local t, ok, v = set:next()
            assert(ok == true)
            table.insert(names_and_values, t:name() .. ":" .. tostring(v))
        end
        table.sort(names_and_values)
        return table.concat(names_and_values, ","), set:len(), set:is_empty()
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("a:1,b:2,c:3"),
            Value::Integer(0),
            Value::Boolean(true),
        ]
    );
}

#[tokio::test]
async fn taskset_add_extends_an_existing_set() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local set = task.taskset({task.spawn(function() return 1 end)})
        set:add(task.spawn(function() return 2 end))
        set:add(task.spawn(function() return 3 end))
        local sum = 0
        while not set:is_empty() do
            local _, ok, v = set:next()
            assert(ok == true)
            sum = sum + v
        end
        return sum
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Integer(6)]);
}

#[tokio::test]
async fn taskset_next_on_empty_returns_nil() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local set = task.taskset({})
        return set:next(), set:is_empty(), set:len()
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::Nil, Value::Boolean(true), Value::Integer(0)]
    );
}

#[tokio::test]
async fn taskset_for_loop_iterates_completions_via_call_metamethod() {
    use shingetsu::FromLuaMulti;
    let env = task_env();
    let raw = run_in_env(
        &env,
        r#"
        local set = task.taskset({
            task.spawn("a", function() return 1 end),
            task.spawn("b", function() return 2 end),
            task.spawn("c", function() return 3 end),
        })
        local seen = {}
        for t, ok, v in set do
            assert(ok)
            table.insert(seen, t:name() .. ":" .. tostring(v))
        end
        table.sort(seen)
        return seen
        "#,
    )
    .await
    .expect("run");
    let seen: Vec<String> = FromLuaMulti::from_lua_multi(raw).expect("convert");
    k9::assert_equal!(
        seen,
        vec!["a:1".to_owned(), "b:2".to_owned(), "c:3".to_owned()]
    );
}

#[tokio::test]
async fn taskset_yields_failure_pair_for_failed_task() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local set = task.taskset({
            task.spawn("oops", function() error("bad") end),
        })
        local t, ok, err = set:next()
        return t:name(), ok, type(err), typeof(err), err:message()
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::string("oops"),
            Value::Boolean(false),
            Value::string("userdata"),
            Value::string("RuntimeError"),
            Value::string("test.lua:3: bad"),
        ]
    );
}

#[tokio::test]
async fn select_returns_winning_index_and_results() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        -- The slow task sleeps for a long time so the fast task wins.
        local slow = task.spawn(function() task.sleep(60) end)
        local fast = task.spawn(function() return "first" end)
        local idx, ok, val = task.select({slow, fast})
        slow:abort()
        return idx, ok, val
        "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![
            Value::Integer(2),
            Value::Boolean(true),
            Value::string("first"),
        ]
    );
}

#[tokio::test]
async fn cancel_runs_close_handlers_and_reports_cancelled_outcome() {
    let env = task_env();
    let recorder = EventRecorder::new();
    add_observer(&env, recorder.clone());

    run_in_env(
        &env,
        r#"
        local closed = {}
        local guard = setmetatable({}, { __close = function() closed[1] = true end })
        local t = task.spawn("sleeper", function()
            local g <close> = guard
            task.sleep(60)
        end)
        task.yield()
        t:cancel()
        assert(closed[1] == true, "__close did not run")
        "#,
    )
    .await
    .expect("run");

    k9::assert_equal!(
        recorder.snapshot(),
        vec!["spawn:1".to_owned(), "complete:1:cancelled".to_owned()]
    );
}

#[tokio::test]
async fn abort_terminates_without_running_close_handlers() {
    let env = task_env();
    let recorder = EventRecorder::new();
    add_observer(&env, recorder.clone());

    run_in_env(
        &env,
        r#"
        local closed = {}
        local guard = setmetatable({}, { __close = function() closed[1] = true end })
        local t = task.spawn("sleeper", function()
            local g <close> = guard
            task.sleep(60)
        end)
        task.yield()
        t:abort()
        assert(closed[1] == nil, "__close should not run on abort")
        "#,
    )
    .await
    .expect("run");

    k9::assert_equal!(
        recorder.snapshot(),
        vec!["spawn:1".to_owned(), "complete:1:aborted".to_owned()]
    );
}

#[tokio::test]
async fn observer_fires_handle_abandoned_when_dropped_unconsumed() {
    /// Records only `on_handle_abandoned` events so the test
    /// remains deterministic regardless of whether the spawned
    /// task has completed by the time the chunk returns.
    struct AbandonRecorder {
        ids: Mutex<Vec<TaskId>>,
    }
    impl TaskObserver for AbandonRecorder {
        fn on_handle_abandoned(&self, _env: &GlobalEnv, info: &TaskInfo) {
            self.ids.lock().push(info.id);
        }
    }

    let env = task_env();
    let recorder = Arc::new(AbandonRecorder {
        ids: Mutex::new(Vec::new()),
    });
    add_observer(&env, recorder.clone());

    run_in_env(
        &env,
        r#"
        -- Spawn and then drop the handle without awaiting it.
        task.spawn(function() return 1 end)
        "#,
    )
    .await
    .expect("run");

    k9::assert_equal!(recorder.ids.lock().clone(), vec![1u64]);
}
