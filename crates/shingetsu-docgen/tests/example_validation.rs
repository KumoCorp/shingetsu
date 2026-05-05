//! Compile and run every fenced ` ```lua ` block in the standard
//! library's documentation, asserting that each example executes
//! without error.
//!
//! When an example fails the test reports the source location
//! (e.g. "math.floor (example 2 of 3)") and the rendered diagnostic.
//! Examples whose fence carries the `no_run` flag and non-`lua`
//! fences are skipped automatically.

use shingetsu::{GlobalEnv, Libraries};
use shingetsu_docgen::{extract, populate_example_outputs};

#[tokio::test]
async fn every_documentation_example_executes() {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::ALL).expect("register_libs");
    let mut model = extract(&env);

    let failures = populate_example_outputs(&mut model).await;

    if !failures.is_empty() {
        let mut buf = String::new();
        for f in &failures {
            buf.push_str(&format!(
                "--- {} (example {} of {}) ---\nsource:\n{}\n\nerror:\n{}\n\n",
                f.path,
                f.index + 1,
                f.total,
                indent(&f.code, "  "),
                f.diagnostic,
            ));
        }
        panic!(
            "{} documentation examples failed:\n\n{}",
            failures.len(),
            buf
        );
    }
}

fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
