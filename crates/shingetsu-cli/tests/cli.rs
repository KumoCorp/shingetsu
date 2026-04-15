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

/// --sandboxed --os enables only the os library.
#[test]
fn sandboxed_with_os() {
    let (stdout, stderr, ok) = run_lua_with("print(type(os), type(io))", |cmd| {
        cmd.arg("--sandboxed").arg("--os")
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "table\tnil");
}

/// --sandboxed --io enables file I/O but not stdio handles.
#[test]
fn sandboxed_with_io_no_stdio() {
    let (stdout, stderr, ok) = run_lua_with("print(type(io), io.stdin)", |cmd| {
        cmd.arg("--sandboxed").arg("--io")
    });
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "table\tnil");
}

/// --sandboxed --stdio enables stdio (and implicitly io).
#[test]
fn sandboxed_with_stdio() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"
io.write("from stdio")
"#,
        |cmd| cmd.arg("--sandboxed").arg("--stdio"),
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout, "from stdio");
}

/// --sandboxed --io --os --stdio enables everything.
#[test]
fn sandboxed_with_all_flags() {
    let (stdout, stderr, ok) = run_lua_with("print(type(io), type(os), io.stdin ~= nil)", |cmd| {
        cmd.arg("--sandboxed")
            .arg("--io")
            .arg("--os")
            .arg("--stdio")
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
    let (stdout, stderr, ok) =
        run_lua_with("print(io.popen)", |cmd| cmd.arg("--sandboxed").arg("--io"));
    assert!(ok, "shingetsu exited with error: {stderr}");
    k9::assert_equal!(stdout.trim(), "nil");
}

/// io.popen is available with --sandboxed --exec.
#[test]
fn popen_with_exec_flag() {
    let (stdout, stderr, ok) = run_lua_with(
        r#"
local f = io.popen("echo sandbox_exec")
io.write(f:read("*a"))
f:close()
"#,
        |cmd| cmd.arg("--sandboxed").arg("--exec").arg("--stdio"),
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
local ok, err = pcall(function()
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
