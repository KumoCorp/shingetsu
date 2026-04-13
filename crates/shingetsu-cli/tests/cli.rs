use std::io::Write;
use std::process::Command;

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
    let mut tmp = tempfile::NamedTempFile::new().expect("failed to create temp file");
    tmp.write_all(code.as_bytes())
        .expect("failed to write temp file");
    tmp.flush().expect("failed to flush temp file");

    let output = Command::new(shingetsu_bin())
        .arg("run")
        .arg(tmp.path())
        .output()
        .expect("failed to execute shingetsu");

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
    assert_eq!(stdout.trim(), "hello world");
}

#[test]
fn cli_print_multiple_args() {
    let (stdout, stderr, ok) = run_lua("print(1, 'two', true)");
    assert!(ok, "shingetsu exited with error: {stderr}");
    assert_eq!(stdout.trim(), "1\ttwo\ttrue");
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
    assert_eq!(stdout.trim(), "custom");
}

#[test]
fn cli_print_multiple_lines() {
    let (stdout, stderr, ok) = run_lua(
        "\
print('line1')
print('line2')",
    );
    assert!(ok, "shingetsu exited with error: {stderr}");
    assert_eq!(stdout.trim(), "line1\nline2");
}
