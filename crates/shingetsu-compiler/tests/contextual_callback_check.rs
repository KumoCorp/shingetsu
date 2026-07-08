//! Contextual typing of event-handler and callback lambda parameters.
//!
//! When a lambda is passed where the callee expects a typed function
//! (an event handler, or a callback argument), the checker binds the
//! lambda's parameters to the expected types inside its body.  That is
//! what lets method calls and nested callback fields on an otherwise
//! untyped handler parameter be validated.

mod common;

use common::compile_diagnostics_with_env;
use shingetsu_vm::types::{FunctionLuaType, LuaType, TableField, TableLuaType, TypedParam};
use shingetsu_vm::{Bytes, GlobalEnv};

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

/// Env modelling a tool registry handed to a `discover_tools` event
/// handler.  The handler's sole parameter `registry` is a table with an
/// `add` method whose argument is a `{ name, run }` table; `run` is a
/// `fn(args, ctx) -> boolean` callback.
fn env_with_tool_registry() -> GlobalEnv {
    let env = common::new_env();

    // agent.on(name, handler) event registrar.
    let on_type = func_type(
        vec![
            TypedParam::new(Some("name"), LuaType::String),
            TypedParam::new(
                Some("handler"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: Some(Box::new(LuaType::Any)),
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: true,
                    deprecated: None,
                    must_use: None,
                })),
            ),
        ],
        vec![],
    );
    let agent = LuaType::Table(Box::new(TableLuaType {
        fields: vec![TableField::new("on", on_type)],
        indexer: None,
    }));
    env.register_global_type("agent", agent);
    env.declare_event_registrar("agent.on");

    // The `run` callback and the `{ name, run }` tool table.
    let run = func_type(
        vec![
            TypedParam::new(Some("args"), LuaType::Any),
            TypedParam::new(Some("ctx"), LuaType::Any),
        ],
        vec![LuaType::Boolean],
    );
    let tool = LuaType::Table(Box::new(TableLuaType {
        fields: vec![
            TableField::new("name", LuaType::String),
            TableField::new("run", run),
        ],
        indexer: None,
    }));

    // registry:add(tool) -- a method, so params[0] is the implicit self.
    let add = LuaType::Function(Box::new(FunctionLuaType {
        type_params: vec![],
        params: vec![
            TypedParam::new(Some("self"), LuaType::Any),
            TypedParam::new(Some("tool"), tool),
        ],
        variadic: None,
        returns: vec![],
        is_method: true,
        inferred_unannotated: false,
        deprecated: None,
        must_use: None,
    }));
    let registry = LuaType::Table(Box::new(TableLuaType {
        fields: vec![TableField::new("add", add)],
        indexer: None,
    }));

    let sig = FunctionLuaType {
        type_params: vec![],
        params: vec![TypedParam::new(Some(Bytes::from("registry")), registry)],
        variadic: None,
        returns: vec![],
        is_method: false,
        inferred_unannotated: false,
        deprecated: None,
        must_use: None,
    };
    env.declare_event_handler_signature("discover_tools", sig);

    env
}

#[tokio::test]
async fn handler_param_method_callback_return_is_checked() {
    let env = env_with_tool_registry();
    let diags = compile_diagnostics_with_env(
        &env,
        "agent.on('discover_tools', function(registry)\n  \
         registry:add { name = 'weather', run = function(_args, _ctx) return 42 end }\n\
         end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[callback_return_type]: callback 'run' handler returns 'integer' but the signature declares return type 'boolean'
 --> test.lua:2:50
  |
2 |   registry:add { name = 'weather', run = function(_args, _ctx) return 42 end }
  |                                                  ^^^^^^^^^^^^^ callback 'run' handler returns 'integer' but the signature declares return type 'boolean'"
    );
}

#[tokio::test]
async fn handler_param_callback_field_wrong_value_is_checked() {
    // A scalar where a callback is expected must be flagged even with
    // the `f { ... }` call sugar (no parentheses around the table).
    let env = env_with_tool_registry();
    let diags = compile_diagnostics_with_env(
        &env,
        "agent.on('discover_tools', function(registry)\n  \
         registry:add { name = 'weather', run = 5 }\n\
         end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "error[arg_type]: expected '{ name: string, run: function }' for parameter 'tool' but got '{ name: string, run: integer }'
 --> test.lua:2:16
  |
2 |   registry:add { name = 'weather', run = 5 }
  |                ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected '{ name: string, run: function }' for parameter 'tool' but got '{ name: string, run: integer }'
  |
help: field 'run' expects 'function' but got 'integer'"
    );
}

#[tokio::test]
async fn handler_param_unknown_method_is_checked() {
    let env = env_with_tool_registry();
    let diags = compile_diagnostics_with_env(
        &env,
        "agent.on('discover_tools', function(registry)\n  \
         registry:no_such_method()\n\
         end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "error[field_access]: unknown field 'no_such_method' on type '{ add: function }'. The only valid field is `add`
 --> test.lua:2:3
  |
2 |   registry:no_such_method()
  |   ^^^^^^^^^^^^^^^^^^^^^^^^^ unknown field 'no_such_method' on type '{ add: function }'. The only valid field is `add`"
    );
}

#[tokio::test]
async fn callback_arg_param_method_is_checked() {
    // The same contextual typing applies to an ordinary callback
    // argument (not just event handlers): `each`'s parameter is a
    // `fn(registry)`, so `registry` inside the lambda is typed and its
    // methods are validated.
    let env = common::new_env();
    let registry = LuaType::Table(Box::new(TableLuaType {
        fields: vec![TableField::new("add", func_type(vec![], vec![]))],
        indexer: None,
    }));
    let each = func_type(
        vec![TypedParam::new(
            Some("cb"),
            func_type(vec![TypedParam::new(Some("registry"), registry)], vec![]),
        )],
        vec![],
    );
    env.register_global_type("each", each);
    let diags =
        compile_diagnostics_with_env(&env, "each(function(registry) registry:nope() end)").await;
    k9::assert_equal!(
        diags,
        "error[field_access]: unknown field 'nope' on type '{ add: function }'. The only valid field is `add`
 --> test.lua:1:25
  |
1 | each(function(registry) registry:nope() end)
  |                         ^^^^^^^^^^^^^^^ unknown field 'nope' on type '{ add: function }'. The only valid field is `add`"
    );
}

#[tokio::test]
async fn annotated_param_method_is_checked() {
    // Part A: an inline parameter annotation binds the parameter type
    // in the body scope, independent of any call-site context.
    let env = common::new_env();
    let diags = compile_diagnostics_with_env(
        &env,
        "local function f(r: { add: () -> () }) r:nope() end\nreturn f",
    )
    .await;
    k9::assert_equal!(
        diags,
        "error[field_access]: unknown field 'nope' on type '{ add: function }'. The only valid field is `add`
 --> test.lua:1:40
  |
1 | local function f(r: { add: () -> () }) r:nope() end
  |                                        ^^^^^^^^ unknown field 'nope' on type '{ add: function }'. The only valid field is `add`"
    );
}

#[tokio::test]
async fn handler_param_matching_tool_passes() {
    let env = env_with_tool_registry();
    let diags = compile_diagnostics_with_env(
        &env,
        "agent.on('discover_tools', function(registry)\n  \
         registry:add { name = 'weather', run = function(_args, _ctx) return true end }\n\
         end)",
    )
    .await;
    k9::assert_equal!(diags, "");
}
