//! Integration tests that validate the emitted `.d.luau` against
//! `luau-lsp analyze`.
//!
//! These tests are skipped when `luau-lsp` is not on `PATH`.  Set the
//! `TEST_LUAU` environment variable to override the binary path; set
//! it to `0` to force-skip.  The probing pattern mirrors
//! `mlua-extras`'s test suite so behaviour is consistent across
//! environments.

use std::sync::LazyLock;

use shingetsu::{module, userdata};
use shingetsu_docgen::{extract, render_luau};
use shingetsu_vm::GlobalEnv;

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// A small math module used by the validation tests.
#[module(name = "smallmath")]
#[allow(dead_code)]
mod smallmath_impl {
    /// Format-time version string.
    #[field]
    fn version() -> String {
        "1.0".to_owned()
    }

    /// Return the larger of two numbers.
    #[function]
    fn max(a: f64, b: f64) -> f64 {
        if a > b {
            a
        } else {
            b
        }
    }

    /// Return the absolute value.
    #[function]
    fn abs(x: f64) -> f64 {
        x.abs()
    }
}

/// A counter exposed as userdata.
struct Counter(#[allow(dead_code)] i64);

/// A counter exposed as userdata.
#[userdata]
impl Counter {
    /// The current count.
    #[lua_field]
    fn value(&self) -> i64 {
        self.0
    }

    /// Add `amount` to the counter and return the new value.
    #[lua_method]
    fn increment(&self, amount: i64) -> i64 {
        self.0 + amount
    }
}

fn build_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    smallmath_impl::register_preload(&env);
    env.register_userdata_type(Counter::userdata_type());
    env
}

// ---------------------------------------------------------------------------
// luau-lsp probe + invocation
// ---------------------------------------------------------------------------

/// Resolve the `luau-lsp` binary path.  Returns `None` when not
/// installed or `TEST_LUAU=0` is set.
fn find_luau_lsp() -> Option<String> {
    const LUAU_LSP: &str = "luau-lsp";

    if let Ok(path) = std::env::var("TEST_LUAU") {
        return (path != "0").then_some(path);
    }

    static PROBED: LazyLock<bool> = LazyLock::new(|| {
        std::process::Command::new(LUAU_LSP)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    });
    PROBED.then(|| LUAU_LSP.to_string())
}

/// Write `defs_content` and `script` to a tempdir and run
/// `luau-lsp analyze --defs=@shingetsu=defs.d.luau script.luau`.
/// Asserts that the only stderr output is `[INFO]` lines, matching
/// the upstream mlua-extras test convention.  Returns silently
/// without running anything when `luau-lsp` is unavailable.
fn validate_with_luau_lsp(defs_content: &str, script: &str) {
    let Some(luau_lsp) = find_luau_lsp() else {
        return;
    };

    let dir = tempfile::TempDir::new().expect("tempdir");
    let defs_path = dir.path().join("defs.d.luau");
    std::fs::write(&defs_path, defs_content).unwrap();
    let script_path = dir.path().join("test.luau");
    std::fs::write(&script_path, script).unwrap();

    let output = std::process::Command::new(&luau_lsp)
        .arg("analyze")
        .arg(format!("--defs=@shingetsu={}", defs_path.display()))
        .arg(&script_path)
        .output()
        .unwrap_or_else(|e| panic!("failed to run luau-lsp ({luau_lsp}): {e}"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    let errors: Vec<&str> = stderr
        .lines()
        .filter(|l| !l.starts_with("[INFO]"))
        .collect();

    assert!(
        errors.is_empty(),
        "luau-lsp reported errors:\n{}\n\n--- generated definitions ---\n{}\n--- script ---\n{}",
        errors.join("\n"),
        defs_content,
        script,
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn luau_lsp_accepts_module_definitions() {
    let defs = render_luau(&extract(&build_env()));
    validate_with_luau_lsp(
        &defs,
        r#"
local _v: string = smallmath.version
local _m: number = smallmath.max(1.0, 2.0)
local _a: number = smallmath.abs(-3.0)
"#,
    );
}

#[test]
fn luau_lsp_accepts_userdata_class_methods() {
    let defs = render_luau(&extract(&build_env()));
    // Add a `declare counter: Counter` line so the script has a
    // value to call methods on.  The emitter doesn't currently
    // declare globals for registered userdata types — that's a
    // deliberate gap (the values are produced by stdlib functions,
    // not held as named globals).
    let defs = format!("{defs}\ndeclare counter: Counter\n");
    validate_with_luau_lsp(
        &defs,
        r#"
local _v: number = counter.value
local _i: number = counter:increment(5)
"#,
    );
}

#[test]
fn luau_lsp_rejects_wrong_argument_type() {
    // Negative case: passing a string where a number is expected
    // should produce an error.  This confirms luau-lsp is actually
    // type-checking against the emitted definitions, rather than
    // silently accepting everything.
    let Some(luau_lsp) = find_luau_lsp() else {
        return;
    };

    let defs = render_luau(&extract(&build_env()));
    let dir = tempfile::TempDir::new().expect("tempdir");
    let defs_path = dir.path().join("defs.d.luau");
    std::fs::write(&defs_path, &defs).unwrap();
    let script_path = dir.path().join("test.luau");
    std::fs::write(
        &script_path,
        r#"local _ = smallmath.max("not a number", 2.0)"#,
    )
    .unwrap();

    let output = std::process::Command::new(&luau_lsp)
        .arg("analyze")
        .arg(format!("--defs=@shingetsu={}", defs_path.display()))
        .arg(&script_path)
        .output()
        .expect("run luau-lsp");

    assert!(
        !output.status.success(),
        "luau-lsp should have rejected the wrong-type call; output:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn luau_lsp_accepts_real_stdlib_definitions() {
    // End-to-end smoke: render the actual shingetsu stdlib's .d.luau
    // and confirm luau-lsp accepts it without errors.  Catches
    // regressions in any of the macro-derived module shapes.
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register_libs");
    let defs = render_luau(&extract(&env));
    validate_with_luau_lsp(
        &defs,
        r#"
local _f: number = math.floor(1.5)
local _n: number = math.random()
local _s: string = string.upper("hi")
local _t: any = table.pack(1, 2, 3)
"#,
    );
}
