mod common;

use common::{new_env, run_with_env};
use shingetsu_vm::{valuevec, Function, Value, VmError};

// ---------------------------------------------------------------------------
// Single return value from a SyncPlain native
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_returns_single_integer() {
    let env = new_env();
    env.register_function(Function::wrap("add1", |n: i64| -> Result<i64, VmError> {
        Ok(n + 1)
    }));
    let results = run_with_env(env, "return add1(41)").await;
    k9::assert_equal!(results, valuevec![Value::Integer(42)]);
}

#[tokio::test]
async fn native_returns_single_bool() {
    let env = new_env();
    env.register_function(Function::wrap(
        "is_even",
        |n: i64| -> Result<bool, VmError> { Ok(n % 2 == 0) },
    ));
    let results = run_with_env(env, "return is_even(4)").await;
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn native_returns_single_float() {
    let env = new_env();
    env.register_function(Function::wrap("half", |n: f64| -> Result<f64, VmError> {
        Ok(n / 2.0)
    }));
    let results = run_with_env(env, "return half(7.0)").await;
    k9::assert_equal!(results, valuevec![Value::Float(3.5)]);
}

// ---------------------------------------------------------------------------
// Native return value used in expressions (not just returned)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_return_used_in_addition() {
    let env = new_env();
    env.register_function(Function::wrap("dbl", |n: i64| -> Result<i64, VmError> {
        Ok(n * 2)
    }));
    let results = run_with_env(env, "return dbl(10) + dbl(5)").await;
    k9::assert_equal!(results, valuevec![Value::Integer(30)]);
}

#[tokio::test]
async fn native_return_used_in_comparison() {
    let env = new_env();
    env.register_function(Function::wrap("dbl", |n: i64| -> Result<i64, VmError> {
        Ok(n * 2)
    }));
    let results = run_with_env(env, "return dbl(5) == 10").await;
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
}

#[tokio::test]
async fn native_return_assigned_to_local() {
    let env = new_env();
    env.register_function(Function::wrap("dbl", |n: i64| -> Result<i64, VmError> {
        Ok(n * 2)
    }));
    let results = run_with_env(
        env,
        "local x = dbl(21)\nreturn x",
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(42)]);
}

#[tokio::test]
async fn native_return_passed_to_another_native() {
    let env = new_env();
    env.register_function(Function::wrap("add1", |n: i64| -> Result<i64, VmError> {
        Ok(n + 1)
    }));
    env.register_function(Function::wrap("dbl", |n: i64| -> Result<i64, VmError> {
        Ok(n * 2)
    }));
    let results = run_with_env(env, "return dbl(add1(5))").await;
    k9::assert_equal!(results, valuevec![Value::Integer(12)]);
}

#[tokio::test]
async fn native_return_passed_to_lua_function() {
    let env = new_env();
    env.register_function(Function::wrap("add1", |n: i64| -> Result<i64, VmError> {
        Ok(n + 1)
    }));
    let results = run_with_env(
        env,
        "local function dbl(n) return n * 2 end\nreturn dbl(add1(5))",
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(12)]);
}

// ---------------------------------------------------------------------------
// Multiple return values (should NOT take the single-value shortcut)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_returns_two_values() {
    let env = new_env();
    env.register_function(Function::wrap(
        "divmod",
        |a: i64, b: i64| -> Result<(i64, i64), VmError> { Ok((a / b, a % b)) },
    ));
    let results = run_with_env(env, "return divmod(17, 5)").await;
    k9::assert_equal!(results, valuevec![Value::Integer(3), Value::Integer(2)]);
}

#[tokio::test]
async fn native_returns_two_values_used_in_assignment() {
    let env = new_env();
    env.register_function(Function::wrap(
        "divmod",
        |a: i64, b: i64| -> Result<(i64, i64), VmError> { Ok((a / b, a % b)) },
    ));
    let results = run_with_env(
        env,
        "local q, r = divmod(17, 5)\nreturn q, r",
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Integer(3), Value::Integer(2)]);
}

// ---------------------------------------------------------------------------
// Zero return values
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_returns_unit() {
    let env = new_env();
    env.register_function(Function::wrap("noop", || -> Result<(), VmError> { Ok(()) }));
    let results = run_with_env(env, "local x = noop()\nreturn x").await;
    k9::assert_equal!(results, valuevec![Value::Nil]);
}

// ---------------------------------------------------------------------------
// Return value discarded by caller
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_return_discarded() {
    let env = new_env();
    let counter = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let c = counter.clone();
    env.register_function(Function::wrap("bump", move || -> Result<i64, VmError> {
        Ok(c.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1)
    }));
    let results = run_with_env(
        env,
        "bump()\nbump()\nbump()\nreturn true",
    )
    .await;
    k9::assert_equal!(results, valuevec![Value::Boolean(true)]);
    k9::assert_equal!(
        counter.load(std::sync::atomic::Ordering::Relaxed),
        3
    );
}

// ---------------------------------------------------------------------------
// Native called in a loop (exercises the shortcut path repeatedly)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_called_in_loop() {
    let env = new_env();
    env.register_function(Function::wrap("dbl", |n: i64| -> Result<i64, VmError> {
        Ok(n * 2)
    }));
    let results = run_with_env(
        env,
        r#"
local sum = 0
for i = 1, 100 do
    sum = sum + dbl(i)
end
return sum
"#,
    )
    .await;
    // sum of 2*i for i=1..100 = 2 * (100*101/2) = 10100
    k9::assert_equal!(results, valuevec![Value::Integer(10100)]);
}

// ---------------------------------------------------------------------------
// Native with branching on return value
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_return_used_in_branch() {
    let env = new_env();
    env.register_function(Function::wrap(
        "classify",
        |n: i64| -> Result<i64, VmError> { Ok((n % 3) + 1) },
    ));
    let results = run_with_env(
        env,
        r#"
local total = 0
for i = 1, 30 do
    local k = classify(i)
    if k == 1 then
        total = total + 1
    elseif k == 2 then
        total = total + 10
    else
        total = total + 100
    end
end
return total
"#,
    )
    .await;
    // i%3: 0->k=1, 1->k=2, 2->k=3
    // 10 each of k=1,2,3: 10*1 + 10*10 + 10*100 = 10+100+1000 = 1110
    k9::assert_equal!(results, valuevec![Value::Integer(1110)]);
}

// ---------------------------------------------------------------------------
// Native return value used as table value
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_return_stored_in_table() {
    let env = new_env();
    env.register_function(Function::wrap("dbl", |n: i64| -> Result<i64, VmError> {
        Ok(n * 2)
    }));
    let results = run_with_env(
        env,
        r#"
local t = {}
for i = 1, 5 do
    t[i] = dbl(i)
end
return t[1], t[2], t[3], t[4], t[5]
"#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::Integer(2),
            Value::Integer(4),
            Value::Integer(6),
            Value::Integer(8),
            Value::Integer(10)
        ]
    );
}
