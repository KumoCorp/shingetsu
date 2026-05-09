//! Cross-engine parity tests for the event facade.
//!
//! Each test runs the *same* Lua script (registering one or more
//! handlers via `myhost.on(...)`) and the *same* Rust dispatch
//! (`EventSignature::call` or `emit_event`) on both backends, and
//! asserts identical observable behavior.  Where the existing
//! per-engine tests in `event.rs` and `install_on.rs` exercise
//! each backend in isolation, this file confirms the two
//! variants stay in lockstep through the facade.

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use shingetsu_migrate::shingetsu::{self, GlobalEnv};
use shingetsu_migrate::{declare_event, emit_event, install_on, Engine, EventDisposition};

declare_event! {
    static SINGLE: Single("single", who: String) -> String;
}

declare_event! {
    static MULTI: Multiple("multi", who: String) -> String;
}

fn shingetsu_engine() -> Engine {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    let engine = Engine::from_shingetsu(env);
    SINGLE.register(&engine);
    MULTI.register(&engine);
    install_on(&engine, "myhost").expect("install_on");
    engine
}

fn mlua_engine() -> Engine {
    let engine = Engine::from_mlua(mlua::Lua::new());
    SINGLE.register(&engine);
    MULTI.register(&engine);
    install_on(&engine, "myhost").expect("install_on");
    engine
}

/// Run `script` against `engine` for side effects.  Used to drive
/// `myhost.on(...)` registrations from Lua before invoking events
/// from Rust.
async fn run_script(engine: &Engine, src: &str) {
    match engine {
        Engine::Shingetsu(env) => {
            use shingetsu::compiler::{CompileOptions, Compiler};
            let mut opts = CompileOptions::default();
            opts.source_name = std::sync::Arc::new("=parity.lua".to_owned());
            let bc = Compiler::new(opts, env.global_type_map())
                .compile(src)
                .await
                .expect("shingetsu compile");
            let func = bc.into_function();
            let _ = shingetsu::Task::new(env.clone(), func, shingetsu::valuevec![])
                .await
                .expect("shingetsu task");
        }
        Engine::Mlua(lua) => {
            lua.load(src)
                .set_name("=parity.lua")
                .exec_async()
                .await
                .expect("mlua exec");
        }
        _ => unreachable!("Engine variant not enabled in this build"),
    }
}

/// Decompose an EventDisposition into a comparable shape so the
/// two engines' dispositions can be asserted identical.
fn parts(disp: EventDisposition<String>) -> (bool, Option<String>, String) {
    (disp.handler_was_defined, disp.result, disp.event_name)
}

// ---------------------------------------------------------------------------
// Behavior parity: same script, same dispatch, identical results
// ---------------------------------------------------------------------------

#[tokio::test]
async fn parity_no_handler_yields_undefined_disposition() {
    let s = SINGLE
        .call(&shingetsu_engine(), ("world".to_owned(),))
        .await
        .expect("shingetsu call");
    let m = SINGLE
        .call(&mlua_engine(), ("world".to_owned(),))
        .await
        .expect("mlua call");
    k9::assert_equal!(parts(s), (false, None, "single".to_owned()));
    k9::assert_equal!(parts(m), (false, None, "single".to_owned()));
}

#[tokio::test]
async fn parity_single_handler_dispatch() {
    let script = "myhost.on('single', function(who) return 'hi ' .. who end)";

    let s_engine = shingetsu_engine();
    run_script(&s_engine, script).await;
    let s = SINGLE
        .call(&s_engine, ("alice".to_owned(),))
        .await
        .expect("shingetsu call");

    let m_engine = mlua_engine();
    run_script(&m_engine, script).await;
    let m = SINGLE
        .call(&m_engine, ("alice".to_owned(),))
        .await
        .expect("mlua call");

    let expected = (true, Some("hi alice".to_owned()), "single".to_owned());
    k9::assert_equal!(parts(s), expected.clone());
    k9::assert_equal!(parts(m), expected);
}

#[tokio::test]
async fn parity_multi_handler_first_non_empty() {
    let script = "\
        myhost.on('multi', function(_who) end);\n\
        myhost.on('multi', function(who) return 'first ' .. who end);\n\
        myhost.on('multi', function(_who) error('third should not run') end);\n";

    let s_engine = shingetsu_engine();
    run_script(&s_engine, script).await;
    let s = MULTI
        .call(&s_engine, ("bob".to_owned(),))
        .await
        .expect("shingetsu call");

    let m_engine = mlua_engine();
    run_script(&m_engine, script).await;
    let m = MULTI
        .call(&m_engine, ("bob".to_owned(),))
        .await
        .expect("mlua call");

    let expected = (true, Some("first bob".to_owned()), "multi".to_owned());
    k9::assert_equal!(parts(s), expected.clone());
    k9::assert_equal!(parts(m), expected);
}

#[tokio::test]
async fn parity_emit_event_runs_all_handlers_until_false() {
    let script = "\
        myhost.on('broadcast', function() return true end);\n\
        myhost.on('broadcast', function() return false end);\n\
        myhost.on('broadcast', function() error('third should not run') end);\n";

    // emit_event broadcasts are inherently multi-handler; the
    // shingetsu side requires the name to be declared multi
    // before any handler registers.
    let s_engine = shingetsu_engine();
    shingetsu::callback::callback_registry(s_engine.as_shingetsu().unwrap())
        .declare_static("broadcast", true);
    run_script(&s_engine, script).await;
    let s = emit_event(&s_engine, "broadcast", ())
        .await
        .expect("shingetsu emit");

    let m_engine = mlua_engine();
    run_script(&m_engine, script).await;
    let m = emit_event(&m_engine, "broadcast", ())
        .await
        .expect("mlua emit");

    k9::assert_equal!(s, false);
    k9::assert_equal!(m, false);
}

#[tokio::test]
async fn parity_handler_error_propagates_through_call() {
    // A handler that errors from inside Lua surfaces as an Err
    // from EventSignature::call on both engines.  The exact
    // rendered string differs (shingetsu's annotated diagnostic
    // vs mlua's plain message), but both must Err.
    let script = "myhost.on('single', function(_who) error('boom') end);";

    let s_engine = shingetsu_engine();
    run_script(&s_engine, script).await;
    let s_err = SINGLE
        .call(&s_engine, ("alice".to_owned(),))
        .await
        .expect_err("expected shingetsu handler error");

    let m_engine = mlua_engine();
    run_script(&m_engine, script).await;
    let m_err = SINGLE
        .call(&m_engine, ("alice".to_owned(),))
        .await
        .expect_err("expected mlua handler error");

    // Capture each engine's rendered error verbatim so any
    // future wording change is visible at review time.  The
    // strings differ structurally (shingetsu's location-prefixed
    // line vs mlua's stack-traceback message); we assert each
    // engine's full output.
    k9::assert_equal!(format!("{s_err}"), "parity.lua:1: boom");
    k9::assert_equal!(
        format!("{m_err}"),
        "\
runtime error: parity.lua:1: boom
stack traceback:
\t[C]: in function 'error'
\tparity.lua:1: in function <parity.lua:1>"
    );
}

#[tokio::test]
async fn shingetsu_single_event_rejects_duplicate_registration_from_lua() {
    // Single events on the shingetsu side accept exactly one
    // handler: the second `myhost.on('single', ...)` errors
    // during script execution.  This is *not* a parity test --
    // mlua has no equivalent enforcement layer at the facade
    // level; kumomta's existing host-side `kumo.on`
    // implementation maintains its own
    // `does_callback_allow_multiple` check that the migration
    // facade does not replicate.  Hosts migrating from kumomta
    // keep that check on the mlua side until the host fully
    // moves to shingetsu, at which point the registry-side
    // enforcement takes over.
    let script = "\
        myhost.on('single', function(who) return who end);\n\
        myhost.on('single', function(who) return who end);\n";

    let engine = shingetsu_engine();
    let err = run_script_expecting_error(&engine, script).await;
    k9::assert_equal!(
        err,
        "\
error: error in 'callback': event 'single' allows only a single event handler to be defined; another handler has already been registered for this name
 --> parity.lua:2:11
  |
2 | myhost.on('single', function(who) return who end);
  |           ^^^^^^^^ error in 'callback': event 'single' allows only a single event handler to be defined; another handler has already been registered for this name
stack traceback:
\tparity.lua:2: in main chunk"
    );
}

// ---------------------------------------------------------------------------
// Helpers + per-engine error fixtures
// ---------------------------------------------------------------------------

async fn run_script_expecting_error(engine: &Engine, src: &str) -> String {
    use shingetsu::diagnostic::{render_runtime_error, RenderStyle};

    match engine {
        Engine::Shingetsu(env) => {
            use shingetsu::compiler::{CompileOptions, Compiler};
            let mut opts = CompileOptions::default();
            opts.source_name = std::sync::Arc::new("=parity.lua".to_owned());
            let bc = Compiler::new(opts, env.global_type_map())
                .compile(src)
                .await
                .expect("compile");
            let func = bc.into_function();
            match shingetsu::Task::new(env.clone(), func, shingetsu::valuevec![]).await {
                Ok(_) => panic!("expected shingetsu runtime error"),
                Err(e) => render_runtime_error(&e, RenderStyle::Plain),
            }
        }
        Engine::Mlua(lua) => match lua.load(src).set_name("=parity.lua").exec_async().await {
            Ok(_) => panic!("expected mlua runtime error"),
            Err(e) => format!("{e}"),
        },
        _ => unreachable!("Engine variant not enabled in this build"),
    }
}
