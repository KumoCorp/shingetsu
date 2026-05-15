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
mod orchestrator;

pub use dispatch::dispatch_chunk;
pub use node::LintContext;
pub use orchestrator::{LoadedPlugin, LoadedPlugins};
pub use shingetsu_compiler::lint_ir::{
    Assign, Branch, Expr, FunctionCall, MethodCall, Param, Stmt, TableEntry,
};

use crate::diagnostic::{render_compile_error, render_runtime_error, RenderStyle};
use crate::sync::RwLock;
use crate::{
    declare_event, register_libs, CallContext, Function, GlobalEnv, Libraries, Ud, Value, VmError,
};
use shingetsu_compiler::{Severity, SourceLocation};
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
    /// Source location of the `lint.declare {...}` call.  Captured
    /// from the [`crate::CallContext`] when the declare runs;
    /// `None` only when the loader couldn't resolve the calling
    /// Lua frame's source mapping (shouldn't happen in practice).
    /// Used by the orchestrator to anchor cross-plugin
    /// duplicate-name diagnostics on both conflicting declarations.
    pub declare_call_site: Option<SourceLocation>,
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

// Statement events (fire with Ud<Stmt> so plugins see the same IR
// struct for all statement kinds; `stmt.kind` disambiguates).
declare_event! {
    /// Fired before every statement's kind-specific event.  Fires
    /// for every statement without exception.
    pub static STATEMENT_EVENT: Multiple(
        "statement",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static EXPR_STATEMENT_EVENT: Multiple(
        "expr_statement",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static LOCAL_ASSIGN_EVENT: Multiple(
        "local_assign",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static LOCAL_FUNCTION_EVENT: Multiple(
        "local_function",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static FUNCTION_DECL_EVENT: Multiple(
        "function_decl",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static GLOBAL_DECL_EVENT: Multiple(
        "global_decl",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static IF_EVENT: Multiple(
        "if",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static WHILE_EVENT: Multiple(
        "while",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static REPEAT_EVENT: Multiple(
        "repeat",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static NUMERIC_FOR_EVENT: Multiple(
        "numeric_for",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static GENERIC_FOR_EVENT: Multiple(
        "generic_for",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static DO_BLOCK_EVENT: Multiple(
        "do_block",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static RETURN_EVENT: Multiple(
        "return",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static BREAK_EVENT: Multiple(
        "break",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static CONTINUE_EVENT: Multiple(
        "continue",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static GOTO_EVENT: Multiple(
        "goto",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static LABEL_EVENT: Multiple(
        "label",
        stmt: Ud<Stmt>,
        ctx: Ud<LintContext>,
    ) -> ();
}

// Chunk-level events (no separate node; ctx is the only arg).
declare_event! {
    /// Fired once at the start of every chunk under analysis,
    /// before any statements are visited.  Use it to reset
    /// per-file plugin state.
    pub static CHUNK_BEGIN_EVENT: Multiple(
        "chunk_begin",
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    /// Fired once at the end of every chunk under analysis, after
    /// all statements have been visited.  Use it to emit
    /// chunk-level diagnostics gathered during the walk.
    pub static CHUNK_END_EVENT: Multiple(
        "chunk_end",
        ctx: Ud<LintContext>,
    ) -> ();
}

// Expression events (fire with Ud<Expr> so plugins read all fields
// through the single Expr userdata type).
declare_event! {
    pub static BINOP_EVENT: Multiple(
        "binop",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static UNOP_EVENT: Multiple(
        "unop",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    /// Fired for every name reference (local or global, read or
    /// write context).
    pub static NAME_EVENT: Multiple(
        "name",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    /// Fired for global name references in a read (value) position.
    /// Subset of `name`; `expr.is_global` is always `true`.
    pub static GLOBAL_READ_EVENT: Multiple(
        "global_read",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    /// Fired for global name references in a write (assignment
    /// target) position.  Subset of `name`; `expr.is_global` is
    /// always `true`.
    pub static GLOBAL_WRITE_EVENT: Multiple(
        "global_write",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static STRING_LITERAL_EVENT: Multiple(
        "string_literal",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static NUMBER_LITERAL_EVENT: Multiple(
        "number_literal",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    /// Fired for Luau interpolated strings (`` `hello {name}` ``).
    pub static INTERP_STRING_EVENT: Multiple(
        "interp_string",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static TABLE_CONSTRUCTOR_EVENT: Multiple(
        "table_constructor",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    pub static FUNCTION_EXPR_EVENT: Multiple(
        "function_expr",
        expr: Ud<Expr>,
        ctx: Ud<LintContext>,
    ) -> ();
}
declare_event! {
    /// Specialised `function_call` for `require("...")` patterns.
    /// The callee is the global name `require`; plugins can rely
    /// on `node.args[1].kind == "string_literal"` for the simple
    /// case.
    pub static REQUIRE_EVENT: Multiple(
        "require",
        node: Ud<FunctionCall>,
        ctx: Ud<LintContext>,
    ) -> ();
}

/// The current schema version of the lint IR and plugin API.  Exposed
/// as `shingetsu.lint.SCHEMA_VERSION` so plugins can check host
/// compatibility at load time.  `lint.declare { min_schema = N }`
/// causes a plugin to refuse to load against an older host.
pub const SCHEMA_VERSION: u32 = 1;

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
    env.register_userdata_type(Expr::userdata_type());
    env.register_userdata_type(TableEntry::userdata_type());
    env.register_userdata_type(Param::userdata_type());
    env.register_userdata_type(Branch::userdata_type());
    env.register_userdata_type(Stmt::userdata_type());
    env.register_userdata_type(MethodCall::userdata_type());
    env.register_userdata_type(FunctionCall::userdata_type());
    env.register_userdata_type(Assign::userdata_type());
    env.register_userdata_type(node::LintContext::userdata_type());
    env.declare_event_registrar("shingetsu.lint.on");

    METHOD_CALL_EVENT.register(env);
    FUNCTION_CALL_EVENT.register(env);
    ASSIGN_EVENT.register(env);
    STATEMENT_EVENT.register(env);
    EXPR_STATEMENT_EVENT.register(env);
    LOCAL_ASSIGN_EVENT.register(env);
    LOCAL_FUNCTION_EVENT.register(env);
    FUNCTION_DECL_EVENT.register(env);
    GLOBAL_DECL_EVENT.register(env);
    IF_EVENT.register(env);
    WHILE_EVENT.register(env);
    REPEAT_EVENT.register(env);
    NUMERIC_FOR_EVENT.register(env);
    GENERIC_FOR_EVENT.register(env);
    DO_BLOCK_EVENT.register(env);
    RETURN_EVENT.register(env);
    BREAK_EVENT.register(env);
    CONTINUE_EVENT.register(env);
    GOTO_EVENT.register(env);
    LABEL_EVENT.register(env);
    CHUNK_BEGIN_EVENT.register(env);
    CHUNK_END_EVENT.register(env);
    BINOP_EVENT.register(env);
    UNOP_EVENT.register(env);
    NAME_EVENT.register(env);
    GLOBAL_READ_EVENT.register(env);
    GLOBAL_WRITE_EVENT.register(env);
    STRING_LITERAL_EVENT.register(env);
    NUMBER_LITERAL_EVENT.register(env);
    INTERP_STRING_EVENT.register(env);
    TABLE_CONSTRUCTOR_EVENT.register(env);
    FUNCTION_EXPR_EVENT.register(env);
    REQUIRE_EVENT.register(env);

    let mut tm = env.global_type_map();
    METHOD_CALL_EVENT.register_compile_type(&mut tm);
    FUNCTION_CALL_EVENT.register_compile_type(&mut tm);
    ASSIGN_EVENT.register_compile_type(&mut tm);
    STATEMENT_EVENT.register_compile_type(&mut tm);
    EXPR_STATEMENT_EVENT.register_compile_type(&mut tm);
    LOCAL_ASSIGN_EVENT.register_compile_type(&mut tm);
    LOCAL_FUNCTION_EVENT.register_compile_type(&mut tm);
    FUNCTION_DECL_EVENT.register_compile_type(&mut tm);
    GLOBAL_DECL_EVENT.register_compile_type(&mut tm);
    IF_EVENT.register_compile_type(&mut tm);
    WHILE_EVENT.register_compile_type(&mut tm);
    REPEAT_EVENT.register_compile_type(&mut tm);
    NUMERIC_FOR_EVENT.register_compile_type(&mut tm);
    GENERIC_FOR_EVENT.register_compile_type(&mut tm);
    DO_BLOCK_EVENT.register_compile_type(&mut tm);
    RETURN_EVENT.register_compile_type(&mut tm);
    BREAK_EVENT.register_compile_type(&mut tm);
    CONTINUE_EVENT.register_compile_type(&mut tm);
    GOTO_EVENT.register_compile_type(&mut tm);
    LABEL_EVENT.register_compile_type(&mut tm);
    CHUNK_BEGIN_EVENT.register_compile_type(&mut tm);
    CHUNK_END_EVENT.register_compile_type(&mut tm);
    BINOP_EVENT.register_compile_type(&mut tm);
    UNOP_EVENT.register_compile_type(&mut tm);
    NAME_EVENT.register_compile_type(&mut tm);
    GLOBAL_READ_EVENT.register_compile_type(&mut tm);
    GLOBAL_WRITE_EVENT.register_compile_type(&mut tm);
    STRING_LITERAL_EVENT.register_compile_type(&mut tm);
    NUMBER_LITERAL_EVENT.register_compile_type(&mut tm);
    INTERP_STRING_EVENT.register_compile_type(&mut tm);
    TABLE_CONSTRUCTOR_EVENT.register_compile_type(&mut tm);
    FUNCTION_EXPR_EVENT.register_compile_type(&mut tm);
    REQUIRE_EVENT.register_compile_type(&mut tm);

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

    /// Schema version of the lint IR and plugin API provided by
    /// this host.  `lint.declare { min_schema = N }` refuses to
    /// load against a host that provides a lower version.
    ///
    /// Plugins read this as `lint.schema_version` (snake_case, per
    /// Lua convention: `math.pi`, `math.huge`, ...).
    #[field]
    fn schema_version() -> u32 {
        super::SCHEMA_VERSION
    }

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
        // The Lua frame at the top of the stack is the one
        // running `lint.declare {...}`; its current-instruction
        // location is the call site we anchor cross-plugin
        // duplicate-name diagnostics on.
        let declare_call_site = ctx
            .call_stack()
            .frames_bottom_up()
            .last()
            .and_then(|f| f.source_location())
            .map(SourceLocation::from);
        let decl = PluginDeclaration {
            name: args.name,
            description: args.description,
            default_severity: args.default_severity,
            sets: args.sets,
            min_schema: args.min_schema,
            source_path: path,
            declare_call_site,
        };
        if let Some(min) = decl.min_schema {
            if min > super::SCHEMA_VERSION {
                return Err(runtime_error(format!(
                    "plugin '{}' requires schema version {min} but this host \
                     provides version {}",
                    super::SCHEMA_VERSION,
                    decl.name,
                )));
            }
        }
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
    load_plugin_with_source(env, path, &source).await
}

/// Variant of [`load_plugin`] that reuses an already-loaded source
/// string.  The orchestrator uses this to avoid re-reading the
/// plugin file when it already has the text on hand for rendering
/// cross-plugin duplicate-name diagnostics.
pub async fn load_plugin_with_source(
    env: &GlobalEnv,
    path: &Path,
    source: &str,
) -> Result<PluginDeclaration, String> {
    let reg = registry(env);
    reg.begin_load(path.to_path_buf())
        .map_err(|e| e.to_string())?;

    let compile_opts = shingetsu_compiler::CompileOptions {
        debug_info: true,
        source_name: Arc::new(format!("@{}", path.display())),
        type_check: true,
    };
    let compiler = shingetsu_compiler::Compiler::new(compile_opts, env.global_type_map());

    let bc = match compiler.compile(source).await {
        Ok(bc) => bc,
        Err(err) => {
            reg.abort_load();
            return Err(render_compile_error(&err, source, RenderStyle::Plain));
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

    /// Load `plugin_src` as a plugin and `expect_err` the load --
    /// the tempfile path is normalized to `<plugin>` before the
    /// rendered diagnostic is returned.  For tests that assert on
    /// load-time failures (compile errors, runtime errors from
    /// `lint.declare`, etc.).
    async fn expect_load_error(plugin_src: &str) -> String {
        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(plugin_src);
        let err = load_plugin(&env, plugin.path())
            .await
            .expect_err("should fail");
        normalize_path(err, plugin.path(), "<plugin>")
    }

    /// Load `plugin_src`, parse + lower `target_src` as the chunk
    /// under analysis, dispatch every event, and return the
    /// rendered warnings (plain style).  The plugin file's path
    /// is normalized to `<plugin>` so plugin-runtime references
    /// inside the rendered output compare stably across runs.
    async fn run_plugin_against(plugin_src: &str, target_src: &str) -> String {
        use crate::diagnostic::render_warnings;
        use shingetsu_compiler::lint_ir;

        let env = new_plugin_env().expect("new env");
        let plugin = write_plugin(plugin_src);
        load_plugin(&env, plugin.path()).await.expect("load");
        let ast = full_moon::parse(target_src).expect("parse");
        let lowered = lint_ir::lower::lower(&ast);
        k9::assert_equal!(lowered.unsupported, vec![]);
        let source_name = Arc::new("@test.lua".to_string());
        let diags = dispatch_chunk(&env, Arc::clone(&source_name), &lowered.chunk)
            .await
            .expect("dispatch");
        let rendered = render_warnings(&diags, target_src, RenderStyle::Plain);
        normalize_path(rendered, plugin.path(), "<plugin>")
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
            // Captured from CallContext; the value depends on the
            // plugin file's tempfile path, so compare it as-is.
            declare_call_site: decl.declare_call_site.clone(),
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
        // The method-call span runs from the receiver start to
        // just past the opening `(`, so the caret covers 8 bytes
        // of `obj:foo()` -- not the trailing `)`.  Span
        // calculation for closing parens is a known minor
        // imprecision in the lowering pass; the diagnostic still
        // points at the right token.
        let rendered = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("method_call", function(call, ctx)
    ctx:warn(call.span, "saw method " .. call.method)
end)
"#,
            "obj:foo()",
        )
        .await;
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
        let rendered = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("function_call", function(call, ctx)
    ctx:warn(call.span, "saw function_call")
end)
"#,
            "print(1)",
        )
        .await;
        k9::assert_equal!(
            rendered,
            r#"warning[project:demo]: saw function_call
 --> test.lua:1:1
  |
1 | print(1)
  | ^^^^^^^^ saw function_call"#
        );
    }

    /// A `function_call` fired from inside a `---`-doc-commented
    /// `local Foo = mod.record(...)` sees the enclosing
    /// statement's doc text via `call.doc_comment`.  Validates
    /// the doc-context threading from walk_stmt -> walk_expr.
    #[tokio::test]
    async fn function_call_inherits_enclosing_doc_comment() {
        let rendered = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("function_call", function(call, ctx)
    local doc = call.doc_comment
    if doc then
        ctx:warn(call.span, "doc=" .. doc)
    end
end)
"#,
            "--- hello\nlocal x = f()",
        )
        .await;
        k9::assert_equal!(
            rendered,
            r#"warning[project:demo]: doc=hello
 --> test.lua:2:11
  |
2 | local x = f()
  |           ^^ doc=hello"#
        );
    }

    /// Same shape as the method_call / function_call smokes but
    /// for the assign event.
    #[tokio::test]
    async fn assign_event_fires() {
        let rendered = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("assign", function(node, ctx)
    ctx:warn(node.span, "saw assign")
end)
"#,
            "x = 1",
        )
        .await;
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
    /// walk.  The plugin file's own path appears in the error
    /// message (it's where `error("boom")` lives); the helper
    /// normalizes it to `<plugin>` so the snapshot is stable.
    #[tokio::test]
    async fn handler_error_becomes_warning_and_walk_continues() {
        let rendered = run_plugin_against(
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
            "obj:bad() obj:good()",
        )
        .await;
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

    /// kumomta_set_meta-style lint: visit every `:set_meta(key,
    /// ...)` method call, check the first arg is a string literal
    /// whose value is either in a whitelist or has the `x_`
    /// prefix.  Exercises arg access (`call.args[1]`), kind
    /// discrimination on `Expr`, `Expr.string_value`, Luau
    /// interpolated strings in the diagnostic message, and the
    /// optional `help` argument to `ctx:warn`.
    #[tokio::test]
    async fn kumomta_set_meta_lint() {
        let rendered = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "kumomta_set_meta", description = "meta key check" }

local KNOWN_META = { queue = true, routing = true }

lint.on("method_call", function(call, ctx)
    if call.method ~= "set_meta" then return end
    local key = call.args[1]
    if not key or key.kind ~= "string_literal" then return end
    local v = key.string_value
    if KNOWN_META[v] then return end
    if v:starts_with("x_") then return end
    ctx:warn(
        key.span,
        `metadata key "{v}" is not pre-defined and may collide with future keys`,
        "prefix the key with 'x_' to avoid collision"
    )
end)
"#,
            r#"msg:set_meta("queue", 1) msg:set_meta("bogus", 2) msg:set_meta("x_my", 3)"#,
        )
        .await;
        k9::assert_equal!(
            rendered,
            r#"warning[project:kumomta_set_meta]: metadata key "bogus" is not pre-defined and may collide with future keys
 --> test.lua:1:39
  |
1 | msg:set_meta("queue", 1) msg:set_meta("bogus", 2) msg:set_meta("x_my", 3)
  |                                       ^^^^^^^ metadata key "bogus" is not pre-defined and may collide with future keys
  |
help: prefix the key with 'x_' to avoid collision"#
        );
    }

    /// kumomta_record_doc_matches_runtime-style lint: visit
    /// `mod.record(name, { fields })` calls, parse `@field`
    /// tags from the doc-commented enclosing statement, and warn
    /// when the runtime table is missing one of the documented
    /// fields.  Exercises function_call.args, Expr.kind dispatch,
    /// table-constructor `entries` iteration, and named-entry
    /// `name` access.
    #[tokio::test]
    async fn kumomta_record_doc_matches_runtime_lint() {
        let rendered = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "kumomta_record_doc_matches", description = "..." }

local function declared_fields(doc)
    local names = {}
    for line in doc:gmatch("[^\n]+") do
        local name = line:match("^@field%s+(%S+)")
        if name then
            names[#names + 1] = name
        end
    end
    return names
end

lint.on("function_call", function(call, ctx)
    local doc = call.doc_comment
    if not doc then return end
    local declared = declared_fields(doc)
    if #declared == 0 then return end
    local tbl = call.args[2]
    if not tbl or tbl.kind ~= "table_constructor" then return end
    local present = {}
    for _, entry in ipairs(tbl.entries) do
        if entry.kind == "named" then
            present[entry.name] = true
        end
    end
    for _, field in ipairs(declared) do
        if not present[field] then
            ctx:warn(
                tbl.span,
                `@field "{field}" is missing from the record table`,
                "add the field to the constructor, or remove the @field tag"
            )
        end
    end
end)
"#,
            r#"--- @class Worker
--- @field name string
local Worker = mod.record("Worker", { naem = "string" })"#,
        )
        .await;
        k9::assert_equal!(
            rendered,
            concat!(
                r#"warning[project:kumomta_record_doc_matches]: @field "name" is missing from the record table
 --> test.lua:3:37
  |
3 | local Worker = mod.record("Worker", { naem = "string" })
  |                                     "#,
                "^^^^^^^^^^^^^^^^^^^",
                r#" @field "name" is missing from the record table
  |
help: add the field to the constructor, or remove the @field tag"#,
            )
        );
    }

    #[tokio::test]
    async fn schema_version_exposed_on_module() {
        let diags = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "schema_ver", description = "d" }
lint.on("return", function(stmt, ctx)
    local v = lint.schema_version
    if type(v) ~= "number" then
        ctx:warn(stmt.span, "SCHEMA_VERSION is not a number: " .. tostring(v))
    end
end)
"#,
            "return nil",
        )
        .await;
        k9::assert_equal!(diags, "");
    }

    #[tokio::test]
    async fn min_schema_too_high_is_load_error() {
        let src = format!(
            "local lint = require(\"shingetsu.lint\")\n\
             lint.declare {{ name = \"demo\", description = \"d\", \
             min_schema = {} }}",
            SCHEMA_VERSION + 1,
        );
        let err = expect_load_error(&src).await;
        assert!(
            err.contains("requires schema version"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn min_schema_at_host_version_is_ok() {
        let src = format!(
            "local lint = require(\"shingetsu.lint\")\n\
             lint.declare {{ name = \"demo\", description = \"d\", \
             min_schema = {} }}",
            SCHEMA_VERSION,
        );
        let err = load_plugin(
            &new_plugin_env().unwrap(),
            &write_plugin(&src).path().to_path_buf(),
        )
        .await;
        assert!(err.is_ok(), "{err:?}");
    }

    #[tokio::test]
    async fn new_events_are_registered() {
        let env = new_plugin_env().unwrap();
        let all_events = [
            "chunk_begin",
            "chunk_end",
            "statement",
            "expr_statement",
            "local_assign",
            "local_function",
            "function_decl",
            "global_decl",
            "if",
            "while",
            "repeat",
            "numeric_for",
            "generic_for",
            "do_block",
            "return",
            "break",
            "continue",
            "goto",
            "label",
            "binop",
            "unop",
            "name",
            "global_read",
            "global_write",
            "string_literal",
            "number_literal",
            "interp_string",
            "table_constructor",
            "function_expr",
            "require",
        ];
        for event in all_events {
            let src = format!(
                "local lint = require(\"shingetsu.lint\")\n\
                 lint.declare {{ name = \"demo{event}\", description = \"d\" }}\n\
                 lint.on(\"{event}\", function() end)"
            );
            let result = load_plugin(
                &new_plugin_env().unwrap(),
                &write_plugin(&src).path().to_path_buf(),
            )
            .await;
            assert!(
                result.is_ok(),
                "event '{event}' registration failed: {:?}",
                result
            );
        }
    }

    #[tokio::test]
    async fn ctx_is_same_line_matches_span_lines() {
        let diags = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "same_line", description = "d" }
lint.on("binop", function(expr, ctx)
    local same = ctx:is_same_line(expr.op_span, expr.span)
    if not same then
        ctx:warn(expr.span, "op and binop span should share a line")
    end
end)
"#,
            "local x = 1 + 2",
        )
        .await;
        k9::assert_equal!(diags, "");
    }

    #[tokio::test]
    async fn ctx_constant_value_returns_literal_value() {
        let diags = run_plugin_against(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "const_val", description = "d" }
lint.on("string_literal", function(expr, ctx)
    local v = ctx:constant_value(expr)
    if v ~= "hello" then
        ctx:warn(expr:span(), "expected 'hello' but got: " .. tostring(v))
    end
end)
"#,
            r#"local x = "hello""#,
        )
        .await;
        k9::assert_equal!(diags, "");
    }

    #[tokio::test]
    async fn unknown_event_name_is_rejected() {
        let err = expect_load_error(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "demo", description = "d" }
lint.on("function_callz", function() end)
"#,
        )
        .await;
        k9::assert_equal!(
            err,
            concat!(
                r#"error: error in 'callback': 'function_callz' is not a recognised event name. Did you mean one of `function_call`, `function_decl`, `function_expr`? There are too many alternatives to list here; consult the documentation!
 --> <plugin>:4:1
  |
4 | lint.on("function_callz", function() end)
  | ^^^^^^^ error in 'callback': 'function_callz' is not a recognised event name. Did you mean one of `function_call`, `function_decl`, `function_expr`? There are too many alternatives to list here; consult the documentation!
stack traceback:"#,
                "\n\t<plugin>:4: in main chunk",
            )
        );
    }
}
