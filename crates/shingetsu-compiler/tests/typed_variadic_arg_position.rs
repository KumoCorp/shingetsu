//! Verifies that the proc-macro correctly positions per-element
//! errors from `TypedVariadic<T>` arguments when the function has
//! fixed parameters preceding the variadic.

mod common;

use common::{new_env, run_err_with_env};
use shingetsu::{module, GlobalEnv, TypedVariadic};

#[module(name = "fixture")]
mod fixture {
    use super::*;

    /// Sums the variadic ints; `tag` and `flag` are fixed and just
    /// thread through so the error path can be exercised with two
    /// fixed params before the variadic.
    #[function(variadic)]
    fn sum_with_prefix(_tag: String, _flag: bool, args: TypedVariadic<i64>) -> i64 {
        args.0.iter().sum()
    }
}

fn env_with_fixture() -> GlobalEnv {
    let env = new_env();
    fixture::register_global_module(&env).expect("register fixture");
    env
}

async fn run_err(src: &str) -> String {
    run_err_with_env(env_with_fixture(), src).await
}

#[tokio::test]
async fn first_variadic_element_reports_position_3() {
    // tag=1, flag=2, then variadic starts at position 3.
    k9::assert_equal!(
        run_err("fixture.sum_with_prefix('tag', true, 'oops')").await,
        "\
error: bad argument #3 to 'sum_with_prefix' (number expected, got string)
 --> test.lua:1:38
  |
1 | fixture.sum_with_prefix('tag', true, 'oops')
  |                                      ^^^^^^ bad argument #3 to 'sum_with_prefix' (number expected, got string)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn third_variadic_element_reports_position_5() {
    // tag=1, flag=2, then variadic[0]=3, variadic[1]=4, variadic[2]=5.
    k9::assert_equal!(
        run_err("fixture.sum_with_prefix('tag', true, 1, 2, 'oops')").await,
        "\
error: bad argument #5 to 'sum_with_prefix' (number expected, got string)
 --> test.lua:1:44
  |
1 | fixture.sum_with_prefix('tag', true, 1, 2, 'oops')
  |                                            ^^^^^^ bad argument #5 to 'sum_with_prefix' (number expected, got string)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}

#[tokio::test]
async fn fixed_arg_error_still_reports_correct_position() {
    // The fixture takes a String first; passing a number must report
    // position #1, not be affected by the variadic offset logic.
    k9::assert_equal!(
        run_err("fixture.sum_with_prefix(42, true, 1)").await,
        "\
error: bad argument #1 to 'sum_with_prefix' (string expected, got number)
 --> test.lua:1:25
  |
1 | fixture.sum_with_prefix(42, true, 1)
  |                         ^^ bad argument #1 to 'sum_with_prefix' (string expected, got number)
stack traceback:
\ttest.lua:1: in main chunk"
    );
}
