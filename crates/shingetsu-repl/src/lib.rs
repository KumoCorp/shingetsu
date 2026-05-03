use std::sync::Arc;

use annotate_snippets::{AnnotationKind, Group, Level, Renderer, Snippet};
use full_moon::tokenizer::TokenKind;
use shingetsu::diagnostic::{render_compile_error, render_runtime_error, RenderStyle};
use shingetsu::{pretty_print, valuevec, Function, GlobalEnv, PrettyPrintConfig, Task, Value};
use shingetsu_compiler::{Bytecode, CompileOptions, Compiler};

/// The syntactic status of the current REPL input.
pub enum ParseStatus {
    /// Input is syntactically complete (no errors).
    Complete,
    /// Input is incomplete — the parser reached EOF expecting more.
    Incomplete,
    /// Input has a genuine syntax error; the message is ready to display.
    Error(String),
}

/// Analyse the combined input (`pending` already-submitted lines plus the
/// live `current_line` being typed) and return its [`ParseStatus`].
///
/// Intended for use by REPL front-ends to drive live preview output.
pub fn parse_status(pending: &str, current_line: &str) -> ParseStatus {
    let combined = format!("{pending}{current_line}");
    if combined.trim().is_empty() {
        return ParseStatus::Complete;
    }
    let lua_version = full_moon::LuaVersion::lua54().with_luau();
    let result = full_moon::parse_fallible(&combined, lua_version);
    let errors = result.errors();
    if errors.is_empty() {
        return ParseStatus::Complete;
    }
    if has_eof_error(errors) {
        return ParseStatus::Incomplete;
    }
    // Statement form failed — try expression form, mirroring submit_line.
    let expr_source = format!("return ({combined})");
    let expr_result = full_moon::parse_fallible(&expr_source, lua_version);
    let expr_errors = expr_result.errors();
    if expr_errors.is_empty() {
        return ParseStatus::Complete;
    }
    if has_eof_error(expr_errors) {
        return ParseStatus::Incomplete;
    }
    // Both forms fail: report the statement-form error.
    ParseStatus::Error(render_syntax_error(&combined, &errors[0]))
}

fn has_eof_error(errors: &[full_moon::Error]) -> bool {
    errors.iter().any(|e| match e {
        full_moon::Error::AstError(ast_err) => {
            ast_err.token().token_type().kind() == TokenKind::Eof
        }
        full_moon::Error::TokenizerError(_) => false,
    })
}

/// Render a full_moon syntax error as a short annotate-snippets diagnostic
/// with a source line and caret, suitable for REPL preview output.
fn render_syntax_error(source: &str, error: &full_moon::Error) -> String {
    match error {
        full_moon::Error::AstError(ast_err) => {
            let start = ast_err.token().start_position().bytes() as usize;
            let end = (ast_err.token().end_position().bytes() as usize)
                .max(start + 1)
                .min(source.len());
            // Extract the short message: strip the "error occurred while
            // creating ast: " prefix and the verbose position suffix.
            let full = format!("{ast_err}");
            let trimmed = full
                .strip_prefix("error occurred while creating ast: ")
                .unwrap_or(&full);
            let msg = trimmed
                .split(". (starting from")
                .next()
                .unwrap_or(trimmed)
                .trim_end_matches('.');
            let snippet = Snippet::source(source)
                .path("<repl>")
                .annotation(AnnotationKind::Primary.span(start..end).label(msg));
            let group = Group::with_title(Level::ERROR.primary_title(msg)).element(snippet);
            Renderer::styled().render(&[group])
        }
        full_moon::Error::TokenizerError(tok_err) => {
            let msg = format!("{}", tok_err.error());
            let (start_pos, end_pos) = tok_err.range();
            let start = start_pos.bytes();
            let end = end_pos.bytes().max(start + 1).min(source.len());
            let snippet = Snippet::source(source)
                .path("<repl>")
                .annotation(AnnotationKind::Primary.span(start..end).label(&msg));
            let group = Group::with_title(Level::ERROR.primary_title(&msg)).element(snippet);
            Renderer::styled().render(&[group])
        }
    }
}

/// The outcome of submitting a line to the REPL.
pub enum SubmitOutcome {
    /// Input was syntactically incomplete; the caller should show a continuation prompt.
    Incomplete,
    /// The chunk compiled and ran; zero or more pretty-printed result values.
    Values(Vec<String>),
    /// A compile or runtime error; pre-rendered string ready to display.
    Error(String),
    /// `os.exit()` was called. The caller should exit with the given code.
    /// If `close` is true, `__gc` finalizers should be run first via
    /// [`GlobalEnv::dispose`](shingetsu::GlobalEnv::dispose).
    Exit { code: i32, close: bool },
}

/// An I/O-agnostic REPL core.
///
/// Callers feed lines in via [`submit_line`](Repl::submit_line) and act on
/// the returned [`SubmitOutcome`]. All terminal interaction is left to the
/// caller.
pub struct Repl {
    env: GlobalEnv,
    pending: String,
    print_config: PrettyPrintConfig,
}

impl Repl {
    pub fn new(env: GlobalEnv) -> Self {
        Self {
            env,
            pending: String::new(),
            print_config: PrettyPrintConfig::default(),
        }
    }

    /// Override the pretty-print config used when displaying return values.
    pub fn set_print_config(&mut self, config: PrettyPrintConfig) {
        self.print_config = config;
    }

    /// Current prompt string: `"> "` for fresh input, `">> "` for continuation.
    pub fn prompt(&self) -> &str {
        if self.pending.is_empty() {
            "> "
        } else {
            ">> "
        }
    }

    /// Accumulated incomplete lines (for preview display by the caller).
    pub fn pending_lines(&self) -> &str {
        &self.pending
    }

    /// Returns a reference to the shared [`GlobalEnv`].
    pub fn env(&self) -> &GlobalEnv {
        &self.env
    }

    /// Submit one line of input from the user.
    ///
    /// Appends the line to the pending buffer and determines whether the
    /// accumulated input is syntactically complete. If complete (or forced
    /// by a blank line), the chunk is compiled and executed.
    pub async fn submit_line(&mut self, line: &str) -> SubmitOutcome {
        self.pending.push_str(line);
        self.pending.push('\n');

        // A blank line while pending is non-empty force-flushes so the user
        // sees the error rather than looping forever.
        let force_flush = line.trim().is_empty()
            && self.pending.trim().is_empty() == false
            && self.pending.trim() != line.trim();

        if !force_flush && is_incomplete(&self.pending) {
            return SubmitOutcome::Incomplete;
        }

        let source = std::mem::take(&mut self.pending);
        self.run_chunk(&source).await
    }

    async fn run_chunk(&self, source: &str) -> SubmitOutcome {
        let style = RenderStyle::Plain;

        // Try wrapping as an expression first so the REPL can print values.
        let expr_source = format!("return ({source})");
        if let Some(outcome) = self
            .try_compile_and_run(&expr_source, source, style, true)
            .await
        {
            return outcome;
        }

        // Fall back to running as a statement.
        match self.compile(source, style).await {
            Err(msg) => SubmitOutcome::Error(msg),
            Ok(bytecode) => self.execute(bytecode, source, style).await,
        }
    }

    /// Attempt to compile and run `attempt_source`; return `None` if it fails
    /// to compile (so the caller can fall through to the statement path).
    async fn try_compile_and_run(
        &self,
        attempt_source: &str,
        _original_source: &str,
        style: RenderStyle,
        _is_expr: bool,
    ) -> Option<SubmitOutcome> {
        let bytecode = self.compile(attempt_source, style).await.ok()?;
        Some(self.execute(bytecode, attempt_source, style).await)
    }

    async fn compile(&self, source: &str, style: RenderStyle) -> Result<Bytecode, String> {
        let opts = CompileOptions {
            debug_info: false,
            source_name: Arc::new("<repl>".to_string()),
            type_check: false,
        };
        let compiler = Compiler::new(opts, self.env.global_type_map());
        compiler
            .compile(source)
            .await
            .map_err(|e| render_compile_error(&e, source, style))
    }

    async fn execute(
        &self,
        bytecode: Bytecode,
        _source: &str,
        style: RenderStyle,
    ) -> SubmitOutcome {
        let func = Function::lua(bytecode.top_level, vec![]);
        let task = Task::new(self.env.clone(), func, valuevec![]);
        match task.await {
            Ok(values) => {
                let rendered: Vec<String> = values
                    .iter()
                    .filter(|v| !matches!(v, Value::Nil))
                    .map(|v| pretty_print(v, &self.print_config))
                    .collect();
                SubmitOutcome::Values(rendered)
            }
            Err(re) if matches!(re.error, shingetsu::VmError::ExitRequested { .. }) => {
                match re.error {
                    shingetsu::VmError::ExitRequested { code, close } => {
                        SubmitOutcome::Exit { code, close }
                    }
                    _ => unreachable!(),
                }
            }
            Err(re) => SubmitOutcome::Error(render_runtime_error(&re, style)),
        }
    }
}

/// Returns `true` if `source` looks syntactically incomplete (i.e. the parser
/// reached EOF while still expecting more input).
fn is_incomplete(source: &str) -> bool {
    let lua_version = full_moon::LuaVersion::lua54().with_luau();
    let result = full_moon::parse_fallible(source, lua_version);
    let errors = result.errors();
    !errors.is_empty() && has_eof_error(errors)
}

/// Query tab-completion candidates for the current cursor position.
///
/// Returns the byte range in `line` that the candidates should replace,
/// and the list of candidate strings sorted alphabetically.
pub fn completions(
    env: &GlobalEnv,
    line: &str,
    cursor_pos: usize,
) -> (std::ops::Range<usize>, Vec<String>) {
    let pos = cursor_pos.min(line.len());
    let prefix_start = line[..pos]
        .rfind(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(0);
    let prefix = &line[prefix_start..pos];

    // Check for a table access: `obj.field` or `obj:method`.
    let before_prefix = line[..prefix_start].trim_end_matches(|c| c == '.' || c == ':');
    let separator = line[before_prefix.len()..prefix_start]
        .chars()
        .next()
        .unwrap_or(' ');

    let candidates = if matches!(separator, '.' | ':') {
        // Tier 2: field / method completion on a table global.
        let obj_start = before_prefix
            .rfind(|c: char| !c.is_alphanumeric() && c != '_')
            .map(|i| i + 1)
            .unwrap_or(0);
        let obj_name = &before_prefix[obj_start..];
        table_field_completions(env, obj_name, prefix)
    } else {
        // Tier 1: global name completion.
        global_completions(env, prefix)
    };

    (prefix_start..pos, candidates)
}

fn global_completions(env: &GlobalEnv, prefix: &str) -> Vec<String> {
    let table = env.env_table();
    collect_string_keys(&table, prefix)
}

fn table_field_completions(env: &GlobalEnv, obj_name: &str, prefix: &str) -> Vec<String> {
    match env.get_global(obj_name) {
        Some(Value::Table(t)) => collect_string_keys(&t, prefix),
        _ => global_completions(env, prefix),
    }
}

fn collect_string_keys(table: &shingetsu::Table, prefix: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut key = Value::Nil;
    loop {
        match table.next(&key) {
            Ok(Some((k, _))) => {
                if let Value::String(ref s) = k {
                    if let Ok(name) = std::str::from_utf8(s.as_ref()) {
                        if name.starts_with(prefix) {
                            candidates.push(name.to_string());
                        }
                    }
                }
                key = k;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    candidates.sort();
    candidates
}
