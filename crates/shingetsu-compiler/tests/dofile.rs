mod common;

use shingetsu::valuevec;
use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::{Function, GlobalEnv, Task, Value, ValueVec};

async fn run_dofile(src: &str) -> ValueVec {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = common::new_env_with_load();
    let func = Function::lua(bc.top_level, vec![]);
    Task::new(env, func, vec![]).await.expect("task failed")
}

async fn run_dofile_one(src: &str) -> Value {
    run_dofile(src)
        .await
        .into_iter()
        .next()
        .unwrap_or(Value::Nil)
}

async fn run_dofile_err(src: &str) -> String {
    let compiler = Compiler::new(CompileOptions::default(), Default::default());
    let bc = compiler.compile(src).await.expect("compile failed");
    let env = common::new_env_with_load();
    let func = Function::lua(bc.top_level, vec![]);
    let err = Task::new(env, func, vec![]).await.unwrap_err();
    err.to_string()
}

fn write_temp_lua(content: &str) -> tempfile::NamedTempFile {
    use std::io::Write;
    let mut f = tempfile::Builder::new()
        .suffix(".lua")
        .tempfile()
        .expect("create tempfile");
    f.write_all(content.as_bytes()).expect("write tempfile");
    f.flush().expect("flush");
    f
}

/// Replace a temp file path in a Value with TMPFILE for stable assertions.
fn normalize_path(s: &str, path: &str) -> String {
    s.replace(path, "TMPFILE")
}

/// Normalize temp file paths in all string values of a result vec.
fn normalize_results(results: &[Value], path: &str) -> Vec<String> {
    results
        .iter()
        .map(|v| normalize_path(&v.to_string(), path))
        .collect()
}

// -----------------------------------------------------------------------
// loadfile
// -----------------------------------------------------------------------

#[tokio::test]
async fn loadfile_returns_function() {
    let tmp = write_temp_lua("return 42");
    let path = tmp.path().display().to_string();
    let v = run_dofile_one(&format!(
        r#"
        local f = loadfile("{path}")
        return type(f)
    "#,
    ))
    .await;
    k9::assert_equal!(v, Value::string("function"));
}

#[tokio::test]
async fn loadfile_execute_returns_value() {
    let tmp = write_temp_lua("return 1 + 2");
    let path = tmp.path().display().to_string();
    let v = run_dofile_one(&format!(
        r#"
        local f = loadfile("{path}")
        return f()
    "#,
    ))
    .await;
    k9::assert_equal!(v, Value::Integer(3));
}

#[tokio::test]
async fn loadfile_missing_file_returns_nil_and_message() {
    let results = run_dofile(
        r#"
        local f, err = loadfile("/nonexistent/path.lua")
        return f, err
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::Nil,
            Value::string("cannot open /nonexistent/path.lua: No such file or directory")
        ]
    );
}

#[tokio::test]
async fn loadfile_syntax_error_returns_nil_and_message() {
    let tmp = write_temp_lua("function(");
    let path = tmp.path().display().to_string();
    let results = run_dofile(&format!(
        r#"
        local f, err = loadfile("{path}")
        return f, err
    "#,
    ))
    .await;
    k9::assert_equal!(
        normalize_results(&results, &path),
        vec![
            "nil",
            "TMPFILE:1:9: unexpected token `(`, expected function name"
        ]
    );
}

#[tokio::test]
async fn loadfile_error_shows_file_path() {
    let tmp = write_temp_lua("error('boom')");
    let path = tmp.path().display().to_string();
    let v = run_dofile_one(&format!(
        r#"
        local f = loadfile("{path}")
        local ok, msg = pcall(f)
        return msg
    "#,
    ))
    .await;
    let msg = normalize_path(&v.to_string(), &path);
    k9::assert_equal!(msg, "TMPFILE:1: boom");
}

#[tokio::test]
async fn loadfile_with_env() {
    let tmp = write_temp_lua("return x");
    let path = tmp.path().display().to_string();
    let v = run_dofile_one(&format!(
        r#"
        local env = {{ x = 55 }}
        local f = loadfile("{path}", "t", env)
        return f()
    "#,
    ))
    .await;
    k9::assert_equal!(v, Value::Integer(55));
}

#[tokio::test]
async fn loadfile_mode_b_rejects() {
    let tmp = write_temp_lua("return 1");
    let path = tmp.path().display().to_string();
    let results = run_dofile(&format!(
        r#"
        local f, err = loadfile("{path}", "b")
        return f, err
    "#,
    ))
    .await;
    k9::assert_equal!(
        results,
        valuevec![
            Value::Nil,
            Value::string("attempt to load a text chunk (mode is 'b')")
        ]
    );
}

#[tokio::test]
async fn loadfile_no_args_returns_error() {
    let results = run_dofile(
        r#"
        local f, err = loadfile()
        return f, err
    "#,
    )
    .await;
    k9::assert_equal!(
        results,
        valuevec![Value::Nil, Value::string("filename required")]
    );
}

// -----------------------------------------------------------------------
// dofile
// -----------------------------------------------------------------------

#[tokio::test]
async fn dofile_returns_values() {
    let tmp = write_temp_lua("return 10, 20, 30");
    let path = tmp.path().display().to_string();
    let results = run_dofile(&format!(r#"return dofile("{path}")"#)).await;
    k9::assert_equal!(
        results,
        valuevec![Value::Integer(10), Value::Integer(20), Value::Integer(30)]
    );
}

#[tokio::test]
async fn dofile_sets_globals() {
    let tmp = write_temp_lua("myglobal = 999");
    let path = tmp.path().display().to_string();
    let v = run_dofile_one(&format!(
        r#"
        dofile("{path}")
        return myglobal
    "#,
    ))
    .await;
    k9::assert_equal!(v, Value::Integer(999));
}

#[tokio::test]
async fn dofile_missing_file_errors() {
    let err = run_dofile_err(
        r#"
        dofile("/nonexistent/path.lua")
    "#,
    )
    .await;
    k9::assert_equal!(
        err,
        "cannot open /nonexistent/path.lua: No such file or directory"
    );
}

#[tokio::test]
async fn dofile_syntax_error_propagates() {
    let tmp = write_temp_lua("function(");
    let path = tmp.path().display().to_string();
    let err = run_dofile_err(&format!(r#"dofile("{path}")"#)).await;
    let err = normalize_path(&err, &path);
    k9::assert_equal!(
        err,
        "TMPFILE:1:9: unexpected token `(`, expected function name"
    );
}

#[tokio::test]
async fn dofile_runtime_error_propagates() {
    let tmp = write_temp_lua("error('file boom')");
    let path = tmp.path().display().to_string();
    let err = run_dofile_err(&format!(r#"dofile("{path}")"#)).await;
    let err = normalize_path(&err, &path);
    k9::assert_equal!(err, "TMPFILE:1: file boom");
}

#[tokio::test]
async fn dofile_error_catchable_by_pcall() {
    let tmp = write_temp_lua("error('caught')");
    let path = tmp.path().display().to_string();
    let results = run_dofile(&format!(
        r#"
        local ok, msg = pcall(dofile, "{path}")
        return ok, msg
    "#,
    ))
    .await;
    k9::assert_equal!(
        normalize_results(&results, &path),
        vec!["false", "TMPFILE:1: caught"]
    );
}

#[tokio::test]
async fn dofile_no_args_errors() {
    let err = run_dofile_err(
        r#"
        dofile()
    "#,
    )
    .await;
    k9::assert_equal!(err, "filename required");
}

// -----------------------------------------------------------------------
// Gating
// -----------------------------------------------------------------------

#[tokio::test]
async fn dofile_not_in_sandboxed() {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::SANDBOXED).expect("register");
    k9::assert_equal!(env.get_global("dofile"), None);
    k9::assert_equal!(env.get_global("loadfile"), None);
}

#[tokio::test]
async fn dofile_available_with_load_flag() {
    let env = GlobalEnv::new();
    shingetsu::register_libs(
        &env,
        shingetsu::Libraries::BUILTINS | shingetsu::Libraries::LOAD,
    )
    .expect("register");
    assert!(env.get_global("dofile").is_some());
    assert!(env.get_global("loadfile").is_some());
}
