//! Verifies the cross-engine event surface:
//!
//! - [`declare_event!`] + [`EventSignature::call`] for kumomta-style
//!   typed dispatch (Single + Multiple).
//! - [`EventSignature::new_single`] for wezterm-style dynamic-name
//!   typed dispatch (`emit_sync_callback` / `emit_async_callback`).
//! - [`emit_event`] for wezterm-style untyped broadcast.
//! - The polymorphic dispatch target: `&Lua`, `&GlobalEnv`, and
//!   `&Engine` should all work as the receiver.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use shingetsu_migrate::shingetsu::{self, GlobalEnv};
use shingetsu_migrate::{declare_event, emit_event, Engine, EventSignature};

declare_event! {
    static GREETING_SINGLE: Single("greeting", who: String) -> String;
}

declare_event! {
    static GREETING_MULTI: Multiple("greeting_multi", who: String) -> String;
}

fn shingetsu_engine() -> Engine {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    let engine = Engine::from_shingetsu(env);
    GREETING_SINGLE.register(&engine);
    GREETING_MULTI.register(&engine);
    engine
}

fn mlua_engine() -> Engine {
    let engine = Engine::from_mlua(mlua::Lua::new());
    GREETING_SINGLE.register(&engine);
    GREETING_MULTI.register(&engine);
    engine
}

// ---------------------------------------------------------------------------
// shingetsu engine: typed dispatch via EventSignature
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_call_with_no_handler_reports_undefined() {
    let engine = shingetsu_engine();
    let disp = GREETING_SINGLE
        .call(&engine, ("world".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.handler_was_defined, false);
    k9::assert_equal!(disp.result, None);
    k9::assert_equal!(disp.event_name, "greeting");
}

#[tokio::test]
async fn shingetsu_single_handler_dispatches() {
    use shingetsu::Function;
    let engine = shingetsu_engine();
    let env = engine.as_shingetsu().unwrap();

    let registry = shingetsu::callback::callback_registry(env);
    let handler = Function::wrap(
        "greeting",
        |who: String| -> Result<String, shingetsu::VmError> { Ok(format!("hello, {who}")) },
    );
    registry.register("greeting", handler).expect("register");

    let disp = GREETING_SINGLE
        .call(&engine, ("world".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.handler_was_defined, true);
    k9::assert_equal!(disp.result, Some("hello, world".to_owned()));
}

#[tokio::test]
async fn shingetsu_multi_handler_picks_first_non_empty() {
    use shingetsu::{Function, Variadic, VmError};
    let engine = shingetsu_engine();
    let env = engine.as_shingetsu().unwrap();

    let registry = shingetsu::callback::callback_registry(env);
    let empty = Function::wrap(
        "greeting_multi/empty",
        |_who: String| -> Result<Variadic, VmError> { Ok(Variadic::default()) },
    );
    let speak = Function::wrap(
        "greeting_multi/speak",
        |who: String| -> Result<String, VmError> { Ok(format!("hi {who}")) },
    );
    registry
        .register("greeting_multi", empty)
        .expect("register empty");
    registry
        .register("greeting_multi", speak)
        .expect("register speak");

    let disp = GREETING_MULTI
        .call(&engine, ("there".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.handler_was_defined, true);
    k9::assert_equal!(disp.result, Some("hi there".to_owned()));
}

// ---------------------------------------------------------------------------
// mlua engine: typed dispatch via EventSignature
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mlua_call_with_no_handler_reports_undefined() {
    let engine = mlua_engine();
    let disp: shingetsu_migrate::EventDisposition<String> = GREETING_SINGLE
        .call(&engine, ("world".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.handler_was_defined, false);
    k9::assert_equal!(disp.result, None);
}

#[tokio::test]
async fn mlua_single_handler_dispatches() {
    let engine = mlua_engine();
    let lua = engine.as_mlua().unwrap();

    let key = GREETING_SINGLE.mlua_registry_key();
    let f = lua
        .create_function(|_, who: String| Ok(format!("hello, {who}")))
        .unwrap();
    lua.set_named_registry_value(&key, f).unwrap();

    let disp = GREETING_SINGLE
        .call(&engine, ("world".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.handler_was_defined, true);
    k9::assert_equal!(disp.result, Some("hello, world".to_owned()));
}

#[tokio::test]
async fn mlua_multi_handler_picks_first_non_empty() {
    let engine = mlua_engine();
    let lua = engine.as_mlua().unwrap();
    let key = GREETING_MULTI.mlua_registry_key();

    let tbl = lua.create_table().unwrap();
    let empty = lua
        .create_function(|_, _who: String| -> mlua::Result<()> { Ok(()) })
        .unwrap();
    let speak = lua
        .create_function(|_, who: String| Ok(format!("hi {who}")))
        .unwrap();
    tbl.push(empty).unwrap();
    tbl.push(speak).unwrap();
    lua.set_named_registry_value(&key, tbl).unwrap();

    let disp = GREETING_MULTI
        .call(&engine, ("there".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.handler_was_defined, true);
    k9::assert_equal!(disp.result, Some("hi there".to_owned()));
}

// ---------------------------------------------------------------------------
// Polymorphic dispatch target: &Lua / &GlobalEnv / &Engine all work
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_call_via_global_env_directly() {
    use shingetsu::Function;
    let engine = shingetsu_engine();
    let env = engine.as_shingetsu().unwrap();
    let registry = shingetsu::callback::callback_registry(env);
    registry
        .register(
            "greeting",
            Function::wrap("h", |who: String| -> Result<String, shingetsu::VmError> {
                Ok(format!("hello, {who}"))
            }),
        )
        .unwrap();

    // Pass the GlobalEnv directly without going through Engine.
    // Existing kumomta call sites that hold &GlobalEnv migrate
    // without changing their callsite signature.
    let disp = GREETING_SINGLE
        .call(env, ("there".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.result, Some("hello, there".to_owned()));
}

#[tokio::test]
async fn mlua_call_via_lua_directly() {
    let engine = mlua_engine();
    let lua = engine.as_mlua().unwrap();
    let key = GREETING_SINGLE.mlua_registry_key();
    let f = lua
        .create_function(|_, who: String| Ok(format!("hello, {who}")))
        .unwrap();
    lua.set_named_registry_value(&key, f).unwrap();

    // Pass &Lua directly.  Pre-migration kumomta call sites keep
    // working unchanged once they swap declare_event to the facade.
    let disp = GREETING_SINGLE
        .call(lua, ("world".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.result, Some("hello, world".to_owned()));
}

// ---------------------------------------------------------------------------
// Dynamic-name signatures (wezterm emit_sync_callback / emit_async_callback)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dynamic_signature_dispatches_on_shingetsu() {
    use shingetsu::Function;
    let engine = shingetsu_engine();
    let env = engine.as_shingetsu().unwrap();
    let registry = shingetsu::callback::callback_registry(env);
    registry
        .register_user_defined(
            "format-tab-title",
            Function::wrap("fmt", |hover: bool| -> Result<String, shingetsu::VmError> {
                Ok(if hover {
                    "[hover]".into()
                } else {
                    "[plain]".into()
                })
            }),
        )
        .unwrap();

    // Construct the signature at runtime, the wezterm pattern.
    let sig: EventSignature<(bool,), String> = EventSignature::new_single("format-tab-title");
    let disp = sig.call(&engine, (true,)).await.expect("call");
    k9::assert_equal!(disp.result, Some("[hover]".to_owned()));
}

// ---------------------------------------------------------------------------
// emit_event: wezterm broadcast semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shingetsu_emit_event_runs_all_handlers_until_false() {
    use shingetsu::Function;
    let engine = shingetsu_engine();
    let env = engine.as_shingetsu().unwrap();
    let registry = shingetsu::callback::callback_registry(env);

    // wezterm-style broadcast events are always multi-handler; the
    // host's `wezterm.on` registration shim is responsible for
    // pre-declaring the name as multi.  We do that here directly.
    registry.declare_static("event-a", true);

    // Track invocations; if the second handler returns false, the
    // third handler should be skipped.
    let log: std::sync::Arc<parking_lot::Mutex<Vec<&'static str>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(Vec::new()));

    let log_a = log.clone();
    registry
        .register_user_defined(
            "event-a",
            Function::wrap(
                "first",
                move |_v: i64| -> Result<bool, shingetsu::VmError> {
                    log_a.lock().push("first");
                    Ok(true)
                },
            ),
        )
        .unwrap();
    let log_b = log.clone();
    registry
        .register_user_defined(
            "event-a",
            Function::wrap(
                "second",
                move |_v: i64| -> Result<bool, shingetsu::VmError> {
                    log_b.lock().push("second");
                    Ok(false)
                },
            ),
        )
        .unwrap();
    let log_c = log.clone();
    registry
        .register_user_defined(
            "event-a",
            Function::wrap(
                "third",
                move |_v: i64| -> Result<bool, shingetsu::VmError> {
                    log_c.lock().push("third");
                    Ok(true)
                },
            ),
        )
        .unwrap();

    let result = emit_event(&engine, "event-a", (1_i64,))
        .await
        .expect("emit");
    k9::assert_equal!(result, false);
    k9::assert_equal!(*log.lock(), vec!["first", "second"]);
}

#[tokio::test]
async fn mlua_emit_event_runs_all_handlers_until_false() {
    let engine = mlua_engine();
    let lua = engine.as_mlua().unwrap();
    let key = format!("wezterm-event-{}", "event-a");
    let tbl = lua.create_table().unwrap();
    tbl.push(
        lua.create_function(|_, _v: i64| -> mlua::Result<bool> { Ok(true) })
            .unwrap(),
    )
    .unwrap();
    tbl.push(
        lua.create_function(|_, _v: i64| -> mlua::Result<bool> { Ok(false) })
            .unwrap(),
    )
    .unwrap();
    tbl.push(
        lua.create_function(|_, _v: i64| -> mlua::Result<bool> {
            panic!("third handler should not have been called")
        })
        .unwrap(),
    )
    .unwrap();
    lua.set_named_registry_value(&key, tbl).unwrap();

    let result = emit_event(&engine, "event-a", (1_i64,))
        .await
        .expect("emit");
    k9::assert_equal!(result, false);
}

#[tokio::test]
async fn emit_event_with_no_handlers_returns_true_on_both_engines() {
    let s_engine = shingetsu_engine();
    let m_engine = mlua_engine();

    k9::assert_equal!(
        emit_event(&s_engine, "no-such-event", (1_i64,))
            .await
            .unwrap(),
        true
    );
    k9::assert_equal!(
        emit_event(&m_engine, "no-such-event", (1_i64,))
            .await
            .unwrap(),
        true
    );
}

// ---------------------------------------------------------------------------
// Static metadata
// ---------------------------------------------------------------------------

#[tokio::test]
async fn signature_metadata_matches_macro_input() {
    k9::assert_equal!(GREETING_SINGLE.name(), "greeting");
    k9::assert_equal!(GREETING_SINGLE.allow_multiple(), false);
    k9::assert_equal!(GREETING_MULTI.name(), "greeting_multi");
    k9::assert_equal!(GREETING_MULTI.allow_multiple(), true);
}
