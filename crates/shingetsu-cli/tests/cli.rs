use std::io::Write;
use std::process::{Command, Stdio};

/// Returns the path to the `shingetsu` binary built by cargo.
fn shingetsu_bin() -> std::path::PathBuf {
    // `cargo test` sets this env var to the directory containing built
    // binaries, but only for the package under test.  We use
    // `env!("CARGO_BIN_EXE_shingetsu")` when available (requires the
    // `[[bin]]` to be in the same package), otherwise fall back to
    // building via `cargo build` and locating it manually.
    let mut path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // Walk up to workspace root.
    path.pop(); // crates/
    path.pop(); // workspace root
    path.push("target");
    path.push("debug");
    path.push("shingetsu");
    path
}

/// Run a Lua snippet via the CLI and return (stdout, stderr, success).
fn run_lua(code: &str) -> (String, String, bool) {
    run_lua_with(code, |cmd| cmd)
}

/// Run a Lua snippet via the CLI, applying `f` to configure the
/// [`Command`] before spawning.  Returns (stdout, stderr, success).
fn run_lua_with(
    code: &str,
    f: impl FnOnce(&mut Command) -> &mut Command,
) -> (String, String, bool) {
    let mut tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    tmp.write_all(code.as_bytes())
        .expect("failed to write temp file");
    tmp.flush().expect("failed to flush temp file");

    let mut cmd = Command::new(shingetsu_bin());
    cmd.arg("run").arg(tmp.path());
    f(&mut cmd);

    let output = cmd.output().expect("failed to execute shingetsu");

    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

#[test]
fn cli_print_hello() {
    let (stdout, stderr, ok) = run_lua("print('hello world')");
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "hello world");
}

#[test]
fn cli_print_multiple_args() {
    let (stdout, stderr, ok) = run_lua("print(1, 'two', true)");
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "1\ttwo\ttrue");
}

#[test]
fn cli_print_tostring_metamethod() {
    let (stdout, stderr, ok) = run_lua(
        "\
local mt = { __tostring = function() return 'custom' end }
local obj = setmetatable({}, mt)
print(obj)",
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "custom");
}

#[test]
fn cli_print_multiple_lines() {
    let (stdout, stderr, ok) = run_lua(
        "\
print('line1')
print('line2')",
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "line1\nline2");
}

// =========================================================================
// Stdio with exotic fd assignments
// =========================================================================

/// Feed a file as stdin and read it with `io.read("*a")`.
#[test]
fn stdio_stdin_from_file_read_all() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    input.write_all(b"hello from file").expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let (stdout, stderr, ok) = run_lua_with("io.write(io.read('*a'))", |cmd| cmd.stdin(input_file));
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "hello from file");
}

/// Feed a file as stdin and read it line-by-line.
#[test]
fn stdio_stdin_from_file_read_lines() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    input.write_all(b"alpha\nbeta\ngamma\n").expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let (stdout, stderr, ok) = run_lua_with(
        r#"
local lines = {}
local line = io.read("*l")
while line ~= nil do
    lines[#lines + 1] = line
    line = io.read("*l")
end
io.write(table.concat(lines, ","))
"#,
        |cmd| cmd.stdin(input_file),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "alpha,beta,gamma");
}

/// Capture stdout to a file via `io.write`.
#[test]
fn stdio_stdout_to_file() {
    let out_file = tempfile::NamedTempFile::new().expect("tmp");
    let writer = out_file.reopen().expect("reopen for write");

    let (_stdout, stderr, ok) =
        run_lua_with("io.write('captured output')", |cmd| cmd.stdout(writer));
    assert!(ok, "shingetsu exited with error: {stderr}");
    let captured = std::fs::read_to_string(out_file.path()).expect("read back");
    k9::assert_equal!(captured, "captured output");
}

/// Capture stderr to a file via `io.stderr:write`.
#[test]
fn stdio_stderr_to_file() {
    let err_file = tempfile::NamedTempFile::new().expect("tmp");
    let writer = err_file.reopen().expect("reopen for write");

    let (stdout, _stderr, ok) = run_lua_with(
        r#"
io.stderr:write("error output")
io.write("ok")
"#,
        |cmd| cmd.stderr(writer),
    );
    assert!(ok, "shingetsu exited with error, stdout: {stdout}");
    let captured = std::fs::read_to_string(err_file.path()).expect("read back");
    k9::assert_equal!(captured, "error output");
}

/// When stdin is a regular file, `io.stdin:seek` should succeed.
#[test]
fn stdio_stdin_seekable_file() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    input.write_all(b"abcdefghij").expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let (stdout, stderr, ok) = run_lua_with(
        r#"
-- Read 3 bytes, seek back to start, read again.
local first = io.read(3)
io.stdin:seek("set", 0)
local again = io.read(3)
io.write(first .. "," .. again)
"#,
        |cmd| cmd.stdin(input_file),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "abc,abc");
}

/// When stdin is a pipe, `io.stdin:seek` should fail gracefully.
#[test]
fn stdio_stdin_pipe_not_seekable() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"
local ok, err = pcall(function() io.stdin:seek("set", 0) end)
io.write(tostring(ok))
"#,
        |cmd| cmd.stdin(Stdio::piped()),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "false");
}

/// Redirect all three stdio handles simultaneously.
#[test]
fn stdio_all_three_redirected() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    input.write_all(b"input data").expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let out_file = tempfile::NamedTempFile::new().expect("tmp");
    let out_writer = out_file.reopen().expect("reopen");

    let err_file = tempfile::NamedTempFile::new().expect("tmp");
    let err_writer = err_file.reopen().expect("reopen");

    let (_stdout, _stderr, ok) = run_lua_with(
        r#"
local data = io.read("*a")
io.write("out:" .. data)
io.stderr:write("err:" .. data)
"#,
        |cmd| cmd.stdin(input_file).stdout(out_writer).stderr(err_writer),
    );
    assert!(ok, "process failed");
    let out = std::fs::read_to_string(out_file.path()).expect("read stdout");
    let err = std::fs::read_to_string(err_file.path()).expect("read stderr");
    k9::assert_equal!(out, "out:input data");
    k9::assert_equal!(err, "err:input data");
}

/// Read numbers from a file piped to stdin.
#[test]
fn stdio_stdin_read_numbers() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    input.write_all(b"  42.5  99  ").expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let (stdout, stderr, ok) = run_lua_with(
        r#"
local a = io.read("*n")
local b = io.read("*n")
io.write(tostring(a) .. "," .. tostring(b))
"#,
        |cmd| cmd.stdin(input_file),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    // io.read("*n") returns floats; tostring(99.0) => "99.0"
    k9::assert_equal!(stdout, "42.5,99.0");
}

// =========================================================================
// Sandboxed mode
// =========================================================================

/// In sandboxed mode, `io` and `os` are not available.
#[test]
fn sandboxed_no_io_no_os() {
    let (stdout, stderr, ok) =
        run_lua_with("print(type(io), type(os))", |cmd| cmd.arg("--sandboxed"));
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "nil\tnil");
}

/// In sandboxed mode, sandbox-safe libs are still available.
#[test]
fn sandboxed_has_safe_libs() {
    let (stdout, stderr, ok) = run_lua_with(
        "print(type(math), type(string), type(table), type(utf8))",
        |cmd| cmd.arg("--sandboxed"),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "table\ttable\ttable\ttable");
}

/// --sandboxed --libraries os enables only the os library.
#[test]
fn sandboxed_with_os() {
    let (stdout, stderr, ok) = run_lua_with("print(type(os), type(io))", |cmd| {
        cmd.arg("--sandboxed").arg("--libraries").arg("os")
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "table\tnil");
}

/// --sandboxed --libraries io enables file I/O but not stdio handles.
#[test]
fn sandboxed_with_io_no_stdio() {
    let (stdout, stderr, ok) = run_lua_with("print(type(io), io.stdin)", |cmd| {
        cmd.arg("--sandboxed").arg("--libraries").arg("io")
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "table\tnil");
}

/// --sandboxed --libraries stdio enables stdio (and implicitly io).
#[test]
fn sandboxed_with_stdio() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"
io.write("from stdio")
"#,
        |cmd| cmd.arg("--sandboxed").arg("--libraries").arg("stdio"),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "from stdio");
}

/// --sandboxed --libraries io,os,stdio enables everything.
#[test]
fn sandboxed_with_all_flags() {
    let (stdout, stderr, ok) = run_lua_with("print(type(io), type(os), io.stdin ~= nil)", |cmd| {
        cmd.arg("--sandboxed").arg("--libraries").arg("io,os,stdio")
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "table\ttable\ttrue");
}

// =========================================================================
// Additional stdio coverage
// =========================================================================

/// io.write with multiple arguments concatenates them.
#[test]
fn stdio_write_multiple_args() {
    let (stdout, stderr, ok) = run_lua(r#"io.write("a", "b", "c")"#);
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "abc");
}

/// Empty stdin: io.read("*a") returns an empty string.
#[test]
fn stdio_empty_stdin_read_all() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"local data = io.read("*a")
io.write("[" .. data .. "]")
"#,
        |cmd| cmd.stdin(Stdio::null()),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "[]");
}

/// Empty stdin: io.read("*l") returns nil.
#[test]
fn stdio_empty_stdin_read_line() {
    let (stdout, stderr, ok) = run_lua_with(r#"io.write(tostring(io.read("*l")))"#, |cmd| {
        cmd.stdin(Stdio::null())
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "nil");
}

/// Large data through stdin (bigger than 8KB internal buffer).
#[test]
fn stdio_large_stdin() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    // Write 32KB of data.
    let data = "x".repeat(32 * 1024);
    input.write_all(data.as_bytes()).expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let (stdout, stderr, ok) = run_lua_with(
        r#"
local data = io.read("*a")
io.write(tostring(#data))
"#,
        |cmd| cmd.stdin(input_file),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "32768");
}

/// Binary data round-trip through stdin/stdout.
#[test]
fn stdio_binary_round_trip() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    // Write bytes 0x00..0xFF.
    let data: Vec<u8> = (0..=255).collect();
    input.write_all(&data).expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let out_file = tempfile::NamedTempFile::new().expect("tmp");
    let out_writer = out_file.reopen().expect("reopen");

    let (_stdout, stderr, ok) = run_lua_with(
        r#"
local data = io.read("*a")
io.write(data)
"#,
        |cmd| cmd.stdin(input_file).stdout(out_writer),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    let output = std::fs::read(out_file.path()).expect("read back");
    k9::assert_equal!(output.len(), 256);
    k9::assert_equal!(output, data);
}

/// io.type(io.stdin) returns "file".
#[test]
fn stdio_io_type_stdin() {
    let (stdout, stderr, ok) = run_lua(r#"io.write(io.type(io.stdin))"#);
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "file");
}

/// io.read() with no args defaults to "*l".
#[test]
fn stdio_read_default_is_line() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    input.write_all(b"first\nsecond\n").expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let (stdout, stderr, ok) = run_lua_with(r#"io.write(io.read())"#, |cmd| cmd.stdin(input_file));
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "first");
}

// =========================================================================
// io.popen
// =========================================================================

/// io.popen in read mode captures child stdout.
#[test]
fn popen_read_echo() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("echo hello")
local data = f:read("*a")
f:close()
io.write(data)
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "hello");
}

/// io.popen read mode: child inherits parent's stdin.
#[test]
fn popen_read_inherits_stdin() {
    let mut input = tempfile::NamedTempFile::new().expect("tmp");
    input.write_all(b"from parent stdin").expect("write");
    input.flush().expect("flush");
    let input_file = std::fs::File::open(input.path()).expect("reopen");

    let (stdout, stderr, ok) = run_lua_with(
        r#"
local f = io.popen("cat")
local data = f:read("*a")
f:close()
io.write(data)
"#,
        |cmd| cmd.stdin(input_file),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "from parent stdin");
}

/// io.popen in write mode pipes to child stdin.
#[test]
fn popen_write_to_child() {
    let out_file = tempfile::NamedTempFile::new().expect("tmp");
    let out_writer = out_file.reopen().expect("reopen");

    let (_stdout, stderr, ok) = run_lua_with(
        r#"
local f = io.popen("cat", "w")
f:write("hello from parent")
f:close()
"#,
        |cmd| cmd.stdout(out_writer),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    let captured = std::fs::read_to_string(out_file.path()).expect("read back");
    k9::assert_equal!(captured, "hello from parent");
}

/// f:close() on popen handle returns exit status.
#[test]
fn popen_close_exit_status() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("exit 0")
f:read("*a")
local ok1, how1, code1 = f:close()

local f2 = io.popen("exit 42")
f2:read("*a")
local ok2, how2, code2 = f2:close()

io.write(tostring(ok1) .. "," .. how1 .. "," .. tostring(code1))
io.write("|")
io.write(tostring(ok2) .. "," .. how2 .. "," .. tostring(code2))
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "true,exit,0|nil,exit,42");
}

/// f:close() on popen handle returns signal info when child is killed.
#[test]
fn popen_close_signal() {
    let (stdout, stderr, ok) = run_lua(
        r#"
-- spawn a process that kills itself with SIGTERM (signal 15)
local f = io.popen("kill -15 $$")
f:read("*a")
local ok, how, code = f:close()
io.write(tostring(ok) .. "," .. how .. "," .. tostring(code))
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "nil,signal,15");
}

/// io.popen read mode: read line by line.
#[test]
fn popen_read_lines() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("printf 'a\nb\nc\n'")
local lines = {}
local line = f:read("*l")
while line ~= nil do
    lines[#lines + 1] = line
    line = f:read("*l")
end
f:close()
io.write(table.concat(lines, ","))
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "a,b,c");
}

/// io.popen with invalid mode returns an error.
#[test]
fn popen_invalid_mode() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local ok, err = pcall(io.popen, "echo hi", "x")
io.write(tostring(ok))
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "false");
}

/// io.popen is not available in sandboxed mode without --exec.
#[test]
fn popen_not_in_sandbox() {
    let (stdout, stderr, ok) = run_lua_with("print(io.popen)", |cmd| {
        cmd.arg("--sandboxed").arg("--libraries").arg("io")
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "nil");
}

/// io.popen is available with --sandboxeded --exec.
#[test]
fn popen_with_exec_flag() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"
local f = io.popen("echo sandbox_exec")
io.write(f:read("*a"))
f:close()
"#,
        |cmd| cmd.arg("--sandboxed").arg("--libraries").arg("exec,stdio"),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "sandbox_exec");
}

/// io.popen seek should fail (pipes are not seekable).
#[test]
fn popen_seek_fails() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("echo hi")
local ok, err = pcall(function() f:seek("set", 0) end)
f:close()
io.write(tostring(ok))
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "false");
}

/// io.popen default mode is "r".
#[test]
fn popen_default_mode_is_read() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("echo default_read")
local data = f:read("*a")
f:close()
io.write(data)
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "default_read");
}

/// io.type on a popen handle returns "file".
#[test]
fn popen_io_type() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("echo hi")
io.write(io.type(f))
f:read("*a")
f:close()
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "file");
}

/// io.type on a closed popen handle returns "closed file".
#[test]
fn popen_io_type_closed() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("echo hi")
f:read("*a")
f:close()
io.write(io.type(f))
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "closed file");
}

/// f:setvbuf on a popen handle works.
#[test]
fn popen_setvbuf() {
    let out_file = tempfile::NamedTempFile::new().expect("tmp");
    let out_writer = out_file.reopen().expect("reopen");

    let (_stdout, stderr, ok) = run_lua_with(
        r#"
local f = io.popen("cat", "w")
f:setvbuf("no")
f:write("unbuffered")
f:close()
"#,
        |cmd| cmd.stdout(out_writer),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    let captured = std::fs::read_to_string(out_file.path()).expect("read back");
    k9::assert_equal!(captured, "unbuffered");
}

/// io.popen read: child with empty output.
#[test]
fn popen_read_empty_output() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("true")
local data = f:read("*a")
f:close()
io.write("[" .. data .. "]")
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "[]");
}

/// io.popen read numbers from child output.
#[test]
fn popen_read_number() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("echo '42.5 99'")
local a = f:read("*n")
local b = f:read("*n")
f:close()
io.write(tostring(a) .. "," .. tostring(b))
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "42.5,99.0");
}

/// io.popen read: child stderr goes to parent stderr.
#[test]
fn popen_stderr_inheritance() {
    let err_file = tempfile::NamedTempFile::new().expect("tmp");
    let err_writer = err_file.reopen().expect("reopen");

    let (stdout, _stderr, ok) = run_lua_with(
        r#"
local f = io.popen("echo error_msg >&2; echo ok")
local data = f:read("*a")
f:close()
io.write(data)
"#,
        |cmd| cmd.stderr(err_writer),
    );
    assert!(ok, "shingetsu exited with error");
    k9::assert_equal!(stdout.trim(), "ok");
    let captured = std::fs::read_to_string(err_file.path()).expect("read back");
    k9::assert_equal!(captured.trim(), "error_msg");
}

/// io.popen write: large data through pipe.
#[test]
fn popen_write_large_data() {
    let out_file = tempfile::NamedTempFile::new().expect("tmp");
    let out_writer = out_file.reopen().expect("reopen");

    let (_stdout, stderr, ok) = run_lua_with(
        r#"
local f = io.popen("cat", "w")
local chunk = string.rep("x", 1024)
for i = 1, 32 do
    f:write(chunk)
end
f:close()
"#,
        |cmd| cmd.stdout(out_writer),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    let captured = std::fs::read(out_file.path()).expect("read back");
    k9::assert_equal!(captured.len(), 32 * 1024);
}

/// io.popen: successful exit returns true, "exit", 0.
#[test]
fn popen_close_success_exit() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("true")
f:read("*a")
local ok, how, code = f:close()
io.write(tostring(ok) .. "," .. how .. "," .. tostring(code))
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "true,exit,0");
}

/// io.popen: running a nonexistent command exits with error code.
#[test]
fn popen_nonexistent_command_exit_code() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("/nonexistent_binary_xyz_42")
f:read("*a")
local ok, how, code = f:close()
io.write(tostring(ok) .. "," .. how)
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    // Shell reports command-not-found as a non-zero exit.
    k9::assert_equal!(stdout, "nil,exit");
}

/// io.open in append mode via CLI.
#[test]
fn cli_io_open_append() {
    let out_file = tempfile::NamedTempFile::new().expect("tmp");
    std::io::Write::write_all(&mut out_file.reopen().expect("reopen"), b"existing ")
        .expect("write");
    let path = out_file.path().to_str().expect("path");

    let (stdout, stderr, ok) = run_lua(&format!(
        r#"
local f = io.open("{path}", "a")
f:write("appended")
f:close()
local r = io.open("{path}", "r")
io.write(r:read("*a"))
r:close()
"#
    ));
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "existing appended");
}

/// When stdout is a read-only fd, io.write should produce a portable
/// EBADF error message.
#[test]
fn stdio_write_to_read_only_fd() {
    // Open a file for reading and pass it as the child's stdout.
    let input = tempfile::NamedTempFile::new().expect("tmp");
    let read_only = std::fs::File::open(input.path()).expect("open read-only");

    let (_stdout, stderr, ok) = run_lua_with(
        r#"
local _ok, err = pcall(function()
    io.write("hello")
    io.flush()
end)
io.stderr:write(tostring(err))
"#,
        |cmd| cmd.stdout(read_only),
    );
    assert!(!ok || !stderr.is_empty(), "expected error output");
    let err_output = stderr.trim();
    k9::assert_equal!(err_output, "Bad file descriptor");
}

/// io.popen read with interleaved read formats.
#[test]
fn popen_read_mixed_formats() {
    let (stdout, stderr, ok) = run_lua(
        r#"
local f = io.popen("printf '42 hello\nworld'")
local n = f:read("*n")
local line = f:read("*l")
local rest = f:read("*a")
f:close()
io.write(tostring(n) .. "|" .. line .. "|" .. rest)
"#,
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "42.0| hello|world");
}

// =========================================================================
// os.getenv / --env flag
//
// These tests spawn the CLI with explicitly-set environment variables via
// `Command::env` so we can positively assert round-trip values, rather
// than relying on whatever `PATH` happens to hold in the parent process.
// =========================================================================

/// os.getenv returns the exact value set in the child's environment.
#[test]
fn getenv_reads_set_value() {
    let (stdout, stderr, ok) =
        run_lua_with(r#"io.write(os.getenv("SHINGETSU_TEST_FOO"))"#, |cmd| {
            cmd.env("SHINGETSU_TEST_FOO", "bar_value")
        });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "bar_value");
}

/// os.getenv returns nil for a variable explicitly removed from the child's env.
#[test]
fn getenv_unset_returns_nil() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"io.write(tostring(os.getenv("SHINGETSU_TEST_UNSET_XYZ")))"#,
        |cmd| cmd.env_remove("SHINGETSU_TEST_UNSET_XYZ"),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "nil");
}

/// os.getenv preserves an empty-string value (distinct from unset).
#[test]
fn getenv_empty_string_value() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"
local v = os.getenv("SHINGETSU_TEST_EMPTY")
io.write("type=" .. type(v) .. ",len=" .. tostring(#v))
"#,
        |cmd| cmd.env("SHINGETSU_TEST_EMPTY", ""),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "type=string,len=0");
}

/// os.getenv preserves spaces in the value verbatim.
#[test]
fn getenv_value_with_spaces() {
    let (stdout, stderr, ok) =
        run_lua_with(r#"io.write(os.getenv("SHINGETSU_TEST_SPACES"))"#, |cmd| {
            cmd.env("SHINGETSU_TEST_SPACES", "hello world  with  spaces")
        });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "hello world  with  spaces");
}

/// `=` is permitted in env var *values* (only names cannot contain it).
#[test]
fn getenv_value_with_equals_sign() {
    let (stdout, stderr, ok) = run_lua_with(r#"io.write(os.getenv("SHINGETSU_TEST_EQ"))"#, |cmd| {
        cmd.env("SHINGETSU_TEST_EQ", "key=value:other=thing")
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "key=value:other=thing");
}

/// Multiple env vars set simultaneously are each individually retrievable.
#[test]
fn getenv_multiple_vars() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"
io.write(os.getenv("SHINGETSU_TEST_A") .. "|")
io.write(os.getenv("SHINGETSU_TEST_B") .. "|")
io.write(os.getenv("SHINGETSU_TEST_C"))
"#,
        |cmd| {
            cmd.env("SHINGETSU_TEST_A", "aaa")
                .env("SHINGETSU_TEST_B", "bbb")
                .env("SHINGETSU_TEST_C", "ccc")
        },
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "aaa|bbb|ccc");
}

/// Value containing UTF-8 multi-byte characters round-trips byte-for-byte.
#[test]
fn getenv_utf8_value() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"
local v = os.getenv("SHINGETSU_TEST_UTF8")
-- Emit length in bytes plus the value itself.
io.write(tostring(#v) .. ":" .. v)
"#,
        |cmd| cmd.env("SHINGETSU_TEST_UTF8", "café—\u{1F680}"),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    // "café—🚀" = 4+2+3+4 = let's compute: c(1) a(1) f(1) é(2) —(3) 🚀(4) = 12 bytes.
    k9::assert_equal!(stdout, "12:café—\u{1F680}");
}

/// Raw (non-UTF-8) bytes round-trip through os.getenv on Unix.
#[cfg(unix)]
#[test]
fn getenv_raw_bytes_unix() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    // Invalid UTF-8: lone high-bit bytes that cannot begin a UTF-8 sequence.
    let value = OsString::from_vec(vec![b'a', 0xFF, 0xFE, b'z']);

    let (stdout, stderr, ok) = run_lua_with(
        r#"
local v = os.getenv("SHINGETSU_TEST_BYTES")
io.write("len=" .. tostring(#v))
io.write(",b1=" .. tostring(string.byte(v, 1)))
io.write(",b2=" .. tostring(string.byte(v, 2)))
io.write(",b3=" .. tostring(string.byte(v, 3)))
io.write(",b4=" .. tostring(string.byte(v, 4)))
"#,
        |cmd| cmd.env("SHINGETSU_TEST_BYTES", &value),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "len=4,b1=97,b2=255,b3=254,b4=122");
}

/// In sandboxed mode without --env, os.getenv is not registered even
/// when --os is present.
#[test]
fn getenv_absent_without_env_flag_in_sandbox() {
    let (stdout, stderr, ok) = run_lua_with(r#"print(os.getenv)"#, |cmd| {
        cmd.arg("--sandboxed")
            .arg("--libraries")
            .arg("os")
            .env("SHINGETSU_TEST_FOO", "should_not_be_reachable")
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "nil");
}

/// In sandboxed mode --env exposes os.getenv and values round-trip.
#[test]
fn getenv_available_with_env_flag_in_sandbox() {
    let (stdout, stderr, ok) =
        run_lua_with(r#"io.write(os.getenv("SHINGETSU_TEST_FOO"))"#, |cmd| {
            cmd.arg("--sandboxed")
                .arg("--libraries")
                .arg("env,stdio")
                .env("SHINGETSU_TEST_FOO", "sandbox_value")
        });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "sandbox_value");
}

/// --libraries env without --sandboxed or builtins does not register print,
/// so the script fails at runtime.
#[test]
fn libraries_env_alone_has_no_builtins() {
    let (_stdout, _stderr, ok) =
        run_lua_with(r#"print("hello")"#, |cmd| cmd.arg("--libraries").arg("env"));
    assert!(!ok, "expected runtime error (print not available)");
}

// =========================================================================
// os.exit / --exit flag
//
// These tests spawn the CLI and observe the real process exit code via
// `ExitStatus::code()`, proving that the VM error propagates all the way
// out to `std::process::exit`.  The in-process tests in
// shingetsu-compiler/tests/os_lib.rs already cover the VmError shape and
// `<close>` metamethod dispatch; here we focus on what the embedder
// observes.
// =========================================================================

/// Run a Lua snippet via the CLI, returning (stdout, stderr, exit_code).
/// `exit_code` is `None` if the child was killed by a signal.
fn run_lua_exit_code(
    code: &str,
    f: impl FnOnce(&mut Command) -> &mut Command,
) -> (String, String, Option<i32>) {
    let mut tmp = tempfile::NamedTempFile::new().expect("tmp");
    tmp.write_all(code.as_bytes()).expect("write");
    tmp.flush().expect("flush");

    let mut cmd = Command::new(shingetsu_bin());
    cmd.arg("run").arg(tmp.path());
    f(&mut cmd);

    let output = cmd.output().expect("spawn");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.code(),
    )
}

/// os.exit() with no arguments exits with status 0.
#[test]
fn exit_no_args_is_success() {
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
print("before")
os.exit()
print("unreachable")
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(0));
    k9::assert_equal!(stdout.trim(), "before");
}

/// os.exit(true) exits with status 0.
#[test]
fn exit_true_is_zero() {
    let (_stdout, _stderr, code) = run_lua_exit_code("os.exit(true)", |cmd| cmd);
    k9::assert_equal!(code, Some(0));
}

/// os.exit(false) exits with status 1.
#[test]
fn exit_false_is_one() {
    let (_stdout, _stderr, code) = run_lua_exit_code("os.exit(false)", |cmd| cmd);
    k9::assert_equal!(code, Some(1));
}

/// os.exit(42) exits with status 42.
#[test]
fn exit_integer_code() {
    let (_stdout, _stderr, code) = run_lua_exit_code("os.exit(42)", |cmd| cmd);
    k9::assert_equal!(code, Some(42));
}

/// os.exit(0) exits cleanly with status 0 even with code explicitly provided.
#[test]
fn exit_zero_integer() {
    let (_stdout, _stderr, code) = run_lua_exit_code("os.exit(0)", |cmd| cmd);
    k9::assert_equal!(code, Some(0));
}

/// Unix exit codes are 8-bit: passing 300 returns 300 & 0xff = 44.
/// We verify that truncation happens at the OS level, not silently
/// inside our code (our layer passes i32 through unchanged).
#[test]
fn exit_large_code_os_truncates() {
    let (_stdout, _stderr, code) = run_lua_exit_code("os.exit(300)", |cmd| cmd);
    // POSIX wait status encodes only the low 8 bits of the exit code.
    k9::assert_equal!(code, Some(44));
}

/// Buffered stdio is flushed on exit, so a print just before os.exit
/// appears on the child's stdout.
#[test]
fn exit_flushes_stdio() {
    // Use io.write (explicitly buffered) rather than print() to
    // exercise the flush path, and omit trailing newline so any
    // un-flushed buffer would be observable as missing output.
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
io.write("flushed no newline")
os.exit(0)
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(0));
    k9::assert_equal!(stdout, "flushed no newline");
}

/// pcall does NOT catch os.exit — the child terminates even though
/// the exit call is wrapped.
#[test]
fn exit_pcall_does_not_catch() {
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
local ok, err = pcall(os.exit, 13)
print("unreachable")
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(13));
    k9::assert_equal!(stdout, "");
}

/// `<close>` locals have __close dispatched on the exit unwind path,
/// even though their enclosing frame never returned normally.
#[test]
fn exit_runs_close_metamethod_end_to_end() {
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
local mt = { __close = function() io.write("closed!") end }
local guard <close> = setmetatable({}, mt)
io.write("before ")
os.exit(0)
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(0));
    k9::assert_equal!(stdout, "before closed!");
}

/// With --sandboxed but no --exit, os.exit is not registered.
#[test]
fn exit_absent_without_exit_flag_in_sandbox() {
    let (stdout, _stderr, code) = run_lua_exit_code(r#"print(os.exit)"#, |cmd| {
        cmd.arg("--sandboxed").arg("--libraries").arg("os")
    });
    k9::assert_equal!(code, Some(0));
    k9::assert_equal!(stdout.trim(), "nil");
}

/// --sandboxed --libraries exit exposes os.exit and termination works.
#[test]
fn exit_available_with_exit_flag_in_sandbox() {
    let (_stdout, _stderr, code) = run_lua_exit_code(r#"os.exit(77)"#, |cmd| {
        cmd.arg("--sandboxed").arg("--libraries").arg("exit")
    });
    k9::assert_equal!(code, Some(77));
}

/// --libraries exit without builtins does not register print,
/// so the script fails at runtime.
#[test]
fn libraries_exit_alone_has_no_builtins() {
    let (_stdout, _stderr, code) =
        run_lua_exit_code(r#"print("hi")"#, |cmd| cmd.arg("--libraries").arg("exit"));
    assert_ne!(
        code,
        Some(0),
        "expected runtime error (print not available)"
    );
}

/// os.exit(true, true) — close=true triggers `__gc` finalizer
/// dispatch (via `GlobalEnv::dispose`).  We observe this by setting
/// up a table with a `__gc` metamethod and asserting that the
/// finalizer output appears on stdout before the process terminates.
#[test]
fn exit_close_true_runs_gc_finalizer() {
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
local mt = { __gc = function() io.write("gc ran|") end }
-- Create a table with __gc metatable, then drop the reference
-- so the collector finds it unreachable.
do
    local obj = setmetatable({}, mt)
end
io.write("before|")
os.exit(0, true)
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(0));
    k9::assert_equal!(stdout, "before|gc ran|");
}

/// os.exit(code) with close=false (default) skips `__gc` finalizers.
#[test]
fn exit_close_false_skips_gc_finalizer() {
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
local mt = { __gc = function() io.write("gc ran|") end }
do
    local obj = setmetatable({}, mt)
end
io.write("before|")
os.exit(0) -- close defaults to false
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(0));
    // "before|" appears; "gc ran|" does NOT (no close=true, no dispose).
    k9::assert_equal!(stdout, "before|");
}

/// close=true must run every tracked `__gc` finalizer, not just one.
/// If dispose's loop had an early-return bug this would catch it.
#[test]
fn exit_close_true_runs_multiple_gc_finalizers() {
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
local function setup()
    local a = setmetatable({}, { __gc = function() io.write("a|") end })
    local b = setmetatable({}, { __gc = function() io.write("b|") end })
    local c = setmetatable({}, { __gc = function() io.write("c|") end })
end
setup()
io.write("before|")
os.exit(0, true)
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(0));
    // Order of finalizer calls depends on mark/sweep internals; we
    // only assert every tag appears (each exactly once).
    assert!(stdout.starts_with("before|"), "got {:?}", stdout);
    k9::assert_equal!(stdout.matches("a|").count(), 1);
    k9::assert_equal!(stdout.matches("b|").count(), 1);
    k9::assert_equal!(stdout.matches("c|").count(), 1);
}

/// A `__gc` finalizer that raises must not abort the dispose loop —
/// subsequent finalizers still run, and the process still exits with
/// the requested code.  `run_pending_finalizers` discards errors via
/// `let _ = task.await` to make this guarantee.
#[test]
fn exit_close_true_gc_finalizer_error_does_not_abort() {
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
local function setup()
    local a = setmetatable({}, { __gc = function() error("bad finalizer") end })
    local b = setmetatable({}, { __gc = function() io.write("b_ran|") end })
end
setup()
io.write("before|")
os.exit(0, true)
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(0));
    // `before|` always appears; `b_ran|` must appear despite a's
    // finalizer raising.
    assert!(stdout.starts_with("before|"), "got {:?}", stdout);
    k9::assert_equal!(stdout.matches("b_ran|").count(), 1);
}

/// close=true with a non-zero exit code: both the code and the
/// finalizer dispatch must work together.  Combines the two paths
/// that the single-purpose tests above exercise separately.
#[test]
fn exit_close_true_nonzero_code_runs_gc_finalizer() {
    let (stdout, _stderr, code) = run_lua_exit_code(
        r#"
local function setup()
    local t = setmetatable({}, { __gc = function() io.write("gc|") end })
end
setup()
io.write("before|")
os.exit(7, true)
"#,
        |cmd| cmd,
    );
    k9::assert_equal!(code, Some(7));
    k9::assert_equal!(stdout, "before|gc|");
}

/// os.exit is available by default (without `--exit`) when the CLI
/// is not in sandboxed mode — non-sandboxed enables `Libraries::ALL`
/// which includes EXIT.  Every other `exit_*` CLI test runs in this
/// mode implicitly; this test makes the contract explicit.
#[test]
fn exit_available_in_non_sandboxed_default_mode() {
    let (_stdout, _stderr, code) = run_lua_exit_code(r#"os.exit(33)"#, |cmd| cmd);
    k9::assert_equal!(code, Some(33));
}

// ---------------------------------------------------------------------------
// debug library
// ---------------------------------------------------------------------------

/// The `debug` table is present in default (non-sandboxed) mode.
#[test]
fn debug_table_present_in_default_mode() {
    let (stdout, _stderr, ok) = run_lua("print(type(debug))");
    assert!(ok, "expected success");
    k9::assert_equal!(stdout.trim(), "table");
}

/// The `debug` table is present in sandboxed mode — sandbox-safe
/// debug functions are always registered.
#[test]
fn debug_table_present_in_sandboxed_mode() {
    let (stdout, _stderr, ok) = run_lua_with("print(type(debug))", |cmd| cmd.arg("--sandboxed"));
    assert!(ok, "expected success");
    k9::assert_equal!(stdout.trim(), "table");
}

// =========================================================================
// shingetsu check
// =========================================================================

/// Run a Lua snippet via `shingetsu check`, applying `f` to configure the
/// [`Command`] before spawning.  Returns (stdout, stderr, exit_code).
/// The temp file path in stderr is replaced with `<FILE>` for stable assertions.
fn check_lua_with(
    code: &str,
    f: impl FnOnce(&mut Command) -> &mut Command,
) -> (String, String, Option<i32>) {
    let mut tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    tmp.write_all(code.as_bytes())
        .expect("failed to write temp file");
    tmp.flush().expect("failed to flush temp file");

    let path_str = tmp.path().to_str().expect("non-utf8 temp path").to_owned();

    let mut cmd = Command::new(shingetsu_bin());
    cmd.arg("check").arg(tmp.path());
    f(&mut cmd);

    let output = cmd.output().expect("failed to execute shingetsu");
    let stderr = String::from_utf8_lossy(&output.stderr)
        .into_owned()
        .replace(&path_str, "<FILE>");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr,
        output.status.code(),
    )
}

fn check_lua(code: &str) -> (String, String, Option<i32>) {
    check_lua_with(code, |cmd| cmd)
}

/// A well-typed file exits 0 with no output.
#[test]
fn check_clean_file_exits_zero() {
    let (stdout, stderr, code) = check_lua("math.abs(-5)");
    k9::assert_equal!(code, Some(0));
    k9::assert_equal!(stdout, "");
    k9::assert_equal!(stderr, "");
}

/// A type error exits 1 with error on stderr.
#[test]
fn check_type_error_exits_nonzero() {
    let (stdout, stderr, code) = check_lua("math.abs()");
    k9::assert_equal!(code, Some(1));
    k9::assert_equal!(stdout, "");
    k9::assert_equal!(
        stderr,
        "error[arg_count]: expected 1 argument but got 0
 --> <FILE>:1:9
  |
1 | math.abs()
  |         ^^ expected 1 argument but got 0
"
    );
}

/// A parse error exits 1.
#[test]
fn check_parse_error_exits_nonzero() {
    let (_stdout, stderr, code) = check_lua("local = 5");
    k9::assert_equal!(code, Some(1));
    k9::assert_equal!(
        stderr,
        "error: unexpected token `=`, expected either a variable name or `function`
 --> <FILE>:1:7
  |
1 | local = 5
  |       ^ unexpected token `=`, expected either a variable name or `function`"
    );
}

/// Warnings-only (no type errors) exits 0.
#[test]
fn check_warnings_only_exits_zero() {
    // An unused variable produces a warning but not an error.
    let (_stdout, stderr, code) = check_lua("local x = 1");
    k9::assert_equal!(code, Some(0));
    k9::assert_equal!(
        stderr,
        "warning[unused_variable]: unused variable 'x'
 --> <FILE>:1:7
  |
1 | local x = 1
  |       ^ unused variable 'x'
  |
help: prefix the name with '_' to suppress this warning: '_x'
"
    );
}

/// --sandboxed limits type info: math.abs() has no type info in
/// --libraries os (without builtins), so no error is reported.
#[test]
fn check_sandboxed_limits_type_info() {
    // With all libs (default), math.abs() is a type error.
    let (_stdout, _stderr, code) = check_lua("math.abs()");
    k9::assert_equal!(code, Some(1));

    // With only os (no builtins), math is not in the type map.
    let (_stdout, _stderr, code) =
        check_lua_with("math.abs()", |cmd| cmd.arg("--libraries").arg("os"));
    k9::assert_equal!(code, Some(0));
}

/// --sandboxed still has builtins, so math.abs() type check works.
#[test]
fn check_sandboxed_has_builtins() {
    let (_stdout, stderr, code) = check_lua_with("math.abs()", |cmd| cmd.arg("--sandboxed"));
    k9::assert_equal!(code, Some(1));
    k9::assert_equal!(
        stderr,
        "error[arg_count]: expected 1 argument but got 0
 --> <FILE>:1:9
  |
1 | math.abs()
  |         ^^ expected 1 argument but got 0
"
    );
}

/// Build a minimal `DocModel` JSON describing a single
/// `myhost.do_thing(x: number)` module function.  Used by the
/// `--types` / `shingetsu.toml [check] types` integration tests.
fn synthetic_types_json() -> String {
    use shingetsu_docgen::{DocModel, FunctionDoc, ModuleDoc, ParamDoc, TypeRef, SCHEMA_VERSION};
    let model = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "myhost".to_string(),
            doc: None,
            strict: true,
            fields: vec![],
            functions: vec![FunctionDoc {
                name: "do_thing".to_string(),
                doc: None,
                synopsis: "myhost.do_thing(x: number) -> nil".to_string(),
                params: vec![ParamDoc {
                    name: Some("x".to_string()),
                    ty: TypeRef::Number,
                    optional: false,
                    doc: None,
                }],
                variadic: None,
                variadic_doc: None,
                returns: vec![],
                is_method: false,
                examples: vec![],
                deprecated: None,
                must_use: None,
            }],
            partial: false,
        }],
        userdata_types: vec![],
        globals: vec![],
        events: vec![],
    };
    serde_json::to_string(&model).expect("serialize")
}

/// `--types <path>` merges an external `DocModel` JSON into the
/// type checker's view, so a script referencing an embedder module
/// is type-checked against the supplied data.
#[test]
fn check_types_flag_adds_module() {
    let mut types_file = tempfile::NamedTempFile::new().expect("tempfile");
    types_file
        .write_all(synthetic_types_json().as_bytes())
        .expect("write types file");
    types_file.flush().expect("flush");
    let types_path = types_file.path().to_owned();

    let (_stdout, stderr, code) = check_lua_with("myhost.do_thing()", |cmd| {
        cmd.arg("--types").arg(&types_path)
    });
    k9::assert_equal!(code, Some(1));
    k9::assert_equal!(
        stderr,
        "error[arg_count]: expected 1 argument but got 0
 --> <FILE>:1:16
  |
1 | myhost.do_thing()
  |                ^^ expected 1 argument but got 0
"
    );
}

/// Build a `DocModel` JSON describing a `Message` userdata with a
/// `set_meta(key: string, value: string)` method, plus a module
/// `kumo` whose `make_message()` function returns a `Message`.  Used
/// to exercise userdata-receiver method resolution end to end.
fn userdata_types_json() -> String {
    use shingetsu_docgen::{
        DocModel, FunctionDoc, ModuleDoc, ParamDoc, ReturnDoc, TypeRef, UserdataDoc, SCHEMA_VERSION,
    };
    let make_message = FunctionDoc {
        name: "make_message".to_string(),
        doc: None,
        synopsis: "kumo.make_message() -> Message".to_string(),
        params: vec![],
        variadic: None,
        variadic_doc: None,
        returns: vec![ReturnDoc {
            ty: TypeRef::Named {
                name: "Message".to_string(),
            },
            doc: None,
        }],
        is_method: false,
        examples: vec![],
        deprecated: None,
        must_use: None,
    };
    let set_meta = FunctionDoc {
        name: "set_meta".to_string(),
        doc: None,
        synopsis: "Message:set_meta(key: string, value: string) -> nil".to_string(),
        params: vec![
            ParamDoc {
                name: Some("key".to_string()),
                ty: TypeRef::String,
                optional: false,
                doc: None,
            },
            ParamDoc {
                name: Some("value".to_string()),
                ty: TypeRef::String,
                optional: false,
                doc: None,
            },
        ],
        variadic: None,
        variadic_doc: None,
        returns: vec![],
        is_method: true,
        examples: vec![],
        deprecated: None,
        must_use: None,
    };
    let model = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "kumo".to_string(),
            doc: None,
            strict: true,
            fields: vec![],
            functions: vec![make_message],
            partial: false,
        }],
        userdata_types: vec![UserdataDoc {
            name: "Message".to_string(),
            doc: None,
            fields: vec![],
            methods: vec![set_meta],
            metamethods: vec![],
            partial: false,
        }],
        globals: vec![],
        events: vec![],
    };
    serde_json::to_string(&model).expect("serialize")
}

/// `--types` data describing a userdata type drives method-call
/// type-checking on a `LuaType::Named` receiver.  Calling
/// `msg:set_meta("x")` (one argument short) must produce an
/// `arg_count` error against the userdata's declared signature.
#[test]
fn check_userdata_method_arg_count() {
    let mut types_file = tempfile::NamedTempFile::new().expect("tempfile");
    types_file
        .write_all(userdata_types_json().as_bytes())
        .expect("write types file");
    types_file.flush().expect("flush");
    let types_path = types_file.path().to_owned();

    let (_stdout, stderr, code) = check_lua_with(
        "local msg = kumo.make_message()\nmsg:set_meta(\"x\")",
        |cmd| cmd.arg("--types").arg(&types_path),
    );
    k9::assert_equal!(code, Some(1));
    k9::assert_equal!(
        stderr,
        "error[arg_count]: expected 2 arguments but got 1
 --> <FILE>:2:13
  |
2 | msg:set_meta(\"x\")
  |             ^^^^^ expected 2 arguments but got 1
"
    );
}

/// Two `--types` files contributing to the same module work when one
/// is marked `partial = true`: the partial side's functions fold
/// into the canonical declaration.  Calling a function declared only
/// in the partial side produces a normal arity diagnostic.
#[test]
fn check_types_partial_merges_modules() {
    use shingetsu_docgen::{DocModel, FunctionDoc, ModuleDoc, ParamDoc, TypeRef, SCHEMA_VERSION};

    let canonical = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "kumo".to_string(),
            doc: None,
            strict: true,
            fields: vec![],
            functions: vec![FunctionDoc {
                name: "core_func".to_string(),
                doc: None,
                synopsis: "kumo.core_func()".to_string(),
                params: vec![],
                variadic: None,
                variadic_doc: None,
                returns: vec![],
                is_method: false,
                examples: vec![],
                deprecated: None,
                must_use: None,
            }],
            partial: false,
        }],
        userdata_types: vec![],
        globals: vec![],
        events: vec![],
    };
    let helpers = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "kumo".to_string(),
            doc: None,
            strict: true,
            fields: vec![],
            functions: vec![FunctionDoc {
                name: "helper".to_string(),
                doc: None,
                synopsis: "kumo.helper(x: number)".to_string(),
                params: vec![ParamDoc {
                    name: Some("x".to_string()),
                    ty: TypeRef::Number,
                    optional: false,
                    doc: None,
                }],
                variadic: None,
                variadic_doc: None,
                returns: vec![],
                is_method: false,
                examples: vec![],
                deprecated: None,
                must_use: None,
            }],
            partial: true,
        }],
        userdata_types: vec![],
        globals: vec![],
        events: vec![],
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let canonical_path = dir.path().join("canonical.json");
    let helpers_path = dir.path().join("helpers.json");
    std::fs::write(&canonical_path, serde_json::to_string(&canonical).unwrap()).unwrap();
    std::fs::write(&helpers_path, serde_json::to_string(&helpers).unwrap()).unwrap();

    let (_stdout, stderr, code) = check_lua_with("kumo.helper()", |cmd| {
        cmd.arg("--types")
            .arg(&canonical_path)
            .arg("--types")
            .arg(&helpers_path)
    });
    k9::assert_equal!(code, Some(1));
    k9::assert_equal!(
        stderr,
        "error[arg_count]: expected 1 argument but got 0
 --> <FILE>:1:12
  |
1 | kumo.helper()
  |            ^^ expected 1 argument but got 0
"
    );
}

/// Two `--types` files declaring the same module name without
/// `partial = true` produces a merge error before the type checker
/// runs.
#[test]
fn check_types_duplicate_module_errors() {
    use shingetsu_docgen::{DocModel, ModuleDoc, SCHEMA_VERSION};
    let mk = |functions| DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "kumo".to_string(),
            doc: None,
            strict: true,
            fields: vec![],
            functions,
            partial: false,
        }],
        userdata_types: vec![],
        globals: vec![],
        events: vec![],
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let a_path = dir.path().join("a.json");
    let b_path = dir.path().join("b.json");
    std::fs::write(&a_path, serde_json::to_string(&mk(vec![])).unwrap()).unwrap();
    std::fs::write(&b_path, serde_json::to_string(&mk(vec![])).unwrap()).unwrap();

    let (_stdout, stderr, code) = check_lua_with("return 1", |cmd| {
        cmd.arg("--types").arg(&a_path).arg("--types").arg(&b_path)
    });
    k9::assert_equal!(code, Some(1));
    k9::assert_equal!(
        stderr,
        "Error: merging --types data: duplicate module 'kumo': set `partial = true` on the additive side to merge\n"
    );
}

/// `[check] types = [...]` in `shingetsu.toml` is picked up by
/// `shingetsu check`, with paths resolved relative to the config
/// file's directory.
#[test]
fn check_project_config_types() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("types.json"), synthetic_types_json())
        .expect("write types.json");
    std::fs::write(
        dir.path().join("shingetsu.toml"),
        "[check]\ntypes = [\"types.json\"]\n",
    )
    .expect("write shingetsu.toml");
    let script_path = dir.path().join("script.lua");
    std::fs::write(&script_path, "myhost.do_thing()").expect("write script");

    let output = Command::new(shingetsu_bin())
        .arg("check")
        .arg(&script_path)
        .output()
        .expect("failed to execute shingetsu");
    let stderr = String::from_utf8_lossy(&output.stderr)
        .into_owned()
        .replace(script_path.to_str().expect("non-utf8 path"), "<FILE>");
    k9::assert_equal!(output.status.code(), Some(1));
    k9::assert_equal!(
        stderr,
        "error[arg_count]: expected 1 argument but got 0
 --> <FILE>:1:16
  |
1 | myhost.do_thing()
  |                ^^ expected 1 argument but got 0
"
    );
}

// ---------------------------------------------------------------------------
// `shingetsu doc` subcommands
// ---------------------------------------------------------------------------

/// `shingetsu doc dump-json` produces a `DocModel` whose top-level
/// module list reflects the libraries registered.  Asserts the full
/// (deterministic) module-name list for the default library set.
#[test]
fn doc_dump_json_emits_doc_model() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_path = tmp.path().join("docs.json");
    let status = Command::new(shingetsu_bin())
        .arg("doc")
        .arg("dump-json")
        .arg("--out")
        .arg(&out_path)
        .status()
        .expect("spawn");
    k9::assert_equal!(status.success(), true);
    let json = std::fs::read_to_string(&out_path).expect("read out");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse json");
    let module_names: Vec<&str> = parsed["modules"]
        .as_array()
        .expect("modules array")
        .iter()
        .map(|m| m["name"].as_str().expect("name"))
        .collect();
    k9::assert_equal!(
        module_names,
        vec![
            "bit32", "builtins", "debug", "io", "math", "os", "regex", "string", "table", "task",
            "utf8",
        ]
    );
    k9::assert_equal!(parsed["schema_version"], serde_json::json!(11));
}

/// `shingetsu doc render-luau` produces a `.d.luau` definition file
/// covering every registered module.  Asserts the full set of module
/// declarations emitted (one `declare <name>: { ... }` block per
/// module).
#[test]
fn doc_render_luau_declares_every_module() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_path = tmp.path().join("defs.d.luau");
    let status = Command::new(shingetsu_bin())
        .arg("doc")
        .arg("render-luau")
        .arg("--out")
        .arg(&out_path)
        .status()
        .expect("spawn");
    k9::assert_equal!(status.success(), true);
    let text = std::fs::read_to_string(&out_path).expect("read out");
    let declared: Vec<&str> = text
        .lines()
        .filter_map(|l| l.strip_prefix("declare "))
        .filter_map(|l| l.split_once(':').map(|(name, _)| name.trim()))
        .collect();
    k9::assert_equal!(
        declared,
        vec![
            "bit32", "builtins", "debug", "io", "math", "os", "regex", "string", "table", "task",
            "utf8",
        ]
    );
}

/// `shingetsu doc extract-lua` produces a JSON `DocModel` from
/// Lua source files, then `shingetsu check --types` consumes that
/// JSON to type-check a caller script.  This is the round-trip
/// kumomta uses to ship documented Lua helpers.
#[test]
fn doc_extract_lua_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lib_dir = dir.path().join("lib");
    std::fs::create_dir_all(&lib_dir).expect("mkdir");
    std::fs::write(
        lib_dir.join("helper.lua"),
        "\
local mod = {}

--- Configure the queue.
--- @param path string  TOML file path
function mod.configure(path) return path end

return mod
",
    )
    .expect("write helper.lua");

    let types_path = dir.path().join("types.json");
    let status = Command::new(shingetsu_bin())
        .arg("doc")
        .arg("extract-lua")
        .arg("--root")
        .arg(&lib_dir)
        .arg("--out")
        .arg(&types_path)
        .arg(lib_dir.join("helper.lua"))
        .status()
        .expect("spawn extract-lua");
    k9::assert_equal!(status.success(), true);

    let script = dir.path().join("script.lua");
    std::fs::write(&script, "helper.configure()").expect("write script");
    let output = Command::new(shingetsu_bin())
        .arg("check")
        .arg("--types")
        .arg(&types_path)
        .arg(&script)
        .output()
        .expect("spawn check");
    let stderr = String::from_utf8_lossy(&output.stderr)
        .into_owned()
        .replace(script.to_str().expect("non-utf8"), "<FILE>");
    k9::assert_equal!(output.status.code(), Some(1));
    k9::assert_equal!(
        stderr,
        "error[arg_count]: expected 1 argument but got 0
 --> <FILE>:1:17
  |
1 | helper.configure()
  |                 ^^ expected 1 argument but got 0
"
    );
}

/// `shingetsu doc render-markdown` consumes a JSON export and writes
/// the markdown subtree to `--out`.  Use a small synthetic DocModel
/// so the test doesn't depend on stdlib registration state.
#[test]
fn doc_render_markdown_writes_pages() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let input = tmp.path().join("docs.json");
    let out_dir = tmp.path().join("site");
    let synthetic = r#"{
  "schema_version": 1,
  "modules": [
    {
      "name": "util",
      "doc": "A small utility module.",
      "strict": false,
      "fields": [],
      "functions": []
    }
  ],
  "userdata_types": [],
  "globals": []
}
"#;
    std::fs::write(&input, synthetic).expect("write input");
    let status = Command::new(shingetsu_bin())
        .arg("doc")
        .arg("render-markdown")
        .arg("--input")
        .arg(&input)
        .arg("--out")
        .arg(&out_dir)
        .status()
        .expect("spawn");
    k9::assert_equal!(status.success(), true);
    let index = std::fs::read_to_string(out_dir.join("index.md")).expect("read index");
    k9::assert_equal!(
        index,
        "# Reference\n\n## Modules\n\n- [`util`](modules/util/index.md) \u{2014} A small utility module.\n\n"
    );
    let module_page =
        std::fs::read_to_string(out_dir.join("modules/util/index.md")).expect("read module page");
    k9::assert_equal!(module_page, "# util\n\nA small utility module.\n\n");
}

/// `shingetsu doc render-markdown --input a.json --input b.json`
/// merges the two models (one declaring `kumo`, one declaring an
/// additional userdata type) and renders a single subtree containing
/// both.
#[test]
fn doc_render_markdown_merges_inputs() {
    use shingetsu_docgen::{DocModel, ModuleDoc, UserdataDoc, SCHEMA_VERSION};
    let a = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "kumo".to_string(),
            doc: Some("core".to_string()),
            strict: true,
            fields: vec![],
            functions: vec![],
            partial: false,
        }],
        userdata_types: vec![],
        globals: vec![],
        events: vec![],
    };
    let b = DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![],
        userdata_types: vec![UserdataDoc {
            name: "Message".to_string(),
            doc: Some("a message".to_string()),
            fields: vec![],
            methods: vec![],
            metamethods: vec![],
            partial: false,
        }],
        globals: vec![],
        events: vec![],
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let a_path = tmp.path().join("a.json");
    let b_path = tmp.path().join("b.json");
    let out_dir = tmp.path().join("site");
    std::fs::write(&a_path, serde_json::to_string(&a).unwrap()).unwrap();
    std::fs::write(&b_path, serde_json::to_string(&b).unwrap()).unwrap();

    let status = Command::new(shingetsu_bin())
        .arg("doc")
        .arg("render-markdown")
        .arg("--input")
        .arg(&a_path)
        .arg("--input")
        .arg(&b_path)
        .arg("--out")
        .arg(&out_dir)
        .status()
        .expect("spawn");
    k9::assert_equal!(status.success(), true);
    let index = std::fs::read_to_string(out_dir.join("index.md")).expect("read index");
    k9::assert_equal!(
        index,
        "# Reference\n\n## Modules\n\n- [`kumo`](modules/kumo/index.md) \u{2014} core\n\n## Types\n\n- [`Message`](types/Message/index.md) \u{2014} a message\n\n"
    );
}

// ---------------------------------------------------------------------------

/// Multiple type errors are all reported.
#[test]
fn check_multiple_errors_reported() {
    let (_stdout, stderr, code) = check_lua(
        "\
math.abs()
math.floor()",
    );
    k9::assert_equal!(code, Some(1));
    k9::assert_equal!(
        stderr,
        "error[arg_count]: expected 1 argument but got 0
 --> <FILE>:1:9
  |
1 | math.abs()
  |         ^^ expected 1 argument but got 0
error[arg_count]: expected 1 argument but got 0
 --> <FILE>:2:11
  |
2 | math.floor()
  |           ^^ expected 1 argument but got 0
"
    );
}
