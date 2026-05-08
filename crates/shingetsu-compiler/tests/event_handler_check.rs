//! Compile-time event-handler signature checking.
//!
//! Covers the test matrix for compile-time validation of registered
//! event handler lambdas: arity (forward-compatible), parameter
//! name swap detection, abbreviation tolerance, underscore
//! conventions, and lint suppression.

mod common;

use common::compile_diagnostics_with_env;
use shingetsu::{declare_event, Bytes};
use shingetsu_vm::types::{FunctionLuaType, LuaType, TypedParam};
use shingetsu_vm::GlobalEnv;

/// Build an env that mimics a host's setup: a global named `host` with
/// an `on` field registered as an event registrar, plus the named
/// event signature recorded.
fn env_with_event(event_name: &str, signature_params: &[(&str, LuaType)]) -> GlobalEnv {
    let env = common::new_env();

    // Type the `host.on` function so the existing call-checker has
    // something coherent to look at; the actual event-handler check
    // runs in addition to the regular arg-type pass.
    let on_type = LuaType::Function(Box::new(FunctionLuaType {
        type_params: vec![],
        params: vec![
            TypedParam::new(Some("name"), LuaType::String),
            TypedParam::new(
                Some("callback"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: Some(Box::new(LuaType::Any)),
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: true,
                })),
            ),
        ],
        variadic: None,
        returns: vec![],
        is_method: false,
        inferred_unannotated: false,
    }));

    let host_table = LuaType::Table(Box::new(shingetsu_vm::types::TableLuaType {
        fields: vec![(Bytes::from("on"), on_type)],
        indexer: None,
    }));
    env.register_global_type("host", host_table);
    env.declare_event_registrar("host.on");

    let sig = FunctionLuaType {
        type_params: vec![],
        params: signature_params
            .iter()
            .map(|(n, t)| TypedParam::new(Some(Bytes::from(*n)), t.clone()))
            .collect(),
        variadic: None,
        returns: vec![],
        is_method: false,
        inferred_unannotated: false,
    };
    env.declare_event_handler_signature(event_name, sig);

    env
}

// ---------------------------------------------------------------------------
// Cases that should produce NO event-handler-related diagnostic.
// ---------------------------------------------------------------------------

// Each test uses params by passing them to `print` so the
// `unused_variable` lint doesn't fire on them and pollute the output.

#[tokio::test]
async fn handler_with_canonical_param_passes() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let diags =
        compile_diagnostics_with_env(&env, "host.on('ev', function(message) print(message) end)")
            .await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn handler_with_abbreviated_param_passes() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let diags =
        compile_diagnostics_with_env(&env, "host.on('ev', function(msg) print(msg) end)").await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn handler_with_novel_param_passes() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let diags =
        compile_diagnostics_with_env(&env, "host.on('ev', function(text) print(text) end)").await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn handler_with_single_letter_param_passes() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let diags = compile_diagnostics_with_env(&env, "host.on('ev', function(m) print(m) end)").await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn variadic_handler_skips_all_checks() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let diags =
        compile_diagnostics_with_env(&env, "host.on('ev', function(...) print(...) end)").await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn fewer_params_is_forward_compatible() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    let diags =
        compile_diagnostics_with_env(&env, "host.on('ev', function(message) print(message) end)")
            .await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn abbreviated_params_at_correct_positions_pass() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    let diags = compile_diagnostics_with_env(
        &env,
        "host.on('ev', function(msg, dom) print(msg, dom) end)",
    )
    .await;
    k9::assert_equal!(diags, "");
}

// ---------------------------------------------------------------------------
// Cases that should produce a diagnostic.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extra_params_emit_arity_warning() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let diags = compile_diagnostics_with_env(
        &env,
        "host.on('ev', function(message, extra) print(message, extra) end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_arity]: event 'ev' declares 1 parameter but the handler accepts 2; extra parameters will always be nil
 --> test.lua:1:23
  |
1 | host.on('ev', function(message, extra) print(message, extra) end)
  |                       ^^^^^^^^^^^^^^^^ event 'ev' declares 1 parameter but the handler accepts 2; extra parameters will always be nil"
    );
}

#[tokio::test]
async fn canonical_transposition_emits_warning() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    let diags = compile_diagnostics_with_env(
        &env,
        "host.on('ev', function(domain, message) print(domain, message) end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_transposition]: event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
 --> test.lua:1:23
  |
1 | host.on('ev', function(domain, message) print(domain, message) end)
  |                       ^^^^^^^^^^^^^^^^^ event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
  |
help: signature parameter order is (message, domain)"
    );
}

#[tokio::test]
async fn abbreviated_transposition_also_warns() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    let diags = compile_diagnostics_with_env(
        &env,
        "host.on('ev', function(dom, msg) print(dom, msg) end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_transposition]: event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'dom' but signature names that position 'message'; position 1 is named 'msg' but signature names that position 'domain'
 --> test.lua:1:23
  |
1 | host.on('ev', function(dom, msg) print(dom, msg) end)
  |                       ^^^^^^^^^^ event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'dom' but signature names that position 'message'; position 1 is named 'msg' but signature names that position 'domain'
  |
help: signature parameter order is (message, domain)"
    );
}

// ---------------------------------------------------------------------------
// Negative cases — calls that look similar but shouldn't trigger checks.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_event_name_with_no_close_match_warns_minimally() {
    // Event name not registered → the type checker emits a soft
    // warning, but with no close-match suggestion when no known
    // event is similar.  The handler-shape check is skipped (we
    // don't have a signature to validate against).
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let diags = compile_diagnostics_with_env(
        &env,
        "host.on('mystery', function(a, b, c) print(a, b, c) end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[event_name_unknown]: event 'mystery' is not a recognised event name. The only valid event is `ev`
 --> test.lua:1:9
  |
1 | host.on('mystery', function(a, b, c) print(a, b, c) end)
  |         ^^^^^^^^^ event 'mystery' is not a recognised event name. The only valid event is `ev`"
    );
}

#[tokio::test]
async fn unknown_event_name_with_close_match_suggests() {
    // The user wrote `get_quere_config` but only `get_queue_config`
    // is registered — close-match suggestion fires.
    let env = env_with_event("get_queue_config", &[("domain", LuaType::String)]);
    let diags = compile_diagnostics_with_env(
        &env,
        "host.on('get_quere_config', function(d) print(d) end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[event_name_unknown]: event 'get_quere_config' is not a recognised event name. Did you mean `get_queue_config`?
 --> test.lua:1:9
  |
1 | host.on('get_quere_config', function(d) print(d) end)
  |         ^^^^^^^^^^^^^^^^^^ event 'get_quere_config' is not a recognised event name. Did you mean `get_queue_config`?"
    );
}

#[tokio::test]
async fn unknown_event_name_warning_suppressible() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let src =
        "--# shingetsu: allow(event_name_unknown)\nhost.on('mystery', function(a) print(a) end)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn non_registrar_call_skipped_even_with_event_name_match() {
    // Build an env where `unrelated.on` exists as a global but is NOT
    // marked as an event registrar.  Even though the event name and
    // handler shape match a real registered event, this call should
    // pass through silently.
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );

    let on_type = LuaType::Function(Box::new(FunctionLuaType {
        type_params: vec![],
        params: vec![
            TypedParam::new(Some("name"), LuaType::String),
            TypedParam::new(
                Some("callback"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: Some(Box::new(LuaType::Any)),
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: true,
                })),
            ),
        ],
        variadic: None,
        returns: vec![],
        is_method: false,
        inferred_unannotated: false,
    }));
    let unrelated = LuaType::Table(Box::new(shingetsu_vm::types::TableLuaType {
        fields: vec![(Bytes::from("on"), on_type)],
        indexer: None,
    }));
    env.register_global_type("unrelated", unrelated);

    let diags = compile_diagnostics_with_env(
        &env,
        "unrelated.on('ev', function(domain, message) print(domain, message) end)",
    )
    .await;
    k9::assert_equal!(diags, "");
}

// Underscore-prefixed handler params should not trip false transposition
// warnings when used at the canonical positions — they're a convention
// for "intentionally unused", not a different name.
#[tokio::test]
async fn underscore_prefixed_params_pass_at_canonical_positions() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    let diags =
        compile_diagnostics_with_env(&env, "host.on('ev', function(_msg, _domain) end)").await;
    k9::assert_equal!(diags, "");
}

// ---------------------------------------------------------------------------
// Out-of-band handler functions — declared separately and passed by name.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn local_function_handler_passed_by_name_is_validated() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    // Local function transposes the parameters; should warn even
    // though the function is passed by name rather than as a literal.
    let src = "local function handler(domain, message) print(domain, message) end\nhost.on('ev', handler)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_transposition]: event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
 --> test.lua:2:15
  |
1 | local function handler(domain, message) print(domain, message) end
  |                       ----------------- event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
2 | host.on('ev', handler)
  |               ^^^^^^^ registering handler here
  |
help: signature parameter order is (message, domain)"
    );
}

#[tokio::test]
async fn local_function_arity_is_validated() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let src = "local function handler(message, surprise) print(message, surprise) end\nhost.on('ev', handler)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_arity]: event 'ev' declares 1 parameter but the handler accepts 2; extra parameters will always be nil
 --> test.lua:2:15
  |
1 | local function handler(message, surprise) print(message, surprise) end
  |                       ------------------- event 'ev' declares 1 parameter but the handler accepts 2; extra parameters will always be nil
2 | host.on('ev', handler)
  |               ^^^^^^^ registering handler here"
    );
}

#[tokio::test]
async fn local_function_with_canonical_names_passes() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    let src = "local function handler(message, domain) print(message, domain) end\nhost.on('ev', handler)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn module_table_function_handler_is_validated() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    // `function mod.handler(...)` syntax — looked up as the qualified
    // name `mod.handler` in the side-channel signature index.
    let src = "local mod = {}\nfunction mod.handler(domain, message) print(domain, message) end\nhost.on('ev', mod.handler)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_transposition]: event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
 --> test.lua:3:15
  |
2 | function mod.handler(domain, message) print(domain, message) end
  |                     ----------------- event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
3 | host.on('ev', mod.handler)
  |               ^^^^^^^^^^^ registering handler here
  |
help: signature parameter order is (message, domain)"
    );
}

#[tokio::test]
async fn module_table_function_canonical_passes() {
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    let src = "local mod = {}\nfunction mod.handler(message, domain) print(message, domain) end\nhost.on('ev', mod.handler)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn unknown_function_reference_skipped_silently() {
    // Reference to a global / out-of-scope name we know nothing
    // about.  Skip silently rather than producing a phantom warning.
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let diags =
        compile_diagnostics_with_env(&env, "host.on('ev', some_global_we_dont_know_about)").await;
    k9::assert_equal!(diags, "");
}

// ---------------------------------------------------------------------------
// Native handler function exposed via GlobalTypeMap participates in the check.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn native_global_function_handler_is_validated() {
    // The host has registered a typed global function as a default
    // handler.  When the user passes that global to `host.on`, the
    // type checker should validate against its declared param shape.
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    // Register `default_handler` as a typed native function global.
    let native_type = LuaType::Function(Box::new(FunctionLuaType {
        type_params: vec![],
        params: vec![
            TypedParam::new(Some("domain"), LuaType::String),
            TypedParam::new(Some("message"), LuaType::String),
        ],
        variadic: None,
        returns: vec![],
        is_method: false,
        inferred_unannotated: false,
    }));
    env.register_global_type("default_handler", native_type);

    // Native function has its params in transposed order vs the
    // signature — the check should catch it even though the function
    // is host-defined and has no chunk-level source span.
    let diags = compile_diagnostics_with_env(&env, "host.on('ev', default_handler)").await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_transposition]: event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
 --> test.lua:1:15
  |
1 | host.on('ev', default_handler)
  |               ^^^^^^^^^^^^^^^ event 'ev' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
  |
help: signature parameter order is (message, domain)"
    );
}

// ---------------------------------------------------------------------------
// Lint suppression
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arity_warning_suppressible_via_directive() {
    // A handler that accepts an extra param can opt out of the
    // warning via the standard `--# shingetsu: allow(...)` directive.
    // Confirms the new lint plumbs through the existing suppression
    // infrastructure.
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let src = "--# shingetsu: allow(event_handler_arity)\nhost.on('ev', function(message, extra) print(message, extra) end)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(diags, "");
}

#[tokio::test]
async fn transposition_warning_suppressible_independently() {
    // The two new lints are split (event_handler_arity and
    // event_handler_transposition) so users can suppress one without
    // the other.  This confirms that suppression is fine-grained.
    let env = env_with_event(
        "ev",
        &[("message", LuaType::String), ("domain", LuaType::String)],
    );
    let src = "--# shingetsu: allow(event_handler_transposition)\nhost.on('ev', function(domain, message) print(domain, message) end)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(diags, "");
}

// Multi-line lambda body — confirms the diagnostic location scopes to
// just the parameter list rather than spanning the entire (possibly
// large) function expression.
#[tokio::test]
async fn arity_warning_localizes_to_parameter_list_for_multiline_lambda() {
    let env = env_with_event("ev", &[("message", LuaType::String)]);
    let src = "host.on('ev', function(message, extra)\n    print(message)\n    print(extra)\nend)";
    let diags = compile_diagnostics_with_env(&env, src).await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_arity]: event 'ev' declares 1 parameter but the handler accepts 2; extra parameters will always be nil
 --> test.lua:1:23
  |
1 | host.on('ev', function(message, extra)
  |                       ^^^^^^^^^^^^^^^^ event 'ev' declares 1 parameter but the handler accepts 2; extra parameters will always be nil"
    );
}

// ---------------------------------------------------------------------------
// Did-you-mean for unknown table fields on a statically-typed receiver.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_field_emits_did_you_mean_suggestion() {
    let env = common::new_env();
    // Configure a typed `cfg` global with two fields.  Reading a
    // typo'd field name should emit `field_access` with a suggestion.
    let cfg_table = LuaType::Table(Box::new(shingetsu_vm::types::TableLuaType {
        fields: vec![
            (Bytes::from("font_size"), LuaType::Number),
            (Bytes::from("line_height"), LuaType::Number),
        ],
        indexer: None,
    }));
    env.register_global_type("cfg", cfg_table);
    let diags = compile_diagnostics_with_env(&env, "return cfg.font_sze").await;
    k9::assert_equal!(
        diags,
        "error[field_access]: unknown field 'font_sze' on type 'cfg'. Did you mean `font_size`? The other option is `line_height`
 --> test.lua:1:8
  |
1 | return cfg.font_sze
  |        ^^^^^^^^^^^^ unknown field 'font_sze' on type 'cfg'. Did you mean `font_size`? The other option is `line_height`"
    );
}

// `declare_event!` round-trip: a typed signature published into a
// GlobalTypeMap is consumed correctly by the compiler.
#[tokio::test]
async fn declare_event_macro_round_trip() {
    declare_event! {
        pub static GREET: Single("greet", message: String) -> ();
    }

    let env = common::new_env();
    let on_type = LuaType::Function(Box::new(FunctionLuaType {
        type_params: vec![],
        params: vec![
            TypedParam::new(Some("name"), LuaType::String),
            TypedParam::new(
                Some("callback"),
                LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: Some(Box::new(LuaType::Any)),
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: true,
                })),
            ),
        ],
        variadic: None,
        returns: vec![],
        is_method: false,
        inferred_unannotated: false,
    }));
    let host_tab = LuaType::Table(Box::new(shingetsu_vm::types::TableLuaType {
        fields: vec![(Bytes::from("on"), on_type)],
        indexer: None,
    }));
    env.register_global_type("host", host_tab);
    env.declare_event_registrar("host.on");
    if let Some(ft) = GREET.handler_function_type() {
        env.declare_event_handler_signature("greet", ft);
    }

    // Handler with too many parameters → the macro-published signature
    // drives the arity check.
    let diags = compile_diagnostics_with_env(
        &env,
        "host.on('greet', function(message, surprise) print(message, surprise) end)",
    )
    .await;
    k9::assert_equal!(
        diags,
        "warning[event_handler_arity]: event 'greet' declares 1 parameter but the handler accepts 2; extra parameters will always be nil
 --> test.lua:1:26
  |
1 | host.on('greet', function(message, surprise) print(message, surprise) end)
  |                          ^^^^^^^^^^^^^^^^^^^ event 'greet' declares 1 parameter but the handler accepts 2; extra parameters will always be nil"
    );
}
