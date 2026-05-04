use std::sync::Arc;

use annotate_snippets::{AnnotationKind, Group, Level, Renderer, Snippet};
use bstr::ByteSlice;
use full_moon::tokenizer::TokenKind;
use shingetsu::diagnostic::{render_compile_error, render_runtime_error, RenderStyle};
use shingetsu::types::{infer_type_from_value, FieldKind, LuaType, ModuleType, TableLuaType};
use shingetsu::{pretty_print, valuevec, Function, GlobalEnv, PrettyPrintConfig, Task, Value};
use shingetsu_compiler::{locals_at_cursor, Bytecode, CompileOptions, Compiler};

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
    let trimmed = combined.trim_end();
    if trimmed.is_empty() {
        return ParseStatus::Complete;
    }
    // A `.` or `:` at the end of the input is always a continuation —
    // there's no valid Lua program that terminates with one. full_moon
    // sometimes reports this as `unexpected token "."` (when the
    // preceding expression is parsed as a complete statement, e.g.
    // `require('table').`), which would otherwise show up as an error
    // in the live preview. Treat it as Incomplete instead.
    if trimmed.ends_with('.') || trimmed.ends_with(':') {
        return ParseStatus::Incomplete;
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
/// `pending` is the multi-line continuation buffer that has already been
/// submitted (see [`Repl::pending_lines`]) but not yet executed. `line` is
/// the current edit line (without a trailing newline), with the cursor at
/// byte `cursor_in_line`.
///
/// Returns the byte range in `line` that the candidates should replace,
/// and the list of candidate strings sorted alphabetically.
pub fn completions(
    env: &GlobalEnv,
    pending: &str,
    line: &str,
    cursor_in_line: usize,
) -> (std::ops::Range<usize>, Vec<String>) {
    // Snap cursor to a char boundary so callers passing arbitrary positions
    // can't poison the slicing below.
    let mut pos = cursor_in_line.min(line.len());
    while pos > 0 && !line.is_char_boundary(pos) {
        pos -= 1;
    }
    let prefix_start = identifier_start(line, pos);
    let prefix = &line[prefix_start..pos];

    // Check for a table access: `obj.field` or `obj:method`.
    let before_prefix = line[..prefix_start].trim_end_matches(|c| c == '.' || c == ':');
    let separator = line[before_prefix.len()..prefix_start]
        .chars()
        .next()
        .unwrap_or(' ');

    // Tier 3c: gather locals visible at the cursor in the combined source.
    // We position the analysis cursor just before the receiver/prefix so
    // that a local being declared on the current line isn't reported as
    // visible to itself.
    let combined = format!("{pending}{line}");
    let combined_cursor = pending.len() + before_prefix.len();
    let visible_locals = locals_at_cursor(&combined, combined_cursor);

    let candidates = if matches!(separator, '.' | ':') {
        // Tier 3d: parse the entire receiver expression and walk it.
        // Handles chained access like `foo.bar`, `f().bar`, `obj:m()`,
        // `require("m").x:y()`. Single-identifier and single-`require`
        // receivers are subsumed by this path.
        if let Some(chain) = parse_receiver_chain(before_prefix) {
            if let Some(ty) = resolve_chain_type(env, &visible_locals, &chain) {
                let mut from_chain: Vec<String> = members_of_type(env, &ty)
                    .into_iter()
                    .filter(|m| m.starts_with(prefix))
                    .collect();
                from_chain.sort();
                from_chain.dedup();
                if !from_chain.is_empty() {
                    return (prefix_start..pos, from_chain);
                }
            }
        }
        // Fallback: identifier-only receiver against the runtime walk.
        let obj_start = identifier_start(before_prefix, before_prefix.len());
        let obj_name = &before_prefix[obj_start..];
        table_field_completions(env, &visible_locals, obj_name, prefix)
    } else {
        // Tier 1 + locals: global names plus locally-declared names.
        let mut candidates = global_completions(env, prefix);
        for (name, _) in &visible_locals {
            if let Ok(name) = name.to_str() {
                if name.starts_with(prefix) && !candidates.iter().any(|c| c == name) {
                    candidates.push(name.to_string());
                }
            }
        }
        candidates.sort();
        candidates
    };

    (prefix_start..pos, candidates)
}

/// Walk backwards from `end` (a char boundary in `s`) and return the byte
/// index of the first byte of the longest trailing run of identifier chars
/// (alphanumeric per Unicode, or `_`).
///
/// Char-boundary safe — multi-byte non-identifier chars (emoji, combining
/// marks, punctuation) cause the run to stop at their boundary, never
/// inside their UTF-8 encoding.
fn identifier_start(s: &str, end: usize) -> usize {
    s[..end]
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0)
}

/// A parsed receiver expression: a head value plus a chain of access /
/// call segments. Used to advance a [`LuaType`] through `foo.bar:baz()`
/// style receiver chains for tier-3d completion.
#[derive(Debug, PartialEq)]
struct ReceiverChain<'a> {
    head: ChainHead<'a>,
    segments: Vec<ChainSegment<'a>>,
}

#[derive(Debug, PartialEq)]
enum ChainHead<'a> {
    /// Bare identifier (local, global, etc.).
    Ident(&'a str),
    /// `require("name")` call — special-cased so we can look the module's
    /// return type up in the env's preload registry without executing it.
    Require(&'a str),
}

#[derive(Debug, PartialEq)]
enum ChainSegment<'a> {
    /// `.name` field access.
    Field(&'a str),
    /// `:name` method access; resolves identically to `Field` here, but a
    /// subsequent [`ChainSegment::Call`] should treat the function as a
    /// method (consuming `self` from its parameter list).
    Method(&'a str),
    /// `(...)` call. We don't inspect arg expressions — only that the
    /// surrounding parens balance.
    Call,
}

/// Parse a receiver expression like `foo.bar:baz()` into a head and a list
/// of segments. Returns `None` if the input doesn't look like a clean
/// receiver (presence of operators, assignment, etc.).
fn parse_receiver_chain(s: &str) -> Option<ReceiverChain<'_>> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();

    // Head: either `require("name")` (special-case) or an identifier.
    let (head, mut pos) = if let Some(rest) = s.strip_prefix("require") {
        if !rest.is_empty()
            && rest
                .chars()
                .next()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false)
        {
            // Just an identifier that happens to start with `require`.
            let (ident, end) = read_identifier(s, 0)?;
            (ChainHead::Ident(ident), end)
        } else {
            match parse_require_head(s) {
                Some((module, end)) => (ChainHead::Require(module), end),
                None => (ChainHead::Ident("require"), "require".len()),
            }
        }
    } else {
        let (ident, end) = read_identifier(s, 0)?;
        (ChainHead::Ident(ident), end)
    };

    // Segments: `.name`, `:name`, or `()`.
    let mut segments = Vec::new();
    while pos < bytes.len() {
        // Skip whitespace.
        while pos < bytes.len() && (bytes[pos] == b' ' || bytes[pos] == b'\t') {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        match bytes[pos] {
            b'.' => {
                pos += 1;
                let (name, end) = read_identifier(s, pos)?;
                pos = end;
                segments.push(ChainSegment::Field(name));
            }
            b':' => {
                pos += 1;
                let (name, end) = read_identifier(s, pos)?;
                pos = end;
                segments.push(ChainSegment::Method(name));
            }
            b'(' => {
                let end = find_matching_paren(s, pos)?;
                pos = end + 1;
                segments.push(ChainSegment::Call);
            }
            _ => return None,
        }
    }

    Some(ReceiverChain { head, segments })
}

/// Read a Lua identifier starting at `start` in `s`. Returns the identifier
/// substring and the byte index past its end. Identifiers begin with
/// `[A-Za-z_]` (using `char::is_alphabetic` so non-ASCII letters work) and
/// continue with alphanumeric or `_`.
fn read_identifier(s: &str, start: usize) -> Option<(&str, usize)> {
    let mut end = start;
    let mut iter = s[start..].char_indices();
    let (_, first) = iter.next()?;
    if !first.is_alphabetic() && first != '_' {
        return None;
    }
    end += first.len_utf8();
    for (_, c) in iter {
        if c.is_alphanumeric() || c == '_' {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    Some((&s[start..end], end))
}

/// Find the byte index of the `)` matching the `(` at `open_pos`, tracking
/// nested parens but skipping over string literal contents.
///
/// Handles `"..."`, `'...'`, and luau backtick `\`...\`` strings (treating
/// the entire backtick-delimited region as opaque, including any embedded
/// `{...}` interpolations — the parens inside an interpolation balance
/// among themselves and aren't relevant to outer-paren matching). Doesn't
/// recursively handle nested backticks inside interpolations; that's a
/// rare enough case to leave for later.
fn find_matching_paren(s: &str, open_pos: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.get(open_pos)? != &b'(' {
        return None;
    }
    let mut depth = 1;
    let mut i = open_pos + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            // Skip over `"..."` and `'...'` string literals.
            b'"' | b'\'' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 1;
                    }
                    i += 1;
                }
            }
            // Skip over `\`...\`` (luau interpolated strings).
            b'`' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'`' {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parse `require("name")` (or `require 'name'` etc.) starting at byte 0
/// of `s`. Returns the module name and the byte index past the closing
/// `)` (or quote, for the no-paren form).
fn parse_require_head(s: &str) -> Option<(&str, usize)> {
    let rest = s.strip_prefix("require")?;
    let leading = s.len() - rest.len(); // = 7
    let after_ws = rest.trim_start();
    let consumed_ws = rest.len() - after_ws.len();
    let bytes = after_ws.as_bytes();
    if bytes.first() == Some(&b'(') {
        // require("name")
        let inner = &after_ws[1..];
        let inner_trim = inner.trim_start();
        let inner_ws = inner.len() - inner_trim.len();
        let inner_bytes = inner_trim.as_bytes();
        let quote = match inner_bytes.first()? {
            b'"' => b'"',
            b'\'' => b'\'',
            _ => return None,
        };
        let body = &inner_trim[1..];
        let close = body.bytes().position(|b| b == quote)?;
        let name = &body[..close];
        // Find the closing `)` after the closing quote, allowing whitespace.
        let after_close_quote = &body[close + 1..];
        let trimmed = after_close_quote.trim_start();
        if !trimmed.starts_with(')') {
            return None;
        }
        let trailing_ws = after_close_quote.len() - trimmed.len();
        // total: leading + consumed_ws + 1 ('(') + inner_ws + 1 (quote) + close + 1 (quote) + trailing_ws + 1 (')')
        let end = leading + consumed_ws + 1 + inner_ws + 1 + close + 1 + trailing_ws + 1;
        Some((name, end))
    } else {
        // require 'name' or require "name"
        let quote = match bytes.first()? {
            b'"' => b'"',
            b'\'' => b'\'',
            _ => return None,
        };
        let body = &after_ws[1..];
        let close = body.bytes().position(|b| b == quote)?;
        let name = &body[..close];
        let end = leading + consumed_ws + 1 + close + 1;
        Some((name, end))
    }
}

/// Walk a parsed receiver chain to determine its [`LuaType`]. Returns
/// `None` if the head can't be resolved or some segment dead-ends (e.g.
/// calling a non-function, accessing a field on a scalar without intrinsic
/// methods).
fn resolve_chain_type(
    env: &GlobalEnv,
    visible_locals: &[(shingetsu::Bytes, LuaType)],
    chain: &ReceiverChain<'_>,
) -> Option<LuaType> {
    // Resolve the head to its type.
    let mut current: LuaType = match chain.head {
        ChainHead::Require(module_name) => {
            // Prefer the typed preload registry; that's the only source
            // that knows about modules that haven't been executed yet.
            // Fall back to the runtime value, which covers stdlib
            // libraries like `table`, `string`, `math` that are
            // registered as plain globals rather than via
            // `register_preload_typed`.
            env.module_type_info(module_name.as_bytes())
                .and_then(|info| info.return_type)
                .or_else(|| {
                    env.get_global(module_name)
                        .and_then(|v| infer_type_from_value(&v))
                })?
        }
        ChainHead::Ident(name) => resolve_identifier_type(env, visible_locals, name)?,
    };

    // Apply each segment.
    for segment in &chain.segments {
        match segment {
            ChainSegment::Field(name) | ChainSegment::Method(name) => {
                current = field_type_of(env, &current, name)?;
            }
            ChainSegment::Call => {
                current = call_return_type(&current)?;
            }
        }
    }

    Some(current)
}

/// Resolve a bare identifier to its [`LuaType`], consulting locals first
/// (most-recent shadowing wins), then the global type map, then the
/// runtime value via `infer_type_from_value`.
fn resolve_identifier_type(
    env: &GlobalEnv,
    visible_locals: &[(shingetsu::Bytes, LuaType)],
    name: &str,
) -> Option<LuaType> {
    if let Some((_, ty)) = visible_locals
        .iter()
        .rev()
        .find(|(n, _)| n.as_ref() == name.as_bytes())
    {
        return Some(ty.clone());
    }
    let type_map = env.global_type_map();
    if let Some(ty) = type_map.get(name.as_bytes()).cloned() {
        return Some(ty);
    }
    env.get_global(name).and_then(|v| infer_type_from_value(&v))
}

/// Look up `name` as a field or method on a [`LuaType`], returning the
/// field's declared type. For wrapper types (Optional, Generic), descends
/// to the inner type.
fn field_type_of(env: &GlobalEnv, ty: &LuaType, name: &str) -> Option<LuaType> {
    match ty {
        LuaType::Module(m) => {
            for f in &m.fields {
                if f.name.as_ref() == name.as_bytes() {
                    return Some(f.lua_type.clone());
                }
            }
            for f in m.functions.iter().chain(m.methods.iter()) {
                if f.name.as_ref() == name.as_bytes() {
                    return Some(function_def_to_lua_type(&f.signature));
                }
            }
            None
        }
        LuaType::Table(t) => t
            .fields
            .iter()
            .find(|(n, _)| n.as_ref() == name.as_bytes())
            .map(|(_, ty)| ty.clone()),
        LuaType::Optional(inner) => field_type_of(env, inner, name),
        LuaType::Generic { base, .. } => field_type_of(env, base, name),
        // String methods live behind the string metatable's `__index`. We
        // consult that rather than the `string` global directly, since the
        // global can be reassigned (e.g. `string = nil`) but the metatable
        // is what actually drives `s:method()` dispatch.
        LuaType::String | LuaType::StringLiteral(_) => {
            let methods = string_method_table(env)?;
            match methods.raw_get(&Value::string(name)).ok()? {
                Value::Nil => None,
                v => infer_type_from_value(&v),
            }
        }
        _ => None,
    }
}

/// Resolve the table that holds string methods (the `__index` of the
/// shared string metatable). Returns `None` if the metatable hasn't been
/// installed (which happens when `register_libs(BUILTINS)` hasn't run).
fn string_method_table(env: &GlobalEnv) -> Option<shingetsu::Table> {
    let mt = env.get_string_metatable()?;
    match mt.raw_get(&Value::string("__index")).ok()? {
        Value::Table(t) => Some(t),
        _ => None,
    }
}

/// Build a `LuaType::Function` from a `FunctionSignature` so that calls in
/// the chain can advance to the function's return type.
fn function_def_to_lua_type(sig: &shingetsu::types::FunctionSignature) -> LuaType {
    // The compiler's type machinery already does this via
    // `infer_function_type`; that function isn't part of the public API,
    // so we re-create just enough here. For our purposes we only care
    // about the `lua_returns` field to feed the next chain segment.
    LuaType::Function(Box::new(shingetsu::types::FunctionLuaType {
        type_params: Vec::new(),
        params: Vec::new(),
        variadic: None,
        returns: sig.lua_returns.clone().unwrap_or_default(),
        is_method: sig.arg_offset > 0,
        inferred_unannotated: false,
    }))
}

/// First return type of a function type. Returns `None` for non-function
/// types or functions with no declared return.
fn call_return_type(ty: &LuaType) -> Option<LuaType> {
    match ty {
        LuaType::Function(f) => f.returns.first().cloned(),
        LuaType::Optional(inner) => call_return_type(inner),
        LuaType::Generic { base, .. } => call_return_type(base),
        _ => None,
    }
}

fn global_completions(env: &GlobalEnv, prefix: &str) -> Vec<String> {
    let table = env.env_table();
    collect_string_keys(&table, prefix)
}

fn table_field_completions(
    env: &GlobalEnv,
    visible_locals: &[(shingetsu::Bytes, LuaType)],
    obj_name: &str,
    prefix: &str,
) -> Vec<String> {
    let value = env.get_global(obj_name);
    let mut candidates: Vec<String> = Vec::new();

    // Tier 2: runtime walk for tables.
    if let Some(Value::Table(ref t)) = value {
        candidates.extend(collect_string_keys(t, prefix));
    }

    // Tier 3c: locals visible at the cursor (function params, `local x: T`).
    // The most-recent declaration wins on shadowing; iterating in reverse
    // and taking the first match gives us that.
    let from_locals = visible_locals
        .iter()
        .rev()
        .find(|(name, _)| name.as_ref() == obj_name.as_bytes())
        .map(|(_, ty)| ty.clone());

    // Tier 3a: type-driven members from the compile-time `GlobalTypeMap`
    // and from runtime value inference.
    let type_map = env.global_type_map();
    let from_map = type_map.get(obj_name.as_bytes()).cloned();
    let from_value = value.as_ref().and_then(infer_type_from_value);
    for ty in from_locals
        .iter()
        .chain(from_map.iter())
        .chain(from_value.iter())
    {
        for member in members_of_type(env, ty) {
            if member.starts_with(prefix) {
                candidates.push(member);
            }
        }
    }

    candidates.sort();
    candidates.dedup();
    candidates
}

/// Enumerate the names accessible via `.` or `:` on a value of the given
/// type, given an env for resolving "intrinsic" types whose methods live in
/// a stdlib table (`string`, `math`, etc.).
///
/// Walks the [`LuaType`] structure, descending into wrappers (Optional,
/// Generic) and combining union/intersection sets the conservative way:
/// unions only include members present in *all* arms, intersections include
/// any. Returns names without de-duplication or sorting; callers handle
/// that.
fn members_of_type(env: &GlobalEnv, ty: &LuaType) -> Vec<String> {
    let mut out = Vec::new();
    walk_type(env, ty, &mut out);
    out
}

fn walk_type(env: &GlobalEnv, ty: &LuaType, out: &mut Vec<String>) {
    match ty {
        LuaType::Module(m) => walk_module(m, out),
        LuaType::Table(t) => walk_table(t, out),
        LuaType::Optional(inner) => walk_type(env, inner, out),
        LuaType::Generic { base, .. } => walk_type(env, base, out),
        LuaType::Intersection(arms) => {
            // Union of member sets: any arm's members are accessible.
            for arm in arms {
                walk_type(env, arm, out);
            }
        }
        LuaType::Union(arms) => {
            // Intersection of member sets: only members in *every* arm are
            // safe to access.
            let mut sets: Vec<std::collections::HashSet<String>> = arms
                .iter()
                .map(|t| {
                    let mut v = Vec::new();
                    walk_type(env, t, &mut v);
                    v.into_iter().collect()
                })
                .collect();
            if let Some(first) = sets.pop() {
                let intersection = sets
                    .into_iter()
                    .fold(first, |acc, s| acc.intersection(&s).cloned().collect());
                out.extend(intersection);
            }
        }
        // String methods live behind the string metatable's `__index`,
        // not via the `string` global (which can be reassigned).
        LuaType::String | LuaType::StringLiteral(_) => {
            if let Some(t) = string_method_table(env) {
                out.extend(collect_all_string_keys(&t));
            }
        }
        // Named aliases would need a TypeAlias registry to resolve, which we
        // don't have plumbed through to the env yet. Falls through to no
        // members; tier 2 runtime walk covers most cases in practice.
        LuaType::Named(_)
        | LuaType::Function(_)
        | LuaType::Nil
        | LuaType::Boolean
        | LuaType::Number
        | LuaType::Integer
        | LuaType::Float
        | LuaType::Any
        | LuaType::Unknown
        | LuaType::Never
        | LuaType::TypeParam(_)
        | LuaType::BoolLiteral(_)
        | LuaType::NumberLiteral(_)
        | LuaType::Variadic(_)
        | LuaType::Tuple(_) => {}
    }
}

/// Walk every string key in `table` (no prefix filtering). Used by
/// `walk_type` for intrinsic types that resolve through stdlib tables.
fn collect_all_string_keys(table: &shingetsu::Table) -> Vec<String> {
    let mut out = Vec::new();
    let mut key = Value::Nil;
    loop {
        match table.next(&key) {
            Ok(Some((k, _))) => {
                if let Value::String(ref s) = k {
                    if let Ok(name) = s.to_str() {
                        out.push(name.to_string());
                    }
                }
                key = k;
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }
    out
}

fn walk_module(m: &ModuleType, out: &mut Vec<String>) {
    for f in &m.fields {
        if matches!(f.kind, FieldKind::Setter) {
            continue;
        }
        if let Ok(name) = f.name.to_str() {
            out.push(name.to_string());
        }
    }
    for f in &m.functions {
        if let Ok(name) = f.name.to_str() {
            out.push(name.to_string());
        }
    }
    for method in &m.methods {
        if let Ok(name) = method.name.to_str() {
            out.push(name.to_string());
        }
    }
}

fn walk_table(t: &TableLuaType, out: &mut Vec<String>) {
    for (name, _ty) in &t.fields {
        if let Ok(name) = name.to_str() {
            out.push(name.to_string());
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use shingetsu::types::{ModuleTypeInfo, TableLuaType};
    use shingetsu::{Bytes, Table};

    /// Build a `LuaType::Table` with the given string field names. Used to
    /// avoid constructing the verbose `FunctionSignature` for module tests.
    fn table_type(names: &[&str]) -> LuaType {
        let fields = names
            .iter()
            .map(|n| (Bytes::from(*n), LuaType::Any))
            .collect();
        LuaType::Table(Box::new(TableLuaType {
            fields,
            indexer: None,
        }))
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn members_of_table_type() {
        let ty = table_type(&["x", "y", "z"]);
        k9::assert_equal!(
            sorted(members_of_type(&GlobalEnv::new(), &ty)),
            vec!["x".to_string(), "y".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn members_descend_through_optional() {
        let ty = LuaType::Optional(Box::new(table_type(&["a", "b"])));
        k9::assert_equal!(
            sorted(members_of_type(&GlobalEnv::new(), &ty)),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn members_descend_through_generic() {
        let ty = LuaType::Generic {
            base: Box::new(table_type(&["first", "second"])),
            args: vec![],
        };
        k9::assert_equal!(
            sorted(members_of_type(&GlobalEnv::new(), &ty)),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn members_intersect_unions() {
        // { a, b } | { b, c }  -> only `b` is safe to access
        let ty = LuaType::Union(vec![table_type(&["a", "b"]), table_type(&["b", "c"])]);
        k9::assert_equal!(
            sorted(members_of_type(&GlobalEnv::new(), &ty)),
            vec!["b".to_string()]
        );
    }

    #[test]
    fn members_union_intersections() {
        // { a, b } & { c }  -> all of a, b, c are accessible
        let ty = LuaType::Intersection(vec![table_type(&["a", "b"]), table_type(&["c"])]);
        k9::assert_equal!(
            sorted(members_of_type(&GlobalEnv::new(), &ty)),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn members_of_unsupported_types_are_empty() {
        // Function, scalar, alias names — no member completions.
        for ty in [
            LuaType::Any,
            LuaType::Number,
            LuaType::String,
            LuaType::Named(Bytes::from("User")),
        ] {
            k9::assert_equal!(
                members_of_type(&GlobalEnv::new(), &ty),
                Vec::<String>::new()
            );
        }
    }

    #[tokio::test]
    async fn completions_for_string_library_method_prefix() {
        // Register the standard library, then verify table_field_completions
        // for `string.up...`. Exercises the full path: tier 2 (runtime walk)
        // and tier 3a (type-driven members) both fire and merge.
        // `upper` is the only standard string method starting with `up`.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        let line = "string.up";
        let (range, candidates) = completions(&env, "", line, line.len());

        k9::assert_equal!(range, 7..9);
        k9::assert_equal!(candidates, vec!["upper".to_string()]);
    }

    // ---------------------------------------------------------------
    // parse_receiver_chain (tier 3d).
    // ---------------------------------------------------------------

    #[test]
    fn chain_parses_bare_identifier() {
        k9::assert_equal!(
            parse_receiver_chain("foo"),
            Some(ReceiverChain {
                head: ChainHead::Ident("foo"),
                segments: vec![],
            })
        );
    }

    #[test]
    fn chain_parses_dotted_field_access() {
        k9::assert_equal!(
            parse_receiver_chain("foo.bar.baz"),
            Some(ReceiverChain {
                head: ChainHead::Ident("foo"),
                segments: vec![ChainSegment::Field("bar"), ChainSegment::Field("baz")],
            })
        );
    }

    #[test]
    fn chain_parses_method_then_call() {
        k9::assert_equal!(
            parse_receiver_chain("obj:method()"),
            Some(ReceiverChain {
                head: ChainHead::Ident("obj"),
                segments: vec![ChainSegment::Method("method"), ChainSegment::Call],
            })
        );
    }

    #[test]
    fn chain_parses_require_with_field() {
        k9::assert_equal!(
            parse_receiver_chain("require(\"net.http\").Get"),
            Some(ReceiverChain {
                head: ChainHead::Require("net.http"),
                segments: vec![ChainSegment::Field("Get")],
            })
        );
    }

    #[test]
    fn chain_parses_require_single_quotes() {
        k9::assert_equal!(
            parse_receiver_chain("require('bar.baz')"),
            Some(ReceiverChain {
                head: ChainHead::Require("bar.baz"),
                segments: vec![],
            })
        );
    }

    #[test]
    fn chain_parses_call_with_balanced_args() {
        // Args contain parens — `find_matching_paren` should track depth.
        k9::assert_equal!(
            parse_receiver_chain("f(a, g(b)).x"),
            Some(ReceiverChain {
                head: ChainHead::Ident("f"),
                segments: vec![ChainSegment::Call, ChainSegment::Field("x")],
            })
        );
    }

    #[test]
    fn chain_parses_call_with_string_arg_containing_paren() {
        // The closing `)` inside the string shouldn't end the call.
        k9::assert_equal!(
            parse_receiver_chain("f(\")\").x"),
            Some(ReceiverChain {
                head: ChainHead::Ident("f"),
                segments: vec![ChainSegment::Call, ChainSegment::Field("x")],
            })
        );
    }

    #[test]
    fn chain_parses_call_with_interpolated_string_arg() {
        // Luau interpolated string in the call args. The inner `{ ... }`
        // contains an arithmetic expression with its own balanced parens
        // (`foo()` and `(3/4)`); paren-matching for the OUTER call
        // shouldn't be confused by them. The `.x` after verifies the
        // outer call's closing `)` is correctly identified.
        k9::assert_equal!(
            parse_receiver_chain("f(`{foo() + (3/4)}`).x"),
            Some(ReceiverChain {
                head: ChainHead::Ident("f"),
                segments: vec![ChainSegment::Call, ChainSegment::Field("x")],
            })
        );
    }

    // ---------------------------------------------------------------
    // Panic-safety: weird input must not crash the chain parser.
    // ---------------------------------------------------------------

    #[test]
    fn chain_handles_emoji_as_head() {
        // The first char isn't alphabetic, so read_identifier returns
        // None and parse_receiver_chain bails. The point is no panic
        // from slicing inside the multi-byte sequence.
        k9::assert_equal!(parse_receiver_chain("\u{1F389}.bar"), None);
    }

    #[test]
    fn chain_handles_emoji_after_dot() {
        // `foo.` then an emoji: read_identifier rejects the emoji as a
        // non-alphabetic first char.
        k9::assert_equal!(parse_receiver_chain("foo.\u{1F389}"), None);
    }

    #[test]
    fn chain_handles_emoji_in_module_name() {
        // Bytes of the emoji never collide with ASCII delimiters, so
        // parse_require_head finds the closing quote correctly.
        k9::assert_equal!(
            parse_receiver_chain("require('\u{1F389}')"),
            Some(ReceiverChain {
                head: ChainHead::Require("\u{1F389}"),
                segments: vec![],
            })
        );
    }

    #[test]
    fn chain_handles_combining_marks_in_module_name() {
        // `e\u{0301}` is `e` + combining acute accent (2 bytes total).
        k9::assert_equal!(
            parse_receiver_chain("require(\"e\u{0301}\")"),
            Some(ReceiverChain {
                head: ChainHead::Require("e\u{0301}"),
                segments: vec![],
            })
        );
    }

    #[test]
    fn chain_handles_multibyte_inside_call_args() {
        // Multi-byte chars in the call args shouldn't confuse paren
        // matching.
        k9::assert_equal!(
            parse_receiver_chain("f(\"\u{1F389}\").x"),
            Some(ReceiverChain {
                head: ChainHead::Ident("f"),
                segments: vec![ChainSegment::Call, ChainSegment::Field("x")],
            })
        );
    }

    #[test]
    fn chain_parses_call_with_unbalanced_paren_in_backtick_string() {
        // A literal `(` inside a backtick string would confuse a naive
        // paren-counter. The parser must skip over the contents of
        // backtick-delimited strings the same way it skips `"..."` and
        // `'...'`.
        k9::assert_equal!(
            parse_receiver_chain("f(`(`).x"),
            Some(ReceiverChain {
                head: ChainHead::Ident("f"),
                segments: vec![ChainSegment::Call, ChainSegment::Field("x")],
            })
        );
    }

    #[test]
    fn chain_rejects_pseudo_require() {
        // `myrequire` is just an identifier; the `(...)` after it is a call
        // segment.
        k9::assert_equal!(
            parse_receiver_chain("myrequire(\"foo\")"),
            Some(ReceiverChain {
                head: ChainHead::Ident("myrequire"),
                segments: vec![ChainSegment::Call],
            })
        );
    }

    #[test]
    fn chain_rejects_assignments_and_operators() {
        // Anything that isn't a receiver expression should fail to parse.
        k9::assert_equal!(parse_receiver_chain("local x = foo"), None);
        k9::assert_equal!(parse_receiver_chain("a + b"), None);
    }

    #[test]
    fn chain_rejects_empty_input() {
        k9::assert_equal!(parse_receiver_chain(""), None);
    }

    #[tokio::test]
    async fn completions_resolve_typed_require() {
        // Register a module with a return type that exposes named fields,
        // but DON'T execute the require: completions for
        // `require("mymod").<TAB>` should still work.
        let env = GlobalEnv::new();
        let info = ModuleTypeInfo {
            return_type: Some(table_type(&["connect", "listen", "shutdown"])),
            ..Default::default()
        };
        env.register_preload_typed("mymod", |_env| Ok(Table::new()), info);

        let line = "require(\"mymod\").l";
        let (range, candidates) = completions(&env, "", line, line.len());

        k9::assert_equal!(range, 17..18);
        k9::assert_equal!(candidates, vec!["listen".to_string()]);
    }

    #[tokio::test]
    async fn completions_for_chained_field_access() {
        // `require("mymod").client.send` — walk through Field, Field, then
        // surface members of the final type. We model `client` as a
        // sub-table and `send` as a string field.
        let env = GlobalEnv::new();
        let send_ty = LuaType::String;
        let client_ty = LuaType::Table(Box::new(TableLuaType {
            fields: vec![(Bytes::from("send"), send_ty)],
            indexer: None,
        }));
        let module_ty = LuaType::Table(Box::new(TableLuaType {
            fields: vec![(Bytes::from("client"), client_ty)],
            indexer: None,
        }));
        let info = ModuleTypeInfo {
            return_type: Some(module_ty),
            ..Default::default()
        };
        env.register_preload_typed("mymod", |_env| Ok(Table::new()), info);
        // Need `string` global so that `send: string` resolves to its methods.
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        let line = "require(\"mymod\").client.send.up";
        let (range, candidates) = completions(&env, "", line, line.len());

        // Replacement range should be the prefix `up`.
        let pos = line.len();
        k9::assert_equal!(range, (pos - 2)..pos);
        // Only `upper` from string library matches `up`.
        k9::assert_equal!(candidates, vec!["upper".to_string()]);
    }

    #[tokio::test]
    async fn completions_for_chain_through_function_call() {
        // `getString()` returns a string; `getString().u<TAB>` should
        // complete to string methods. Models the function as a typed
        // module field with `lua_returns: [string]`.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        // Set up a custom global `getString` whose runtime value happens
        // to be a function with declared return type `string`. The simplest
        // way: write a typed function via the type map directly.
        let func_ty = LuaType::Function(Box::new(shingetsu::types::FunctionLuaType {
            type_params: Vec::new(),
            params: Vec::new(),
            variadic: None,
            returns: vec![LuaType::String],
            is_method: false,
            inferred_unannotated: false,
        }));
        // We need this in the type map. set_global infers from the runtime
        // value, which would lose the declared return type, so we have to
        // use the function's *runtime* signature to carry returns. The
        // shortcut: build a Lua function whose signature has `lua_returns`
        // set. For this test, register via raw type-map manipulation by
        // creating an entry directly.
        //
        // Since GlobalEnv doesn't expose a direct "insert into type map"
        // API, we sidestep by using a typed module: register `mymod` whose
        // return_type IS the function type. Then `require("mymod")()`
        // would have the function's return type — but we're calling the
        // module's return value. Use a Module return type with a function
        // field instead.
        let module_ty = LuaType::Table(Box::new(TableLuaType {
            fields: vec![(Bytes::from("build"), func_ty)],
            indexer: None,
        }));
        env.register_preload_typed(
            "strings",
            |_| Ok(Table::new()),
            ModuleTypeInfo {
                return_type: Some(module_ty),
                ..Default::default()
            },
        );

        let line = "require(\"strings\").build().u";
        let (range, candidates) = completions(&env, "", line, line.len());
        let pos = line.len();
        k9::assert_equal!(range, (pos - 1)..pos);
        // String library has both `unpack` and `upper` starting with `u`.
        k9::assert_equal!(candidates, vec!["unpack".to_string(), "upper".to_string()]);
    }

    #[tokio::test]
    async fn completions_for_require_of_runtime_global_module() {
        // `table` is registered as a runtime global by register_libs, NOT
        // via register_preload_typed, so it has no module_type_info.
        // The fallback path in resolve_chain_type should pick up its
        // runtime value and infer a type, so completions still work.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        let line = "require('table').inse";
        let (range, candidates) = completions(&env, "", line, line.len());
        let pos = line.len();
        k9::assert_equal!(range, (pos - 4)..pos);
        k9::assert_equal!(candidates, vec!["insert".to_string()]);
    }

    #[test]
    fn parse_status_double_and_triple_dot_inputs() {
        // `table..` ends with `.`, so the heuristic catches it. (Lexically
        // this is `table` followed by the start of a `..` concat operator;
        // we treat it as Incomplete.)
        assert!(matches!(
            parse_status("", "table.."),
            ParseStatus::Incomplete
        ));
        // `table...` also ends with `.`. Lexically `...` is the vararg,
        // but our heuristic doesn't peek that deeply — either way the
        // input is Incomplete because no valid Lua program ends with `.`.
        assert!(matches!(
            parse_status("", "table..."),
            ParseStatus::Incomplete
        ));
    }

    #[test]
    fn completions_with_double_dot_inputs() {
        // Trailing `..` and `...` are degenerate inputs (Lua's concat
        // operator and vararg — not field access). These tests pin the
        // observed behaviour: the receiver-detection slicing is
        // panic-safe, and `before_prefix` after stripping trailing `.`s
        // happens to be `table`, so we get the whole table library.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        let table_members = vec![
            "clear".to_string(),
            "clone".to_string(),
            "concat".to_string(),
            "create".to_string(),
            "find".to_string(),
            "freeze".to_string(),
            "insert".to_string(),
            "isfrozen".to_string(),
            "move".to_string(),
            "pack".to_string(),
            "remove".to_string(),
            "sort".to_string(),
            "unpack".to_string(),
        ];

        // `table..` — trailing `..`s are stripped by `trim_end_matches`,
        // leaving `table` as the receiver and an empty prefix; we get the
        // full table library.
        let line = "table..";
        let result = completions(&env, "", line, line.len());
        k9::assert_equal!(result, (line.len()..line.len(), table_members.clone()));

        // `table...` — same as above, three dots all stripped.
        let line = "table...";
        let result = completions(&env, "", line, line.len());
        k9::assert_equal!(result, (line.len()..line.len(), table_members.clone()));

        // `table..foo` — `foo` is the prefix; nothing in table starts with foo.
        let line = "table..foo";
        let result = completions(&env, "", line, line.len());
        k9::assert_equal!(result, (7..line.len(), Vec::<String>::new()));

        // `table...foo` — same.
        let line = "table...foo";
        let result = completions(&env, "", line, line.len());
        k9::assert_equal!(result, (8..line.len(), Vec::<String>::new()));
    }

    #[test]
    fn parse_status_trailing_dot_is_incomplete() {
        // `require('table').` makes full_moon report `unexpected token .`
        // because the preceding call parses as a complete statement.
        // The trailing-dot heuristic should classify it as Incomplete.
        assert!(matches!(
            parse_status("", "require('table')."),
            ParseStatus::Incomplete
        ));
        // `table.` was already Incomplete via has_eof_error; verify the
        // new check doesn't break that.
        assert!(matches!(
            parse_status("", "table."),
            ParseStatus::Incomplete
        ));
        // A trailing `:` should also be Incomplete (method-call sugar).
        assert!(matches!(parse_status("", "obj:"), ParseStatus::Incomplete));
    }

    #[tokio::test]
    async fn completions_for_unknown_require_module_falls_through() {
        // No registered type for `nope` — require_module_completions
        // returns nothing, and the identifier-based path also finds
        // nothing useful, so we get an empty result rather than a panic.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        let line = "require(\"nope\").x";
        let (_range, candidates) = completions(&env, "", line, line.len());

        k9::assert_equal!(candidates, Vec::<String>::new());
    }

    // ---------------------------------------------------------------
    // Panic-safety: weird inputs must not crash the slicing logic.
    // Each test asserts the full (range, candidates) result so we both
    // verify no panic and document the (admittedly unhelpful) behaviour
    // for these edge cases.
    // ---------------------------------------------------------------

    #[test]
    fn completions_no_paren_require_does_not_panic() {
        // Lua's bare-string require sugar: `require 'foo'.bar`. We don't
        // recognise this form for module type lookup, so we fall through
        // to identifier-based receiver detection (which gives an empty
        // receiver name and thus no candidates). The point is that no
        // slicing panics.
        let env = GlobalEnv::new();
        let line = "require 'foo'.bar";
        let result = completions(&env, "", line, line.len());
        k9::assert_equal!(result, (14..17, Vec::<String>::new()));
    }

    #[test]
    fn completions_emoji_in_module_name_does_not_panic() {
        let env = GlobalEnv::new();
        let line = "require(\"\u{1F389}mod\").bar";
        // \u{1F389} is 4 bytes in UTF-8.
        let pos = line.len();
        let result = completions(&env, "", line, pos);
        // prefix `bar` ends at pos, starts after the `.`.
        let prefix_start = pos - 3;
        k9::assert_equal!(result, (prefix_start..pos, Vec::<String>::new()));
    }

    #[test]
    fn completions_combining_mark_does_not_panic() {
        let env = GlobalEnv::new();
        // `e` + combining acute accent (U+0301, 2 bytes in UTF-8).
        let line = "require(\"e\u{0301}\").foo";
        let pos = line.len();
        let result = completions(&env, "", line, pos);
        let prefix_start = pos - 3;
        k9::assert_equal!(result, (prefix_start..pos, Vec::<String>::new()));
    }

    #[test]
    fn completions_non_breaking_space_in_module_does_not_panic() {
        let env = GlobalEnv::new();
        let line = "require(\"bogus\u{00A0}\").x";
        let pos = line.len();
        let result = completions(&env, "", line, pos);
        let prefix_start = pos - 1;
        k9::assert_equal!(result, (prefix_start..pos, Vec::<String>::new()));
    }

    #[test]
    fn completions_emoji_as_receiver_does_not_panic() {
        // Without the char-boundary fix, this panicked: `rfind` returns
        // the start byte of the emoji, then `i + 1` was inside the UTF-8.
        let env = GlobalEnv::new();
        let line = "\u{1F389}.bar";
        let pos = line.len();
        let result = completions(&env, "", line, pos);
        // The emoji is the non-id char that bounds `bar`. Empty receiver
        // name + empty global "" → no candidates.
        let prefix_start = pos - 3;
        k9::assert_equal!(result, (prefix_start..pos, Vec::<String>::new()));
    }

    #[test]
    fn completions_cursor_inside_multibyte_char_does_not_panic() {
        // A caller might hand us a cursor position inside a multi-byte
        // sequence. We snap it back to the nearest preceding char
        // boundary rather than panicking. After snapping, prefix is `x`,
        // which matches the built-in global `xpcall`.
        let env = GlobalEnv::new();
        let line = "x\u{1F389}";
        // \u{1F389} occupies bytes 1..5; cursor at byte 3 is mid-emoji.
        let result = completions(&env, "", line, 3);
        k9::assert_equal!(result, (0..1, vec!["xpcall".to_string()]));
    }

    // ---------------------------------------------------------------
    // Tier 3c: locals visible at the cursor.
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn completions_for_local_with_typed_function_param() {
        // Cursor inside a function body should see the typed parameter
        // and complete on its members. We register a typed module so the
        // parameter's `User` type resolves to a usable LuaType. (For an
        // un-aliased `User`, members_of_type returns nothing because we
        // don't have a type-alias registry plumbed through; use a typed
        // module field annotation instead via a structural Table type.)
        // Simpler test: annotate as `string` and complete its methods.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        let pending = "function f(s: string)\n";
        let line = "  s.up";
        let (range, candidates) = completions(&env, pending, line, line.len());

        k9::assert_equal!(range, 4..6);
        k9::assert_equal!(candidates, vec!["upper".to_string()]);
    }

    #[tokio::test]
    async fn completions_for_local_declared_in_pending_buffer() {
        // `local s: string = ...` declared in a previous incomplete line,
        // cursor on a continuation line accessing `s`.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        let pending = "local s: string = 'hello'\n";
        let line = "s.up";
        let (range, candidates) = completions(&env, pending, line, line.len());

        k9::assert_equal!(range, 2..4);
        k9::assert_equal!(candidates, vec!["upper".to_string()]);
    }

    #[tokio::test]
    async fn completions_local_name_appears_in_bare_prefix() {
        // Tier 1 + locals merge: a local declared in pending should appear
        // when the user is typing a bare prefix that matches it.
        let env = GlobalEnv::new();
        // Use an empty env so we don't have to filter out builtins.
        let pending = "local apple = 1\nlocal apricot = 2\n";
        let line = "ap";
        let (range, candidates) = completions(&env, pending, line, line.len());

        k9::assert_equal!(range, 0..2);
        k9::assert_equal!(candidates, vec!["apple".to_string(), "apricot".to_string()]);
    }

    #[test]
    fn completions_for_global_prefix_matches_string() {
        // Tier 1: bare prefix `strin` against BUILTINS only matches `string`.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::BUILTINS).expect("register");

        let line = "strin";
        let (range, candidates) = completions(&env, "", line, line.len());

        k9::assert_equal!(range, 0..5);
        k9::assert_equal!(candidates, vec!["string".to_string()]);
    }
}

fn collect_string_keys(table: &shingetsu::Table, prefix: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut key = Value::Nil;
    loop {
        match table.next(&key) {
            Ok(Some((k, _))) => {
                if let Value::String(ref s) = k {
                    if let Ok(name) = s.to_str() {
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
