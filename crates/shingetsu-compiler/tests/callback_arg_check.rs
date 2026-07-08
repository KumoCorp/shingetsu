//! Compile-time validation of callback-typed function arguments and
//! callback-typed table-literal fields.
//!
//! A host function can declare a parameter (or a table field) whose
//! type is a concrete `fn(A) -> R`.  When a Lua script supplies a
//! function there, the checker validates its arity, parameter names,
//! and return type the same way it validates event handlers.

mod common;

use common::compile_diagnostics_with_env;
use shingetsu_vm::types::{FunctionLuaType, LuaType, TableField, TableLuaType, TypedParam};
use shingetsu_vm::GlobalEnv;

/// A concrete `fn(params) -> returns` type.
fn func_type(params: Vec<TypedParam>, returns: Vec<LuaType>) -> LuaType {
    LuaType::Function(Box::new(FunctionLuaType {
        type_params: vec![],
        params,
        variadic: None,
        returns,
        is_method: false,
        inferred_unannotated: false,
        deprecated: None,
        must_use: None,
    }))
}

/// Env with a global `apply` whose single parameter `cb` is a callback
/// of the given signature.
fn env_with_callback_param(params: Vec<TypedParam>, returns: Vec<LuaType>) -> GlobalEnv {
    let env = common::new_env();
    let apply = func_type(
        vec![TypedParam::new(Some("cb"), func_type(params, returns))],
        vec![],
    );
    env.register_global_type("apply", apply);
    env
}

/// Env with a global `configure` whose single parameter is a table
/// containing `enabled: boolean` and a `callback` of the given
/// signature.
fn env_with_callback_field(params: Vec<TypedParam>, returns: Vec<LuaType>) -> GlobalEnv {
    let env = common::new_env();
    let opts = LuaType::Table(Box::new(TableLuaType {
        fields: vec![
            TableField::new("enabled", LuaType::Boolean),
            TableField::new("callback", func_type(params, returns)),
        ],
        indexer: None,
    }));
    let configure = func_type(vec![TypedParam::new(Some("opts"), opts)], vec![]);
    env.register_global_type("configure", configure);
    env
}

#[tokio::test]
async fn callback_arg_return_mismatch_warns() {
    let env = env_with_callback_param(vec![], vec![LuaType::Boolean]);
    let diags = compile_diagnostics_with_env(&env, "apply(function() return 'wrong' end)").await;
    k9::assert_equal!(
        diags,
        "warning[callback_return_type]: callback 'cb' handler returns 'string' but the signature declares return type 'boolean'
 --> test.lua:1:15
  |
1 | apply(function() return 'wrong' end)
  |               ^^ callback 'cb' handler returns 'string' but the signature declares return type 'boolean'"
    );
}

#[tokio::test]
async fn callback_arg_matching_return_passes() {
    let env = env_with_callback_param(vec![], vec![LuaType::Boolean]);
    let diags = compile_diagnostics_with_env(&env, "apply(function() return true end)").await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn callback_field_return_mismatch_warns() {
    // The motivating case: a validating-constructor table literal whose
    // callback field is supplied a function with the wrong return type.
    let env = env_with_callback_field(vec![], vec![LuaType::Boolean]);
    let diags = compile_diagnostics_with_env(
        &env,
        "configure { enabled = true, callback = function() return 'wrong' end }",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[callback_return_type]: callback 'callback' handler returns 'string' but the signature declares return type 'boolean'
 --> test.lua:1:48
  |
1 | configure { enabled = true, callback = function() return 'wrong' end }
  |                                                ^^ callback 'callback' handler returns 'string' but the signature declares return type 'boolean'"
    );
}

#[tokio::test]
async fn callback_field_arity_warns() {
    let env = env_with_callback_field(
        vec![TypedParam::new(Some("n"), LuaType::Number)],
        vec![LuaType::Boolean],
    );
    let diags = compile_diagnostics_with_env(
        &env,
        "configure { enabled = true, callback = function(a, b) return a == b end }",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[callback_arity]: callback 'callback' declares 1 parameter but the handler accepts 2; extra parameters will always be nil
 --> test.lua:1:48
  |
1 | configure { enabled = true, callback = function(a, b) return a == b end }
  |                                                ^^^^^^ callback 'callback' declares 1 parameter but the handler accepts 2; extra parameters will always be nil"
    );
}

#[tokio::test]
async fn callback_field_matching_passes() {
    let env = env_with_callback_field(vec![], vec![LuaType::Boolean]);
    let diags = compile_diagnostics_with_env(
        &env,
        "configure { enabled = true, callback = function() return true end }",
    )
    .await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn annotated_local_return_mismatch_warns() {
    // A callback-typed local annotation validates the assigned lambda.
    let env = common::new_env();
    let diags = compile_diagnostics_with_env(
        &env,
        "local f: () -> boolean = function() return 'wrong' end\nreturn f",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[callback_return_type]: callback 'f' handler returns 'string' but the signature declares return type 'boolean'
 --> test.lua:1:34
  |
1 | local f: () -> boolean = function() return 'wrong' end
  |                                  ^^ callback 'f' handler returns 'string' but the signature declares return type 'boolean'"
    );
}

#[tokio::test]
async fn annotated_local_matching_passes() {
    let env = common::new_env();
    let diags = compile_diagnostics_with_env(
        &env,
        "local f: () -> boolean = function() return true end\nreturn f",
    )
    .await;
    k9::assert_equal!(diags, "");
}
