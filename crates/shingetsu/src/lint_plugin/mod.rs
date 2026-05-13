//! Lint-plugin runner and the `shingetsu.lint` host module.
//!
//! Plugins are shingetsu Lua files loaded into a dedicated, sandboxed
//! `GlobalEnv` separate from the user's code under analysis.  Each
//! plugin file calls `lint.declare {...}` exactly once to register
//! itself with a name and metadata, then any number of
//! `lint.on(event, fn)` calls to attach visitor callbacks.
//!
//! Event registration and dispatch ride on shingetsu's standard
//! `declare_event!` / [`callback_registry`] infrastructure (see
//! `docs/embedding/events.md`).  Events are declared as `Multiple`
//! so several plugins can listen to the same node kind; visitor
//! handlers return nothing so every registered handler fires on
//! every visited node.  Unknown event names are rejected by the
//! callback registry's `NamePolicy::Closed`.

mod dispatch;
mod node;

pub use dispatch::dispatch_chunk;
pub use node::LintContext;
pub use shingetsu_compiler::lint_ir::{Assign, FunctionCall, MethodCall};

use crate::diagnostic::{render_compile_error, render_runtime_error, RenderStyle};
use crate::sync::RwLock;
use crate::{
    declare_event, register_libs, CallContext, Function, GlobalEnv, Libraries, Ud, Value, VmError,
};
use shingetsu_compiler::Severity;
use shingetsu_vm::callback::{callback_registry, NamePolicy};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn runtime_error(msg: impl Into<String>) -> VmError {
    let msg = msg.into();
    VmError::LuaError {
        display: msg.clone(),
        value: Value::string(msg),
    }
}

/// A lint plugin declaration parsed from a `lint.declare {...}`
/// call.  One per plugin file (multiple `declare` in one file is a
/// load-time error).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginDeclaration {
    /// Snake-case ASCII identifier.  Used in diagnostics as
    /// `project:<name>` (see [`shingetsu_compiler::LintId::Plugin`])
    /// and in `--# shingetsu: allow=...` directives.
    pub name: String,
    pub description: String,
    pub default_severity: Severity,
    /// Lint set memberships.  Empty by default (lint runs only when
    /// explicitly enabled).
    pub sets: Vec<String>,
    /// Minimum host schema version the plugin requires.  Advisory;
    /// the host loads regardless because there is no frozen schema
    /// yet.
    pub min_schema: Option<u32>,
    /// Path to the plugin source file.
    pub source_path: PathBuf,
}

/// Shared registry attached to the plugin `GlobalEnv`'s extension
/// storage.  Holds plugin declarations only -- visitor callbacks
/// live in the standard [`callback_registry`] keyed by event name.
pub struct PluginRegistry {
    inner: RwLock<Inner>,
}

struct Inner {
    declarations: Vec<PluginDeclaration>,
    /// `Some` while a plugin file is being loaded.
    loading: Option<LoadingState>,
}

struct LoadingState {
    path: PathBuf,
    /// Filled in by `declare`.  When the load completes without
    /// this being set, the plugin file is invalid.
    declared: Option<PluginDeclaration>,
}

impl PluginRegistry {
    fn new() -> Self {
        Self {
            inner: RwLock::new(Inner {
                declarations: Vec::new(),
                loading: None,
            }),
        }
    }

    /// All plugins registered against this env, in load order.
    pub fn declarations(&self) -> Vec<PluginDeclaration> {
        self.inner.read().declarations.clone()
    }

    fn begin_load(&self, path: PathBuf) -> Result<(), PluginLoadError> {
        let mut inner = self.inner.write();
        if inner.loading.is_some() {
            return Err(PluginLoadError::ConcurrentLoad);
        }
        inner.loading = Some(LoadingState {
            path,
            declared: None,
        });
        Ok(())
    }

    fn end_load(&self) -> Result<PluginDeclaration, PluginLoadError> {
        let mut inner = self.inner.write();
        let state = inner.loading.take().ok_or(PluginLoadError::NotLoading)?;
        let Some(decl) = state.declared else {
            return Err(PluginLoadError::MissingDeclare { path: state.path });
        };
        inner.declarations.push(decl.clone());
        Ok(decl)
    }

    fn abort_load(&self) {
        let mut inner = self.inner.write();
        inner.loading = None;
    }

    fn attach_declaration(&self, decl: PluginDeclaration) -> Result<(), VmError> {
        // Cross-plugin duplicate-name detection isn't done here.
        // Each plugin lives in its own `GlobalEnv` (one-plugin-per-env),
        // so this registry only ever sees a single plugin's
        // declaration.  Collision detection across plugins is the
        // orchestrator's responsibility.
        let mut inner = self.inner.write();
        let Some(state) = inner.loading.as_mut() else {
            return Err(runtime_error("lint.declare called outside of plugin load"));
        };
        if state.declared.is_some() {
            return Err(runtime_error(
                "lint.declare called more than once in the same plugin file",
            ));
        }
        state.declared = Some(decl);
        Ok(())
    }
}

/// Errors raised from the load harness around the plugin chunk.
/// VM-side errors raised by `lint.declare` / `lint.on` flow through
/// [`VmError`] and surface through the load function's rendered
/// runtime-error path.
#[derive(Debug, thiserror::Error)]
pub enum PluginLoadError {
    #[error("plugin loader is already in the middle of loading another plugin")]
    ConcurrentLoad,
    #[error("end_load called without a matching begin_load")]
    NotLoading,
    #[error("plugin file {path} never called `lint.declare {{...}}`")]
    MissingDeclare { path: PathBuf },
}

/// Retrieve (lazily creating) the [`PluginRegistry`] for `env`.
pub fn registry(env: &GlobalEnv) -> Arc<PluginRegistry> {
    env.extension_or_init(PluginRegistry::new)
}

// ---------------------------------------------------------------------------
// Lint-IR event signatures.
//
// Each event the plugin layer fires gets a declare_event! entry as a
// `Multiple` so multiple plugins can register handlers for the same
// node kind.  Param types are `Value` for now: the lint IR's node /
// ctx userdata types don't exist yet, and we don't want compile-time
// type errors blocking plugin compilation while that scaffolding is
// still landing.  When the userdata are in place these signatures
// tighten and the type checker validates handler parameter types in
// addition to event names.
// ---------------------------------------------------------------------------

declare_event! {
    /// Fired once for each method call expression
    /// (`receiver:m(args)`) in the chunk under analysis.
    pub static METHOD_CALL_EVENT: Multiple(
        "method_call",
        /// the method-call node
        node: Ud<MethodCall>,
        /// the lint context
        ctx: Ud<LintContext>,
    ) -> ();
}

declare_event! {
    /// Fired once for each function call expression (`f(args)`,
    /// including the `f { ... }` and `f "str"` sugar forms).
    pub static FUNCTION_CALL_EVENT: Multiple(
        "function_call",
        /// the function-call node
        node: Ud<FunctionCall>,
        /// the lint context
        ctx: Ud<LintContext>,
    ) -> ();
}

declare_event! {
    /// Fired once for each multi-target assignment statement
    /// (`a, b = x, y`).
    pub static ASSIGN_EVENT: Multiple(
        "assign",
        /// the assign node
        node: Ud<Assign>,
        /// the lint context
        ctx: Ud<LintContext>,
    ) -> ();
}

// ---------------------------------------------------------------------------
// `shingetsu.lint` host module
// ---------------------------------------------------------------------------

/// Build a fresh sandboxed `GlobalEnv` ready to load lint plugins.
///
/// Registers [`Libraries::SANDBOXED`] (math, string, table, utf8,
/// regex, bit32) plus the `shingetsu.lint` host module.  No I/O,
/// `os`, or filesystem access -- plugins that need those must be
/// loaded into a separately constructed env.
pub fn new_plugin_env() -> Result<GlobalEnv, VmError> {
    let env = GlobalEnv::new();
    register_libs(&env, Libraries::SANDBOXED)?;
    register(&env)?;
    Ok(env)
}

/// Register the `shingetsu.lint` host module on `env`.
///
/// Wires the module as a require-able preload, declares each
/// lint-IR event under [`callback_registry`] with `NamePolicy::Closed`
/// so unknown names in `lint.on(...)` produce a rendered runtime
/// error with a did-you-mean suggestion, and publishes the
/// compile-time signatures + event-registrar path so the type
/// checker independently catches the same typo as an
/// `event_name_unknown` diagnostic before the chunk runs.
pub fn register(env: &GlobalEnv) -> Result<(), VmError> {
    lint_mod::register_preload(env);
    env.register_userdata_type(shingetsu_compiler::lint_ir::Span::userdata_type());
    env.register_userdata_type(MethodCall::userdata_type());
    env.register_userdata_type(FunctionCall::userdata_type());
    env.register_userdata_type(Assign::userdata_type());
    env.register_userdata_type(node::LintContext::userdata_type());
    env.declare_event_registrar("shingetsu.lint.on");
    METHOD_CALL_EVENT.register(env);
    FUNCTION_CALL_EVENT.register(env);
    ASSIGN_EVENT.register(env);
    let mut tm = env.global_type_map();
    METHOD_CALL_EVENT.register_compile_type(&mut tm);
    FUNCTION_CALL_EVENT.register_compile_type(&mut tm);
    ASSIGN_EVENT.register_compile_type(&mut tm);
    for (name, sig) in tm.event_handler_signatures {
        env.declare_event_handler_signature(name, sig);
    }
    callback_registry(env).set_policy(NamePolicy::Closed);
    Ok(())
}

/// Arguments accepted by `lint.declare {...}`.  Unknown table keys
/// are silently ignored, matching the default behaviour of
/// `derive(LuaTable)` -- forward-compat insurance against a future
/// plugin using a key the host doesn't recognise.
#[derive(crate::LuaTable, Debug)]
pub struct DeclareArgs {
    #[lua(validate = "validate_lint_name")]
    pub name: String,
    pub description: String,
    #[lua(default)]
    pub default_severity: Severity,
    #[lua(default)]
    pub sets: Vec<String>,
    pub min_schema: Option<u32>,
}

#[crate::module(name = "shingetsu.lint")]
mod lint_mod {
    use super::*;

    /// Declare this file as a lint plugin.  Required exactly once
    /// per file; subsequent calls raise.  May come before or after
    /// any `lint.on(...)` calls in the same file.
    #[function]
    fn declare(ctx: CallContext, args: DeclareArgs) -> Result<(), VmError> {
        let reg = registry(&ctx.global);
        let path = reg
            .inner
            .read()
            .loading
            .as_ref()
            .map(|s| s.path.clone())
            .unwrap_or_else(|| PathBuf::from("<unknown>"));
        let decl = PluginDeclaration {
            name: args.name,
            description: args.description,
            default_severity: args.default_severity,
            sets: args.sets,
            min_schema: args.min_schema,
            source_path: path,
        };
        reg.attach_declaration(decl)?;
        Ok(())
    }

    /// Register a visitor callback for the named lint-IR event.
    /// Backed by the standard [`callback_registry`]: the closed
    /// name policy rejects unknown event names with a did-you-mean
    /// suggestion, and the registry's `Multiple` dispatch invokes
    /// every registered handler on every visited node.
    #[function]
    fn on(ctx: CallContext, event: String, callback: Function) -> Result<(), VmError> {
        callback_registry(&ctx.global).register(event, callback)?;
        Ok(())
    }
}

/// `#[lua(validate = "...")]` callback for the `name` field on
/// [`DeclareArgs`].  Returns `Err(String)` on rejection; the
/// derive turns the string into a `BadArgument` VmError with
/// position 0 and the failing field key surfaced in the message.
fn validate_lint_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("lint name must not be empty".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(format!(
            "lint name '{name}' must be snake_case ASCII \
             (lowercase letters, digits, underscores)"
        ));
    }
    if name.starts_with('_') {
        return Err(format!("lint name '{name}' must not start with '_'"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Plugin loader
// ---------------------------------------------------------------------------

/// Compile and run a single plugin file against `env`, returning
/// the resulting [`PluginDeclaration`].  Failures come back as
/// fully rendered diagnostic strings -- compile errors via
/// `render_compile_error`, runtime errors via
/// `render_runtime_error` -- so a failed load surfaces the same
/// snippet-annotated output a user would see from `shingetsu run`.
///
/// Type-checking is enabled so the event registrar can validate
/// `lint.on(...)` handler signatures and an unknown event name
/// surfaces as an `event_name_unknown` warning independent of the
/// runtime Closed-policy check.
pub async fn load_plugin(env: &GlobalEnv, path: &Path) -> Result<PluginDeclaration, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read plugin file {}: {}", path.display(), e))?;
    let reg = registry(env);
    reg.begin_load(path.to_path_buf())
        .map_err(|e| e.to_string())?;

    let compile_opts = shingetsu_compiler::CompileOptions {
        debug_info: true,
        source_name: Arc::new(format!("@{}", path.display())),
        type_check: true,
    };
    let compiler = shingetsu_compiler::Compiler::new(compile_opts, env.global_type_map());

    let bc = match compiler.compile(&source).await {
        Ok(bc) => bc,
        Err(err) => {
            reg.abort_load();
            return Err(render_compile_error(&err, &source, RenderStyle::Plain));
        }
    };
    let func = bc.into_function();
    let task = crate::Task::new(env.clone(), func, crate::valuevec![]);
    match task.await {
        Ok(_) => reg.end_load().map_err(|e| {
            reg.abort_load();
            e.to_string()
        }),
        Err(rt_err) => {
            reg.abort_load();
            Err(render_runtime_error(&rt_err, RenderStyle::Plain))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn write_plugin(contents: &str) -> NamedTempFile {
        use std::io::Write;
        let mut file = NamedTempFile::new().expect("tempfile");
        file.write_all(contents.as_bytes()).expect("write");
        file.flush().expect("flush");
        file
    }

    /// Names of every event a callback is currently registered
    /// under in `env`'s callback registry.  Test-friendly summary
    /// that doesn't depend on `Function` being comparable.
    fn registered_event_names(env: &GlobalEnv) -> Vec<String> {
        let reg = callback_registry(env);
        let mut out: Vec<String> = Vec::new();
        for name in ["method_call", "function_call", "assign"] {
            if !reg.handlers(name.as_bytes()).is_empty() {
                out.push(name.to_string());
            }
        }
        out
    }

    /// Strip a varying tempfile path out of a rendered diagnostic
    /// so the snapshot string compares stably.
    fn normalize_path(s: String, path: &Path, placeholder: &str) -> String {
        s.replace(&path.display().to_string(), placeholder)
    }

    /// `lint.declare` and `lint.on` round-trip through the registry
    /// and the callback registry respectively.
    #[tokio::test]
    async fn load_minimal_plugin_records_declaration_and_callbacks() {
        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare {
    name = "demo",
    description = "demo plugin",
}
lint.on("method_call", function() end)
lint.on("function_call", function() end)
"#,
        );
        let decl = load_plugin(&env, plugin.path()).await.expect("load");
        let expected_decl = PluginDeclaration {
            name: "demo".into(),
            description: "demo plugin".into(),
            default_severity: Severity::Warning,
            sets: vec![],
            min_schema: None,
            source_path: plugin.path().to_path_buf(),
        };
        k9::assert_equal!(decl, expected_decl);
        let reg = registry(&env);
        k9::assert_equal!(reg.declarations(), vec![expected_decl]);
        k9::assert_equal!(
            registered_event_names(&env),
            vec!["method_call".to_string(), "function_call".to_string()]
        );
    }

    /// `lint.on` may appear before `lint.declare`: the registry
    /// commits the declaration at end-of-load, the callback registry
    /// has already accepted the handler under the closed-name check.
    #[tokio::test]
    async fn declare_after_on_is_harmless() {
        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.on("method_call", function() end)
lint.declare {
    name = "late_declare",
    description = "registration ordering",
}
"#,
        );
        let decl = load_plugin(&env, plugin.path()).await.expect("load");
        k9::assert_equal!(decl.name, "late_declare");
        k9::assert_equal!(
            registered_event_names(&env),
            vec!["method_call".to_string()]
        );
    }

    #[tokio::test]
    async fn duplicate_declare_in_same_file_errors() {
        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "first", description = "1" }
lint.declare { name = "second", description = "2" }
"#,
        );
        let err = load_plugin(&env, plugin.path())
            .await
            .expect_err("should fail");
        let err = normalize_path(err, plugin.path(), "<plugin>");
        k9::assert_equal!(
            err,
            concat!(
                r#"error: lint.declare called more than once in the same plugin file
 --> <plugin>:4:1
  |
4 | lint.declare { name = "second", description = "2" }
  | ^^^^^^^^^^^^ lint.declare called more than once in the same plugin file
stack traceback:"#,
                "\n\t<plugin>:4: in main chunk",
            )
        );
    }

    #[tokio::test]
    async fn missing_declare_errors() {
        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.on("method_call", function() end)
"#,
        );
        let err = load_plugin(&env, plugin.path())
            .await
            .expect_err("should fail");
        let err = normalize_path(err, plugin.path(), "<plugin>");
        k9::assert_equal!(
            err,
            "plugin file <plugin> never called `lint.declare {...}`"
        );
    }

    #[tokio::test]
    async fn invalid_lint_name_errors() {
        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "BadName", description = "x" }
"#,
        );
        let err = load_plugin(&env, plugin.path())
            .await
            .expect_err("should fail");
        let err = normalize_path(err, plugin.path(), "<plugin>");
        k9::assert_equal!(
            err,
            concat!(
                r#"error: bad argument #1 to 'declare' (validated name expected, got lint name 'BadName' must be snake_case ASCII (lowercase letters, digits, underscores))
 --> <plugin>:3:14
  |
3 | lint.declare { name = "BadName", description = "x" }
  |              ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ bad argument #1 to 'declare' (validated name expected, got lint name 'BadName' must be snake_case ASCII (lowercase letters, digits, underscores))
stack traceback:"#,
                "\n\t<plugin>:3: in main chunk",
            )
        );
    }

    /// Unknown event names are rejected by the callback registry's
    /// closed name policy with a fully rendered diagnostic carrying
    /// a did-you-mean suggestion.
    /// End-to-end smoke: a plugin registers a method_call handler
    /// that emits a `ctx:warn` with a message derived from the
    /// node, dispatch walks a tiny lint IR over `obj:foo()`, and
    /// the resulting diagnostic renders with the plugin's
    /// `project:<name>` lint id, the call expression as the
    /// anchor span, and the plugin's `default_severity` of warn.
    #[tokio::test]
    async fn method_call_event_fires_and_warn_collects_diagnostic() {
        use crate::diagnostic::render_warnings;
        use shingetsu_compiler::lint_ir;

        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("method_call", function(call, ctx)
    ctx:warn(call.span, "saw method " .. call.method)
end)
"#,
        );
        load_plugin(&env, plugin.path()).await.expect("load");

        // Build a tiny IR by parsing one line of source.  Using
        // the real lowering pass keeps this test honest -- a
        // future change to lint_ir lowering will surface here.
        let source_text = "obj:foo()";
        let ast = full_moon::parse(source_text).expect("parse");
        let lowered = lint_ir::lower::lower(&ast);
        k9::assert_equal!(lowered.unsupported, vec![]);

        let source_name = Arc::new("@test.lua".to_string());
        let diags = dispatch_chunk(&env, Arc::clone(&source_name), &lowered.chunk)
            .await
            .expect("dispatch");

        let rendered = render_warnings(&diags, source_text, RenderStyle::Plain);
        // The method-call span runs from the receiver start to
        // just past the opening `(`, so the caret covers 8 bytes
        // of `obj:foo()` -- not the trailing `)`.  Span
        // calculation for closing parens is a known minor
        // imprecision in the lowering pass; the diagnostic still
        // points at the right token.
        k9::assert_equal!(
            rendered,
            r#"warning[project:demo]: saw method foo
 --> test.lua:1:1
  |
1 | obj:foo()
  | ^^^^^^^^ saw method foo"#
        );
    }

    /// Same shape as the method_call smoke but for the
    /// function_call event.  Confirms the typed event payload
    /// (`lint_ir::FunctionCall`) round-trips through the userdata
    /// derive on the IR struct directly.
    #[tokio::test]
    async fn function_call_event_fires() {
        use crate::diagnostic::render_warnings;
        use shingetsu_compiler::lint_ir;

        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("function_call", function(call, ctx)
    ctx:warn(call.span, "saw function_call")
end)
"#,
        );
        load_plugin(&env, plugin.path()).await.expect("load");

        let source_text = "print(1)";
        let ast = full_moon::parse(source_text).expect("parse");
        let lowered = lint_ir::lower::lower(&ast);
        k9::assert_equal!(lowered.unsupported, vec![]);

        let source_name = Arc::new("@test.lua".to_string());
        let diags = dispatch_chunk(&env, Arc::clone(&source_name), &lowered.chunk)
            .await
            .expect("dispatch");

        let rendered = render_warnings(&diags, source_text, RenderStyle::Plain);
        k9::assert_equal!(
            rendered,
            r#"warning[project:demo]: saw function_call
 --> test.lua:1:1
  |
1 | print(1)
  | ^^^^^^^^ saw function_call"#
        );
    }

    /// Same shape as the method_call / function_call smokes but
    /// for the assign event.
    #[tokio::test]
    async fn assign_event_fires() {
        use crate::diagnostic::render_warnings;
        use shingetsu_compiler::lint_ir;

        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("assign", function(node, ctx)
    ctx:warn(node.span, "saw assign")
end)
"#,
        );
        load_plugin(&env, plugin.path()).await.expect("load");

        let source_text = "x = 1";
        let ast = full_moon::parse(source_text).expect("parse");
        let lowered = lint_ir::lower::lower(&ast);
        k9::assert_equal!(lowered.unsupported, vec![]);

        let source_name = Arc::new("@test.lua".to_string());
        let diags = dispatch_chunk(&env, Arc::clone(&source_name), &lowered.chunk)
            .await
            .expect("dispatch");

        let rendered = render_warnings(&diags, source_text, RenderStyle::Plain);
        k9::assert_equal!(
            rendered,
            r#"warning[project:demo]: saw assign
 --> test.lua:1:1
  |
1 | x = 1
  | ^^^^^ saw assign"#
        );
    }

    /// A plugin handler that raises an error during dispatch is
    /// caught and converted to a `Warning` diagnostic at the
    /// visited node's span; subsequent events still fire so a
    /// single buggy callback doesn't disable the rest of the
    /// walk.
    #[tokio::test]
    async fn handler_error_becomes_warning_and_walk_continues() {
        use crate::diagnostic::render_warnings;
        use shingetsu_compiler::lint_ir;

        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
-- The first call (`obj:bad()`) raises; the second (`obj:good()`)
-- should still fire and emit its own diagnostic.
lint.on("method_call", function(call, ctx)
    if call.method == "bad" then
        error("boom")
    else
        ctx:warn(call.span, "hi from " .. call.method)
    end
end)
"#,
        );
        load_plugin(&env, plugin.path()).await.expect("load");

        let source_text = "obj:bad() obj:good()";
        let ast = full_moon::parse(source_text).expect("parse");
        let lowered = lint_ir::lower::lower(&ast);
        k9::assert_equal!(lowered.unsupported, vec![]);

        let source_name = Arc::new("@test.lua".to_string());
        let diags = dispatch_chunk(&env, Arc::clone(&source_name), &lowered.chunk)
            .await
            .expect("dispatch");

        let rendered = render_warnings(&diags, source_text, RenderStyle::Plain);
        // The error message embeds the *plugin* file's path + line
        // (where `error("boom")` lives) -- not the user-source
        // location.  Normalize the tempfile path so the snapshot
        // is stable across runs.
        let rendered = rendered.replace(&plugin.path().display().to_string(), "<plugin>");
        k9::assert_equal!(
            rendered,
            r#"warning[project:demo]: lint plugin 'demo' handler raised: <plugin>:8: boom
 --> test.lua:1:1
  |
1 | obj:bad() obj:good()
  | ^^^^^^^^ lint plugin 'demo' handler raised: <plugin>:8: boom
warning[project:demo]: hi from good
 --> test.lua:1:11
  |
1 | obj:bad() obj:good()
  |           ^^^^^^^^^ hi from good"#
        );
    }

    #[tokio::test]
    async fn unknown_event_name_is_rejected() {
        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("function_callz", function() end)
"#,
        );
        let err = load_plugin(&env, plugin.path())
            .await
            .expect_err("should fail");
        let err = normalize_path(err, plugin.path(), "<plugin>");
        k9::assert_equal!(
            err,
            concat!(
                r#"error: error in 'callback': 'function_callz' is not a recognised event name. Did you mean `function_call`? Other alternatives are `assign`, `method_call`
 --> <plugin>:4:1
  |
4 | lint.on("function_callz", function() end)
  | ^^^^^^^ error in 'callback': 'function_callz' is not a recognised event name. Did you mean `function_call`? Other alternatives are `assign`, `method_call`
stack traceback:"#,
                "\n\t<plugin>:4: in main chunk",
            )
        );
    }
}
