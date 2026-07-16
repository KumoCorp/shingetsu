//! Error-path coverage for the `shlex` standard library.
//!
//! The success paths (splitting, quoting, joining, and the malformed
//! `split` that returns `nil` plus a message) are exercised through
//! doc-test examples; this file covers the nul-byte rejection in
//! `shlex.quote` and `shlex.join`, which raise a runtime error rather
//! than producing a value.

mod common;

#[tokio::test]
async fn quote_rejects_nul_byte() {
    common::assert_runtime_error!(
        "shlex.quote('a\\0b')",
        "\
error: bad argument #1 to 'shlex.quote' (a string without a nul byte expected, got a nul byte)
 --> test.lua:1:13
  |
1 | shlex.quote('a\\0b')
  |             ^^^^^^ bad argument #1 to 'shlex.quote' (a string without a nul byte expected, got a nul byte)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}

#[tokio::test]
async fn join_rejects_nul_byte() {
    common::assert_runtime_error!(
        "shlex.join({'ok', 'a\\0b'})",
        "\
error: bad argument #1 to 'shlex.join' (a string without a nul byte expected, got a nul byte in element 2)
 --> test.lua:1:12
  |
1 | shlex.join({'ok', 'a\\0b'})
  |            ^^^^^^^^^^^^^^ bad argument #1 to 'shlex.join' (a string without a nul byte expected, got a nul byte in element 2)
stack traceback:
\ttest.lua:1: in main chunk",
    );
}
