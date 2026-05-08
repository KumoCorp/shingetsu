//! Verifies `install_on` installs a `<module>.on(name, fn)` shim
//! that registers handlers in a slot reachable by the facade's
//! event-dispatch APIs (`EventSignature::call`, `emit_event`).

#![cfg(all(feature = "mlua-backend", feature = "shingetsu-backend"))]

use shingetsu_migrate::shingetsu::{self, GlobalEnv};
use shingetsu_migrate::{declare_event, emit_event, install_on, Engine};

declare_event! {
    static GREETING: Single("greeting", who: String) -> String;
}

declare_event! {
    static MESSAGE_RECEIVED: Single(
        "message_received",
        message: String,
        domain: String,
    ) -> ();
}

fn shingetsu_engine() -> Engine {
    let env = GlobalEnv::new();
    shingetsu::builtins::register(&env).expect("builtins");
    let engine = Engine::from_shingetsu(env);
    GREETING.register(&engine);
    MESSAGE_RECEIVED.register(&engine);
    engine
}

fn mlua_engine() -> Engine {
    let engine = Engine::from_mlua(mlua::Lua::new());
    GREETING.register(&engine);
    MESSAGE_RECEIVED.register(&engine);
    engine
}

#[tokio::test]
async fn shingetsu_install_on_registers_handler_visible_to_event_signature() {
    use shingetsu::{Task, Value, ValueVec};

    let engine = shingetsu_engine();
    install_on(&engine, "myhost").expect("install_on");

    // Drive a small Lua chunk that calls `myhost.on(...)`.
    let env = engine.as_shingetsu().unwrap();
    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile("myhost.on('greeting', function(who) return 'hello, ' .. who end)")
    .await
    .expect("compile");
    let func = bc.into_function();
    let _: ValueVec = Task::new(env.clone(), func, shingetsu_migrate::shingetsu::valuevec![])
        .await
        .expect("task");

    // EventSignature::call now sees the registered handler.
    let disp = GREETING
        .call(&engine, ("world".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.handler_was_defined, true);
    k9::assert_equal!(disp.result, Some("hello, world".to_owned()));
    let _ = Value::Nil; // silence unused import
}

#[tokio::test]
async fn mlua_install_on_registers_handler_visible_to_event_signature() {
    let engine = mlua_engine();
    install_on(&engine, "myhost").expect("install_on");

    let lua = engine.as_mlua().unwrap();
    lua.load("myhost.on('greeting', function(who) return 'hello, ' .. who end)")
        .exec_async()
        .await
        .expect("exec on");

    let disp = GREETING
        .call(&engine, ("world".to_owned(),))
        .await
        .expect("call");
    k9::assert_equal!(disp.handler_was_defined, true);
    k9::assert_equal!(disp.result, Some("hello, world".to_owned()));
}

#[tokio::test]
async fn shingetsu_install_on_handlers_visible_to_emit_event() {
    use shingetsu::{Task, ValueVec};

    let engine = shingetsu_engine();

    // emit_event needs the event name pre-declared as multi.
    let env = engine.as_shingetsu().unwrap();
    shingetsu::callback::callback_registry(env).declare_static("broadcast-x", true);

    install_on(&engine, "myhost").expect("install_on");

    let bc = shingetsu::compiler::Compiler::new(
        shingetsu::compiler::CompileOptions::default(),
        env.global_type_map(),
    )
    .compile(
        "myhost.on('broadcast-x', function() return true end);
         myhost.on('broadcast-x', function() return false end);
         myhost.on('broadcast-x', function() error('third should not run') end);",
    )
    .await
    .expect("compile");
    let func = bc.into_function();
    let _: ValueVec = Task::new(env.clone(), func, shingetsu_migrate::shingetsu::valuevec![])
        .await
        .expect("task");

    let result = emit_event(&engine, "broadcast-x", ()).await.expect("emit");
    k9::assert_equal!(result, false);
}

#[tokio::test]
async fn mlua_install_on_handlers_visible_to_emit_event() {
    let engine = mlua_engine();
    install_on(&engine, "myhost").expect("install_on");

    let lua = engine.as_mlua().unwrap();
    lua.load(
        "myhost.on('broadcast-y', function() return true end);
         myhost.on('broadcast-y', function() return false end);
         myhost.on('broadcast-y', function() error('third should not run') end);",
    )
    .exec_async()
    .await
    .expect("exec on");

    let result = emit_event(&engine, "broadcast-y", ()).await.expect("emit");
    k9::assert_equal!(result, false);
}

/// Compile `src` against `env` with type checking enabled, then
/// produce the same fully-rendered warnings string
/// shingetsu-compiler's own test suite uses for diagnostic
/// assertions.  Lets us `k9::assert_equal!` against the entire
/// rendered output rather than partial-match on individual
/// substrings.
async fn compile_warnings(env: &shingetsu::GlobalEnv, src: &str) -> String {
    use shingetsu::compiler::{CompileOptions, Compiler};
    use shingetsu::diagnostic::{render_warnings, RenderStyle};

    let mut opts = CompileOptions::default();
    opts.source_name = std::sync::Arc::new("=test.lua".to_owned());
    opts.type_check = true;
    let bc = Compiler::new(opts, env.global_type_map())
        .compile(src)
        .await
        .expect("compile");
    let filtered = bc.lint_directives.filter(bc.diagnostics);
    render_warnings(&filtered, src, RenderStyle::Plain)
}

#[tokio::test]
async fn shingetsu_install_on_enables_compile_time_event_name_check() {
    let engine = shingetsu_engine();
    install_on(&engine, "myhost").expect("install_on");

    // GREETING is a typed signature registered above.  A typo'd
    // event name in `myhost.on('greting', ...)` should trigger
    // the EventNameUnknown lint with a did-you-mean suggestion
    // pointing at "greeting".
    let env = engine.as_shingetsu().unwrap();
    let warnings =
        compile_warnings(env, "myhost.on('greting', function(who) return who end)").await;
    k9::assert_equal!(
        warnings,
        "warning[event_name_unknown]: event 'greting' is not a recognised event name. \
Did you mean `greeting`? The other option is `message_received`
 --> test.lua:1:11
  |
1 | myhost.on('greting', function(who) return who end)
  |           ^^^^^^^^^ event 'greting' is not a recognised event name. \
Did you mean `greeting`? The other option is `message_received`"
    );
}

#[tokio::test]
async fn shingetsu_install_on_catches_event_handler_param_transposition() {
    let engine = shingetsu_engine();
    install_on(&engine, "myhost").expect("install_on");

    // The MESSAGE_RECEIVED signature declares parameters in the
    // order (message, domain).  A handler that names them
    // (domain, message) -- transposed -- looks like it forgot
    // which slot holds which value.  shingetsu's compile-time
    // event-handler checker emits an EventHandlerTransposition
    // warning for this; we verify it fires through the migration
    // facade.
    let env = engine.as_shingetsu().unwrap();
    let warnings = compile_warnings(
        env,
        "myhost.on('message_received', function(domain, message) print(domain, message) end)",
    )
    .await;
    k9::assert_equal!(
        warnings,
        "\
warning[event_handler_transposition]: event 'message_received' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
 --> test.lua:1:39
  |
1 | myhost.on('message_received', function(domain, message) print(domain, message) end)
  |                                       ^^^^^^^^^^^^^^^^^ event 'message_received' handler parameter names look transposed relative to the registered signature: position 0 is named 'domain' but signature names that position 'message'; position 1 is named 'message' but signature names that position 'domain'
  |
help: signature parameter order is (message, domain)"
    );
}

#[tokio::test]
async fn install_on_extends_existing_module_table() {
    let engine = mlua_engine();
    let lua = engine.as_mlua().unwrap();

    // Pre-existing host module with other functions on it.
    lua.load("myhost = { greet = function() return 'hi' end }")
        .exec_async()
        .await
        .expect("preload");

    install_on(&engine, "myhost").expect("install_on");

    // Both the pre-existing function and the newly installed `on`
    // are reachable.
    let result: (String, mlua::Function) = lua
        .load("return myhost.greet(), myhost.on")
        .eval()
        .expect("eval");
    k9::assert_equal!(result.0, "hi".to_owned());
    let _ = result.1;
}
