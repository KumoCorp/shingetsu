//! Runtime behaviour of `TypedCallable`: a Lua function captured from
//! policy, called from Rust against a declared `fn(A) -> R` shape.
//!
//! Compilation lives in this crate, so a real Lua function with a
//! `return` is available -- needed to exercise the return-site anchoring
//! that recasts a decode failure as a `ReturnValueMismatch` pointing at
//! the handler's `return`.

mod common;

use common::{assert_multi_line_output, compile_diagnostics_with_env, new_env, run_in_env};
use shingetsu::diagnostic::{render_runtime_error, RenderStyle};
use shingetsu::types::{FunctionLuaType, LuaType, TypedParam};
use shingetsu::{declare_callable, GlobalEnv, LuaTyped, TypedCallable, Value};

declare_callable! {
    /// Decide whether to accept a connection.
    pub type AcceptConn = fn(
        /// the candidate domain
        domain: String,
        /// the connecting port
        port: i64,
    ) -> bool;
}

/// Compile `src` (which must evaluate to a single function) and capture
/// it as a `TypedCallable<A, R>` against `env`.
async fn capture<A, R>(env: &GlobalEnv, src: &str) -> TypedCallable<A, R> {
    let vv = run_in_env(env, src).await.expect("compile+run");
    let func = match vv.into_iter().next() {
        Some(Value::Function(f)) => f,
        other => panic!("expected a function, got {other:?}"),
    };
    TypedCallable::new(env.clone(), func)
}

#[tokio::test]
async fn named_callable_type_detects_transposed_params() {
    // A bare TypedCallable exposes no parameter names, so the checker
    // cannot tell 'domain'/'port' apart.  The named declare_callable!
    // type attaches them, so a handler that swaps the two is flagged.
    let env = new_env();
    let apply = LuaType::Function(Box::new(FunctionLuaType {
        type_params: vec![],
        params: vec![TypedParam::new(
            Some("cb"),
            <AcceptConn as LuaTyped>::lua_type(),
        )],
        variadic: None,
        returns: vec![],
        is_method: false,
        inferred_unannotated: false,
        deprecated: None,
        must_use: None,
    }));
    env.register_global_type("apply", apply);
    let diags = compile_diagnostics_with_env(
        &env,
        "apply(function(port, domain) return domain ~= '' and port > 0 end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[callback_param_transposition]: callback 'cb' handler parameter names look transposed relative to the registered signature: position 0 is named 'port' but signature names that position 'domain'; position 1 is named 'domain' but signature names that position 'port'
 --> test.lua:1:15
  |
1 | apply(function(port, domain) return domain ~= '' and port > 0 end)
  |               ^^^^^^^^^^^^^^ callback 'cb' handler parameter names look transposed relative to the registered signature: position 0 is named 'port' but signature names that position 'domain'; position 1 is named 'domain' but signature names that position 'port'
  |
help: signature parameter order is (domain, port)"
    );
}

#[tokio::test]
async fn call_decodes_declared_return() {
    let env = new_env();
    let cb: TypedCallable<(i64,), i64> = capture(&env, "return function(n) return n * 2 end").await;
    let got = cb.call((21i64,)).await.expect("call");
    k9::assert_equal!(got, 42i64);
}

#[tokio::test]
async fn call_passes_multiple_args_and_returns() {
    let env = new_env();
    let cb: TypedCallable<(i64, i64), (i64, i64)> =
        capture(&env, "return function(a, b) return a + b, a - b end").await;
    let got = cb.call((10i64, 3i64)).await.expect("call");
    k9::assert_equal!(got, (13i64, 7i64));
}

#[tokio::test]
async fn return_type_mismatch_is_anchored_at_the_handler_return() {
    let env = new_env();
    let cb: TypedCallable<(), bool> = capture(&env, "return function() return 'wrong' end").await;
    let err = cb
        .call(())
        .await
        .expect_err("expected a return-type mismatch");
    let rendered = render_runtime_error(&err, RenderStyle::Plain);
    assert_multi_line_output!(
        rendered,
        "\
error: expected return type 'boolean' but got 'string'
 --> test.lua:1:19
  |
1 | return function() return 'wrong' end
  |                   ^^^^^^^^^^^^^^ expected return type 'boolean' but got 'string'
stack traceback:
	test.lua:1: in function <anonymous>()",
        "return mismatch diagnostic"
    );
}
