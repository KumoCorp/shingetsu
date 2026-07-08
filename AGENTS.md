## Project Overview

shingetsu is a lua VM implementation that supports a blend of lua 5.5 and luau
syntax.  It is designed for embedding in highly concurrent async applications.

## Build Commands

```bash
make build        # Build all binaries (debug)
make check        # Quick syntax check (cargo check)
make clippy       # Check lints (cargo clippy)
make test         # Full test suite (cargo nextest run)
make fmt          # Format all code (cargo +nightly fmt)
make bench        # Run benchmarks (cargo bench)
```

 - to rustfmt directly use `cargo +nightly fmt`
 - to run tests directly use `cargo nextest run` as it is faster than cargo test.

## Code Structure

 - Rust crates are found in the `crates` directory, and are linked into the workspace members list.
 - Cargo.toml workspace.members must be kept in alphabetical order
 - workspace dependencies are used throughout.
 - Cargo.toml dependencies are always kept in alphabetical order
 - Prefer to use the `anyhow.workspace = true` form when adding a dependency to a crate

## Coding Conventions

 - Always preserve existing comments when modifying code; both doc comments and inline comments.
 - Except in tests, Avoid `.unwrap()` and `panic!`.
   Prefer to propagate errors using the `?` operator when
   in a function that returns a `Result`. If a panic is unavoidable, use
   `.expect("REASON WHY")` instead of a bare `.unwrap()`.
 - In tests, always assert on the full rendered output a human would see --
   never examine a substring or a single field in isolation.  Follow this
   priority order when choosing how to assert:
   1. Use an existing macro from `crates/shingetsu-compiler/tests/common.rs`
      (e.g. `assert_runtime_error!`, `assert_plugin_diagnostics!`,
      `type_check`, `compile_err`) when one covers the scenario.
   2. Use `assert_multi_line_output!(actual, expected, "description")` from
      `common.rs` for any multi-line string comparison not covered above.
   3. Use `k9::assert_equal!` for non-string value comparisons
      (e.g. `Vec<Value>`, integers, booleans).
   4. Before writing a new bespoke assertion helper, stop and ask for guidance.
 - Do not use `str.contains("something")` in a test, or other similar "keyhole"
   result examination.  Examine the full value.
 - If a test has unstable/variable output (eg: includes temporary file paths), preprocess the string to replace
   the known temporary file path with a constant string like TMPDIR before asserting.
 - When adding `use` imports, they should all be placed in a block at the top
   of the file, or if in a `mod`, inside the top of the mod.  Make sure that
   you use the code formatting instructions to keep the import section
   organized.

## Submitting or Preparing Pull Requests

 - Add the suffix `(beep boop) 🤖` to the end of the pull request title
