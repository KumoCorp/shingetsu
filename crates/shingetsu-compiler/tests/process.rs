//! Integration tests for the `process` standard library.
//!
//! `process.run` spawns real child processes, so these tests drive it
//! with small POSIX commands and assert on the full set of result
//! fields.  Signal-specific expectations are gated to Unix.

mod common;

use common::run_with;
use shingetsu::{Libraries, Value};

/// Builtins plus `process` (via the `EXEC` gate).
const LIBS: Libraries = Libraries::BUILTINS.union(Libraries::EXEC);

/// Run `src` with `process` available and return the result values,
/// panicking with the rendered runtime error if it raises.
async fn run(src: &str) -> Vec<Value> {
    run_with(LIBS, src, |_| {})
        .await
        .expect("run")
        .into_iter()
        .collect()
}

#[tokio::test]
async fn run_argv_captures_stdout_and_status() {
    let vs = run(r#"
        local r = process.run{ cmd = {"printf", "hello"} }
        return r.ok, r.code, r.signal, r.stdout, r.stderr, r.timed_out, r.truncated, r.io_error
        "#)
    .await;
    k9::assert_equal!(
        vs,
        vec![
            Value::Boolean(true),
            Value::Integer(0),
            Value::Nil,
            Value::string("hello"),
            Value::string(""),
            Value::Boolean(false),
            Value::Boolean(false),
            Value::Nil,
        ]
    );
}

#[tokio::test]
async fn run_stdin_broken_pipe_is_not_reported() {
    // `true` never reads its standard input and exits immediately;
    // feeding it a payload larger than a pipe buffer trips a broken pipe
    // that must not flip `ok` or report an `io_error`, matching how a
    // shell treats SIGPIPE from a filter like `head`.
    let vs = run(r#"
        local r = process.run{ cmd = {"true"}, stdin = string.rep("x", 1000000) }
        return r.ok, r.code, r.signal, r.stdout, r.stderr, r.timed_out, r.truncated, r.io_error
        "#)
    .await;
    k9::assert_equal!(
        vs,
        vec![
            Value::Boolean(true),
            Value::Integer(0),
            Value::Nil,
            Value::string(""),
            Value::string(""),
            Value::Boolean(false),
            Value::Boolean(false),
            Value::Nil,
        ]
    );
}

#[tokio::test]
async fn run_accepts_reap_timeout() {
    let vs = run(r#"
        local r = process.run{ cmd = {"printf", "ok"}, reap_timeout = 2.0 }
        return r.ok, r.code, r.signal, r.stdout, r.stderr, r.timed_out, r.truncated, r.io_error
        "#)
    .await;
    k9::assert_equal!(
        vs,
        vec![
            Value::Boolean(true),
            Value::Integer(0),
            Value::Nil,
            Value::string("ok"),
            Value::string(""),
            Value::Boolean(false),
            Value::Boolean(false),
            Value::Nil,
        ]
    );
}

#[tokio::test]
async fn run_writes_stdin_and_reads_transformed_output() {
    let vs = run(r#"
        local r = process.run{ cmd = {"tr", "a-z", "A-Z"}, stdin = "hi there\n" }
        return r.ok, r.stdout
        "#)
    .await;
    k9::assert_equal!(vs, vec![Value::Boolean(true), Value::string("HI THERE\n")]);
}

#[tokio::test]
async fn run_string_command_uses_the_shell() {
    let vs = run(r#"
        local r = process.run{ cmd = "printf hi | tr a-z A-Z" }
        return r.ok, r.stdout
        "#)
    .await;
    k9::assert_equal!(vs, vec![Value::Boolean(true), Value::string("HI")]);
}

#[tokio::test]
async fn run_captures_stderr_separately() {
    let vs = run(r#"
        local r = process.run{ cmd = {"sh", "-c", "printf out; printf err 1>&2"} }
        return r.stdout, r.stderr
        "#)
    .await;
    k9::assert_equal!(vs, vec![Value::string("out"), Value::string("err")]);
}

#[tokio::test]
async fn run_nonzero_exit_reports_code_and_not_ok() {
    let vs = run(r#"
        local r = process.run{ cmd = {"sh", "-c", "exit 42"} }
        return r.ok, r.code, r.signal
        "#)
    .await;
    k9::assert_equal!(
        vs,
        vec![Value::Boolean(false), Value::Integer(42), Value::Nil]
    );
}

#[tokio::test]
async fn run_large_output_does_not_deadlock() {
    // 100000 lines of "0123456789\n" (11 bytes each) is 1_100_000 bytes,
    // far exceeding a pipe buffer; a naive read-after-wait would wedge.
    // Concurrent draining runs it to completion.
    let vs = run(r#"
        local r = process.run{ cmd = {"sh", "-c", "yes 0123456789 | head -n 100000"} }
        return r.ok, #r.stdout
        "#)
    .await;
    k9::assert_equal!(vs, vec![Value::Boolean(true), Value::Integer(1_100_000)]);
}

#[tokio::test]
async fn run_merges_env_onto_inherited() {
    let vs = run(r#"
        local r = process.run{ cmd = {"sh", "-c", "printf %s \"$FOO\""}, env = {FOO = "bar"} }
        return r.stdout
        "#)
    .await;
    k9::assert_equal!(vs, vec![Value::string("bar")]);
}

#[tokio::test]
async fn run_clear_env_drops_inherited_variables() {
    let vs = run(r#"
        local r = process.run{
            cmd = {"sh", "-c", "printf %s \"${INHERITED-unset}\""},
            clear_env = true,
        }
        return r.stdout
        "#)
    .await;
    k9::assert_equal!(vs, vec![Value::string("unset")]);
}

#[cfg(unix)]
#[tokio::test]
async fn run_max_output_truncates_and_kills() {
    let vs = run(r#"
        local r = process.run{ cmd = {"sh", "-c", "yes | head -c 100000000"}, max_output = 1000 }
        return r.truncated, #r.stdout, r.ok, r.signal
        "#)
    .await;
    k9::assert_equal!(
        vs,
        vec![
            Value::Boolean(true),
            Value::Integer(1000),
            Value::Boolean(false),
            Value::Integer(libc::SIGKILL as i64),
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn run_timeout_kills_and_reports() {
    let vs = run(r#"
        local r = process.run{ cmd = {"sleep", "5"}, timeout = 0.3 }
        return r.timed_out, r.ok, r.signal, r.code
        "#)
    .await;
    k9::assert_equal!(
        vs,
        vec![
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Integer(libc::SIGKILL as i64),
            Value::Nil,
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn run_signalled_child_reports_signal() {
    let vs = run(r#"
        local r = process.run{ cmd = {"sh", "-c", "kill -TERM $$"} }
        return r.ok, r.code, r.signal
        "#)
    .await;
    k9::assert_equal!(
        vs,
        vec![
            Value::Boolean(false),
            Value::Nil,
            Value::Integer(libc::SIGTERM as i64),
        ]
    );
}

#[tokio::test]
async fn run_empty_command_vector_raises() {
    let err = run_with(LIBS, "return process.run{ cmd = {} }", |_| {})
        .await
        .expect_err("should raise");
    common::assert_multi_line_output!(
        err,
        "\
error: bad argument #1 to 'process.run' (a non-empty command vector expected, got an empty table)
 --> test.lua:1:19
  |
1 | return process.run{ cmd = {} }
  |                   ^^^^^^^^^^^^ bad argument #1 to 'process.run' (a non-empty command vector expected, got an empty table)
stack traceback:
\ttest.lua:1: in main chunk",
        "empty command vector"
    );
}

#[tokio::test]
async fn run_non_string_non_table_cmd_raises() {
    let err = run_with(LIBS, "return process.run{ cmd = 42 }", |_| {})
        .await
        .expect_err("should raise");
    common::assert_multi_line_output!(
        err,
        "\
error: bad argument #1 to 'run' (string | table for field 'cmd' expected, got number)
 --> test.lua:1:19
  |
1 | return process.run{ cmd = 42 }
  |                   ^^^^^^^^^^^^ bad argument #1 to 'run' (string | table for field 'cmd' expected, got number)
stack traceback:
\ttest.lua:1: in main chunk",
        "non-string non-table cmd"
    );
}

#[tokio::test]
async fn run_argv_with_non_string_element_raises() {
    let err = run_with(LIBS, "return process.run{ cmd = {\"echo\", 42} }", |_| {})
        .await
        .expect_err("should raise");
    common::assert_multi_line_output!(
        err,
        "\
error: bad argument #1 to 'run' (string (OS string) for field 'cmd' expected, got number)
 --> test.lua:1:19
  |
1 | return process.run{ cmd = {\"echo\", 42} }
  |                   ^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'run' (string (OS string) for field 'cmd' expected, got number)
stack traceback:
\ttest.lua:1: in main chunk",
        "argv with non-string element"
    );
}

#[tokio::test]
async fn run_missing_program_raises() {
    let err = run_with(
        LIBS,
        "return process.run{ cmd = {\"/no/such/program/here\"} }",
        |_| {},
    )
    .await
    .expect_err("should raise");
    common::assert_multi_line_output!(
        err,
        "\
error: failed to spawn process: No such file or directory
 --> test.lua:1:8
  |
1 | return process.run{ cmd = {\"/no/such/program/here\"} }
  |        ^^^^^^^^^^^ failed to spawn process: No such file or directory
stack traceback:
\ttest.lua:1: in main chunk",
        "missing program"
    );
}
