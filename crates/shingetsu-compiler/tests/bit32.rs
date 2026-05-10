//! Error-path coverage for the `bit32` standard library.
//!
//! The success paths are exercised through doc-test examples;
//! this file covers the validation and coercion-rejection branches
//! that can't be expressed as a fenced `lua` block (since they raise
//! a runtime error rather than producing a value).

mod common;

#[tokio::test]
async fn band_rejects_string() {
    k9::assert_equal!(
        common::run_err("bit32.band('hello', 1)").await,
        "\
error: bad argument #0 to 'band' (number expected, got string)
 --> test.lua:1:1
  |
1 | bit32.band('hello', 1)
  | ^^^^^^^^^^ bad argument #0 to 'band' (number expected, got string)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn bnot_rejects_string() {
    k9::assert_equal!(
        common::run_err("bit32.bnot('hello')").await,
        "\
error: bad argument #1 to 'bnot' (number expected, got string)
 --> test.lua:1:12
  |
1 | bit32.bnot('hello')
  |            ^^^^^^^ bad argument #1 to 'bnot' (number expected, got string)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn band_rejects_nan() {
    k9::assert_equal!(
        common::run_err("bit32.band(0/0, 1)").await,
        "\
error: bad argument #0 to 'band' (number has no integer representation)
 --> test.lua:1:1
  |
1 | bit32.band(0/0, 1)
  | ^^^^^^^^^^ bad argument #0 to 'band' (number has no integer representation)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn bnot_rejects_infinity() {
    k9::assert_equal!(
        common::run_err("bit32.bnot(math.huge)").await,
        "\
error: bad argument #1 to 'bnot' (number has no integer representation)
 --> test.lua:1:12
  |
1 | bit32.bnot(math.huge)
  |            ^^^^^^^^^ bad argument #1 to 'bnot' (number has no integer representation)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn extract_rejects_negative_field() {
    k9::assert_equal!(
        common::run_err("bit32.extract(0xFF, -1, 4)").await,
        "\
error: bad argument #2 to 'extract' (field cannot be negative)
 --> test.lua:1:21
  |
1 | bit32.extract(0xFF, -1, 4)
  |                     ^^ bad argument #2 to 'extract' (field cannot be negative)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn extract_rejects_zero_width() {
    k9::assert_equal!(
        common::run_err("bit32.extract(0xFF, 0, 0)").await,
        "\
error: bad argument #3 to 'extract' (width must be positive)
 --> test.lua:1:24
  |
1 | bit32.extract(0xFF, 0, 0)
  |                        ^ bad argument #3 to 'extract' (width must be positive)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn extract_rejects_overflow() {
    k9::assert_equal!(
        common::run_err("bit32.extract(0xFF, 16, 17)").await,
        "\
error: bad argument #2 to 'extract' (trying to access non-existent bits)
 --> test.lua:1:21
  |
1 | bit32.extract(0xFF, 16, 17)
  |                     ^^ bad argument #2 to 'extract' (trying to access non-existent bits)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn replace_rejects_negative_field() {
    k9::assert_equal!(
        common::run_err("bit32.replace(0, 0xF, -1, 4)").await,
        "\
error: bad argument #3 to 'replace' (field cannot be negative)
 --> test.lua:1:23
  |
1 | bit32.replace(0, 0xF, -1, 4)
  |                       ^^ bad argument #3 to 'replace' (field cannot be negative)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn replace_rejects_zero_width() {
    k9::assert_equal!(
        common::run_err("bit32.replace(0, 0xF, 0, 0)").await,
        "\
error: bad argument #4 to 'replace' (width must be positive)
 --> test.lua:1:26
  |
1 | bit32.replace(0, 0xF, 0, 0)
  |                          ^ bad argument #4 to 'replace' (width must be positive)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn replace_rejects_overflow() {
    k9::assert_equal!(
        common::run_err("bit32.replace(0, 0xF, 16, 17)").await,
        "\
error: bad argument #3 to 'replace' (trying to access non-existent bits)
 --> test.lua:1:23
  |
1 | bit32.replace(0, 0xF, 16, 17)
  |                       ^^ bad argument #3 to 'replace' (trying to access non-existent bits)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}
