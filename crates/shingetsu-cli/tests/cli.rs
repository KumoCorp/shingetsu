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
