//! Integration tests for `task.rwlock`.

mod common;

use common::{run_in_env, task_env};
use shingetsu::{valuevec, Value};

#[tokio::test]
async fn anon_read_and_write() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock()
        do
            local g = rw:read()
        end
        do
            local g = rw:write()
        end
        return "ok"
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::string("ok")]);
}

#[tokio::test]
async fn multiple_readers_coexist() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock()
        local r1 = rw:read()
        local r2 = rw:read()
        local r3 = rw:read()
        return r1 ~= nil and r2 ~= nil and r3 ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn try_write_fails_while_read_held() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock()
        local r = rw:read()
        local w = rw:try_write()
        return w
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

#[tokio::test]
async fn try_read_fails_while_write_held() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock()
        local w = rw:write()
        local r = rw:try_read()
        return r
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

#[tokio::test]
async fn explicit_unlock_releases_read_guard() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock()
        local r = rw:read()
        r:unlock()
        local w = rw:try_write()
        return w ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn explicit_unlock_releases_write_guard() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock()
        local w = rw:write()
        w:unlock()
        local r = rw:try_read()
        return r ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn double_unlock_read_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local rw = task.rwlock()
        local r = rw:read()
        r:unlock()
        r:unlock()
    "#,
        "error: rwlock read guard has already been released
 --> test.lua:5:9
  |
5 |         r:unlock()
  |         ^^^^^^^^ rwlock read guard has already been released
stack traceback:
\ttest.lua:5: in main chunk",
    );
}

#[tokio::test]
async fn double_unlock_write_is_an_error() {
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local rw = task.rwlock()
        local w = rw:write()
        w:unlock()
        w:unlock()
    "#,
        "error: rwlock write guard has already been released
 --> test.lua:5:9
  |
5 |         w:unlock()
  |         ^^^^^^^^ rwlock write guard has already been released
stack traceback:
\ttest.lua:5: in main chunk",
    );
}

#[tokio::test]
async fn named_returns_same_rwlock() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local a = task.rwlock("a")
        local b = task.rwlock("a")
        local c = task.rwlock("b")
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
async fn named_rwlock_visible_across_tasks() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock("shared")
        local w = rw:write()
        local t = task.spawn(function()
            local rw2 = task.rwlock("shared")
            local r = rw2:try_read()
            return r == nil
        end)
        local sees_held = t:await()
        w:unlock()
        return sees_held
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn write_awaits_until_readers_release() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock()
        local order = {}
        local r1 = rw:read()
        local r2 = rw:read()
        local writer = task.spawn(function()
            local w = rw:write()
            table.insert(order, "writer")
        end)
        task.yield()
        table.insert(order, "main")
        r1:unlock()
        r2:unlock()
        writer:await()
        return order[1], order[2]
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(
        results,
        valuevec![Value::string("main"), Value::string("writer")]
    );
}

#[tokio::test]
async fn aborted_write_await_does_not_block_others() {
    let env = task_env();
    let results = run_in_env(
        &env,
        r#"
        local rw = task.rwlock()
        local r = rw:read()
        local blocked = task.spawn(function()
            local w = rw:write()
        end)
        task.yield()
        blocked:abort()
        r:unlock()
        local w2 = rw:try_write()
        return w2 ~= nil
    "#,
    )
    .await
    .expect("run");
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn named_type_mismatch_with_mutex_errors() {
    // Confirms the SharedRegistry's type-mismatch diagnostic surfaces
    // when the same name is requested as a different primitive type.
    let env = task_env();
    common::assert_runtime_error_with_env!(
        env,
        r#"
        local m = task.mutex("collide")
        local r = task.rwlock("collide")
    "#,
        "error: bad argument #1 to 'rwlock' (shared registry entry \"collide\" already exists with type \
         shingetsu::task::mutex::LuaMutex, cannot reuse as shingetsu::task::rwlock::LuaRwLock)
 --> test.lua:3:31
  |
3 |         local r = task.rwlock(\"collide\")
  |                               ^^^^^^^^^ bad argument #1 to 'rwlock' (shared registry entry \"collide\" already exists with type shingetsu::task::mutex::LuaMutex, cannot reuse as shingetsu::task::rwlock::LuaRwLock)
stack traceback:
\ttest.lua:3: in main chunk",
    );
}
