//! Diagnostic rendering for compile-time and runtime errors.
//!
//! Uses `annotate-snippets` to produce source-annotated error messages
//! with underlines and labels pointing to the exact location of the
//! problem.

use annotate_snippets::{AnnotationKind, Group, Level, Renderer, Snippet};
use shingetsu_compiler::{CompileError, Diagnostic, Severity};
use shingetsu_vm::error::RuntimeError;
use shingetsu_vm::proto::{format_source_name, SourceLocation};
#[cfg(feature = "test-utils")]
use similar::TextDiff;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Controls whether diagnostic output includes ANSI color codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderStyle {
    Colored,
    Plain,
}

/// Render a compile error with source annotations.
///
/// `source_text` is the full source that was passed to `compile()`.
pub fn render_compile_error(err: &CompileError, source_text: &str, style: RenderStyle) -> String {
    let location = match err {
        CompileError::Parse { location, .. } => location,
        CompileError::UnsupportedFeature { location, .. } => location,
        CompileError::Semantic { location, .. } => location,
    };
    // Use just the message text, not the full Display which prefixes
    // the source location (annotate-snippets renders that separately).
    let message = match err {
        CompileError::Parse { message, .. } => message.clone(),
        CompileError::UnsupportedFeature { feature, .. } => {
            format!("unsupported feature: {feature}")
        }
        CompileError::Semantic { message, .. } => message.clone(),
    };
    let help = match err {
        CompileError::Semantic { help, .. } => help.clone(),
        CompileError::UnsupportedFeature { help, .. } => help.clone(),
        _ => None,
    };

    let renderer = match style {
        RenderStyle::Colored => Renderer::styled(),
        RenderStyle::Plain => Renderer::plain(),
    };

    // If we have a byte offset, produce an annotated snippet.
    if location.byte_offset > 0 || location.line > 0 {
        let span_start = location.byte_offset as usize;
        let span_end = if location.byte_len > 0 {
            span_start + location.byte_len as usize
        } else {
            // Point span: extend to end of token or at least 1 byte.
            find_token_end(source_text, span_start)
        };
        let span_end = span_end.min(source_text.len());

        let label = annotation_label(&message, &message);
        let display_name = format_source_name(&location.source_name);
        let snippet = Snippet::source(source_text).path(&display_name).annotation(
            AnnotationKind::Primary
                .span(span_start..span_end)
                .label(&label),
        );

        let primary = Level::ERROR.primary_title(&message).element(snippet);
        let mut groups: Vec<Group<'_>> = vec![primary];
        if let Some(help_text) = help.as_deref() {
            groups.push(Group::with_title(Level::HELP.secondary_title(help_text)));
        }
        renderer.render(&groups)
    } else {
        // No location info — just render the message.
        let primary = Group::with_title(Level::ERROR.primary_title(&message));
        let mut groups: Vec<Group<'_>> = vec![primary];
        if let Some(help_text) = help.as_deref() {
            groups.push(Group::with_title(Level::HELP.secondary_title(help_text)));
        }
        renderer.render(&groups)
    }
}

/// Render a single compiler warning with source annotations.
///
/// `source_text` is the full source that was passed to `compile()`.
pub fn render_warning(diag: &Diagnostic, source_text: &str, style: RenderStyle) -> String {
    render_warnings(std::slice::from_ref(diag), source_text, style)
}

/// Render multiple compiler warnings, grouping annotations onto a
/// shared source snippet so that each source line appears only once.
///
/// `source_text` is the full source that was passed to `compile()`.
pub fn render_warnings(diags: &[Diagnostic], source_text: &str, style: RenderStyle) -> String {
    if diags.is_empty() {
        return String::new();
    }

    let renderer = match style {
        RenderStyle::Colored => Renderer::styled(),
        RenderStyle::Plain => Renderer::plain(),
    };

    let mut sorted: Vec<&Diagnostic> = diags.iter().collect();
    sorted.sort_by_key(|d| match d.severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Allow => 2,
    });

    let mut output = String::new();

    for diag in sorted {
        let level = severity_to_level(diag.severity);
        let has_loc = diag.location.byte_offset > 0 || diag.location.line > 0;

        let mut groups: Vec<Group<'_>> = Vec::new();
        let display_name = format_source_name(&diag.location.source_name);

        if has_loc {
            let span_start = diag.location.byte_offset as usize;
            let span_end = if diag.location.byte_len > 0 {
                span_start + diag.location.byte_len as usize
            } else {
                find_token_end(source_text, span_start)
            };
            let span_end = span_end.min(source_text.len());

            let primary_label = diag.primary_label.as_deref().unwrap_or(&diag.message);
            let mut snippet = Snippet::source(source_text).path(&display_name).annotation(
                AnnotationKind::Primary
                    .span(span_start..span_end)
                    .label(primary_label),
            );
            // Secondary spans — only those that point at the same
            // source file as the primary location can be rendered on
            // the same snippet.  Different sources would need a
            // separate Snippet, which we skip for now.
            for (sec_loc, sec_label) in &diag.secondary_spans {
                if sec_loc.source_name.as_str() != diag.location.source_name.as_str() {
                    continue;
                }
                if sec_loc.byte_offset == 0 && sec_loc.line == 0 {
                    continue;
                }
                let s_start = sec_loc.byte_offset as usize;
                let s_end = if sec_loc.byte_len > 0 {
                    s_start + sec_loc.byte_len as usize
                } else {
                    find_token_end(source_text, s_start)
                };
                let s_end = s_end.min(source_text.len());
                snippet = snippet.annotation(
                    AnnotationKind::Context
                        .span(s_start..s_end)
                        .label(sec_label.as_str()),
                );
            }
            let id = diag.lint.display_name().into_owned();
            groups.push(
                Group::with_title(level.primary_title(&diag.message).id(id)).element(snippet),
            );
        } else {
            let id = diag.lint.display_name().into_owned();
            groups.push(Group::with_title(level.primary_title(&diag.message).id(id)));
        }

        if let Some(help) = &diag.help {
            groups.push(Group::with_title(
                Level::HELP.secondary_title(help.as_str()),
            ));
        }

        let rendered = renderer.render(&groups);
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&rendered);
    }

    output
}

/// Assert that rendered diagnostics match the expected output.
///
/// Panics with a unified diff if the actual rendered output differs
/// from `expected`.
#[cfg(feature = "test-utils")]
#[track_caller]
pub fn assert_diagnostics(diags: &[Diagnostic], source_text: &str, expected: &str) {
    let actual = render_warnings(diags, source_text, RenderStyle::Plain);
    if actual != expected {
        let diff = TextDiff::from_lines(expected, &actual);
        panic!(
            "diagnostic output mismatch:\n\n{}\n",
            diff.unified_diff()
                .context_radius(3)
                .missing_newline_hint(false)
                .header("expected", "actual")
        );
    }
}

/// Render a single [`Diagnostic`] whose annotations may span
/// multiple source files, looking up each annotation's source text
/// in `sources` (source-name -> text).  Annotations whose source
/// name isn't in `sources` are skipped.  Returns the rendered
/// string in the requested style.
///
/// Used for diagnostics that anchor on more than one file -- e.g.
/// the cross-plugin duplicate-name path where the primary span
/// lives in plugin B's source and a secondary span points at
/// plugin A's first declaration.
pub fn render_diagnostic_multi_source(
    diag: &Diagnostic,
    sources: &[(&str, &str)],
    style: RenderStyle,
) -> String {
    let renderer = match style {
        RenderStyle::Colored => Renderer::styled(),
        RenderStyle::Plain => Renderer::plain(),
    };
    let level = severity_to_level(diag.severity);
    let id = diag.lint.display_name().into_owned();

    let lookup =
        |name: &str| -> Option<&str> { sources.iter().find(|(n, _)| *n == name).map(|(_, t)| *t) };

    // Collect (source_name -> (primary?, secondaries)) so we can
    // emit one Snippet per distinct source file.
    let mut by_source: BTreeMap<
        String,
        (Option<(usize, usize, String)>, Vec<(usize, usize, String)>),
    > = BTreeMap::new();

    let primary_name = diag.location.source_name.as_str().to_string();
    if let Some(text) = lookup(&primary_name) {
        let span_start = diag.location.byte_offset as usize;
        let span_end = if diag.location.byte_len > 0 {
            (span_start + diag.location.byte_len as usize).min(text.len())
        } else {
            find_token_end(text, span_start).min(text.len())
        };
        let label = diag.primary_label.as_deref().unwrap_or(&diag.message);
        by_source.entry(primary_name.clone()).or_default().0 =
            Some((span_start, span_end, label.to_string()));
    }
    for (sec_loc, sec_label) in &diag.secondary_spans {
        let name = sec_loc.source_name.as_str().to_string();
        let Some(text) = lookup(&name) else { continue };
        if sec_loc.byte_offset == 0 && sec_loc.line == 0 {
            continue;
        }
        let s_start = sec_loc.byte_offset as usize;
        let s_end = if sec_loc.byte_len > 0 {
            (s_start + sec_loc.byte_len as usize).min(text.len())
        } else {
            find_token_end(text, s_start).min(text.len())
        };
        by_source
            .entry(name)
            .or_default()
            .1
            .push((s_start, s_end, sec_label.clone()));
    }

    // Iterate sources with the primary file first so the title's
    // snippet anchors the primary span.  Pre-render each source's
    // display name into an owned String so the borrow lives long
    // enough to be threaded through Snippet::path.
    let ordered_names: Vec<&String> = std::iter::once(&primary_name)
        .chain(by_source.keys().filter(|n| *n != &primary_name))
        .collect();
    let displays: Vec<String> = ordered_names
        .iter()
        .map(|n| format_source_name(&Arc::new((*n).clone())))
        .collect();
    let mut group = Group::with_title(level.primary_title(&diag.message).id(id));
    for (name, display) in ordered_names.iter().zip(displays.iter()) {
        let Some((primary_span, secondaries)) = by_source.get(*name) else {
            continue;
        };
        let text = lookup(name).unwrap_or("");
        let mut snippet = Snippet::source(text).path(display.as_str());
        if let Some((s, e, label)) = primary_span {
            snippet =
                snippet.annotation(AnnotationKind::Primary.span(*s..*e).label(label.as_str()));
        }
        for (s, e, label) in secondaries {
            snippet =
                snippet.annotation(AnnotationKind::Context.span(*s..*e).label(label.as_str()));
        }
        group = group.element(snippet);
    }
    let mut groups: Vec<Group<'_>> = vec![group];
    if let Some(help) = &diag.help {
        groups.push(Group::with_title(
            Level::HELP.secondary_title(help.as_str()),
        ));
    }
    renderer.render(&groups)
}

/// When the annotation label spans multiple lines, truncate to the
/// first line + " ...".
fn annotation_label(message: &str, _title: &str) -> String {
    if message.contains('\n') {
        let first_line = message.lines().next().unwrap_or(message);
        format!("{first_line} ...")
    } else {
        message.to_owned()
    }
}

/// Map a compiler diagnostic severity to an `annotate-snippets` level.
fn severity_to_level(severity: Severity) -> Level<'static> {
    match severity {
        Severity::Allow => Level::NOTE,
        Severity::Warning => Level::WARNING,
        Severity::Error => Level::ERROR,
    }
}

/// Render a runtime error with source annotations and stack trace.
///
/// Source text is pulled from `Proto::source_text` via the stack frames
/// in the `RuntimeError`.
pub fn render_runtime_error(err: &RuntimeError, style: RenderStyle) -> String {
    let renderer = match style {
        RenderStyle::Colored => Renderer::styled(),
        RenderStyle::Plain => Renderer::plain(),
    };

    let message = err.to_string();

    // Source text is stored directly on the RuntimeError.
    let source_text = &err.source_text;
    // Pick the best span for this error variant.  If the innermost
    // Lua frame's instruction has per-argument or key sub-spans
    // (see `InstrSpans`), use them when the error is one that names
    // a specific argument or a problematic key.  Otherwise fall back
    // to the instruction's own source location.
    let location = error_specific_location(err).or_else(|| innermost_lua_location(err));

    let mut result = if let Some(loc) = &location {
        let source_str = std::str::from_utf8(source_text).unwrap_or("");
        if !source_str.is_empty() && (loc.byte_offset > 0 || loc.line > 0) {
            let span_start = loc.byte_offset as usize;
            let span_end = if loc.byte_len > 0 {
                span_start + loc.byte_len as usize
            } else {
                find_token_end(source_str, span_start)
            };
            let span_end = span_end.min(source_str.len());
            // Cap the primary span to at most three lines.  When the
            // offending expression is a multi-line constructor (e.g.
            // `os.time({ year=..., month=..., ... })` written across
            // many lines), highlighting every line buries the actual
            // diagnostic in source quotation.  Three lines is
            // usually enough to keep the call boundary visible
            // (`f({` ... `...,` ... `},`) without painting half the
            // file.
            const MAX_PRIMARY_SPAN_LINES: usize = 3;
            let span_end = source_str[span_start..span_end]
                .match_indices('\n')
                .nth(MAX_PRIMARY_SPAN_LINES - 1)
                .map(|(n, _)| span_start + n)
                .unwrap_or(span_end);

            let label = annotation_label(&message, &message);

            let display_name = format_source_name(&loc.source_name);
            let mut snippet = Snippet::source(source_str).path(&display_name).annotation(
                AnnotationKind::Primary
                    .span(span_start..span_end)
                    .label(&label),
            );

            // Add variable-context annotations (definition site,
            // last assignment site).
            if let Some(ref var_ctx) = err.var_context {
                if let Some(ref def) = var_ctx.definition {
                    let def_start = def.byte_offset as usize;
                    let def_end = if def.byte_len > 0 {
                        def_start + def.byte_len as usize
                    } else {
                        find_token_end(source_str, def_start)
                    };
                    let def_end = def_end.min(source_str.len());
                    // Only add if it's a different location from the primary.
                    if def_start != span_start {
                        let def_label = if var_ctx.is_implicit_self {
                            "self implicitly defined here by `:` function syntax"
                        } else {
                            "defined here"
                        };
                        snippet = snippet.annotation(
                            AnnotationKind::Context
                                .span(def_start..def_end)
                                .label(def_label),
                        );
                    }
                }
                if let Some(ref assign) = var_ctx.last_assignment {
                    let assign_start = assign.byte_offset as usize;
                    let assign_end = if assign.byte_len > 0 {
                        assign_start + assign.byte_len as usize
                    } else {
                        find_token_end(source_str, assign_start)
                    };
                    let assign_end = assign_end.min(source_str.len());
                    // Only add if different from both primary and definition.
                    if assign_start != span_start
                        && err
                            .var_context
                            .as_ref()
                            .and_then(|c| c.definition.as_ref())
                            .map_or(true, |d| assign_start != d.byte_offset as usize)
                    {
                        snippet = snippet.annotation(
                            AnnotationKind::Context
                                .span(assign_start..assign_end)
                                .label("last assigned here"),
                        );
                    }
                }
            }

            let group = Group::with_title(Level::ERROR.primary_title(&message)).element(snippet);
            let report: &[Group<'_>] = &[group];
            renderer.render(report)
        } else {
            let group = Group::with_title(Level::ERROR.primary_title(&message));
            let report: &[Group<'_>] = &[group];
            renderer.render(report)
        }
    } else {
        let group = Group::with_title(Level::ERROR.primary_title(&message));
        let report: &[Group<'_>] = &[group];
        renderer.render(report)
    };

    // Render structured hints.
    for hint in &err.hints {
        result.push('\n');
        if let Some(loc) = &hint.location {
            let source_str = std::str::from_utf8(&err.source_text).unwrap_or("");
            if !source_str.is_empty() && (loc.byte_offset > 0 || loc.line > 0) {
                let span_start = loc.byte_offset as usize;
                let span_end = if loc.byte_len > 0 {
                    span_start + loc.byte_len as usize
                } else {
                    find_token_end(source_str, span_start)
                };
                let span_end = span_end.min(source_str.len());
                let hint_display_name = format_source_name(&loc.source_name);
                let snippet = Snippet::source(source_str)
                    .path(&hint_display_name)
                    .annotation(
                        AnnotationKind::Primary
                            .span(span_start..span_end)
                            .label(&hint.message),
                    );
                let group =
                    Group::with_title(Level::HELP.secondary_title(&hint.message)).element(snippet);
                let report: &[Group<'_>] = &[group];
                result.push_str(&renderer.render(report));
            } else {
                let group = Group::with_title(Level::HELP.secondary_title(&hint.message));
                let report: &[Group<'_>] = &[group];
                result.push_str(&renderer.render(report));
            }
        } else {
            let group = Group::with_title(Level::HELP.secondary_title(&hint.message));
            let report: &[Group<'_>] = &[group];
            result.push_str(&renderer.render(report));
        }
    }

    // Append the stack traceback.
    let traceback = format_traceback(&err.call_stack);
    if !traceback.is_empty() {
        result.push('\n');
        result.push_str(&traceback);
    }

    result
}

/// Extract the source location from the innermost Lua frame.
fn innermost_lua_location(err: &RuntimeError) -> Option<SourceLocation> {
    for frame in err.call_stack.iter().rev() {
        if let Some(loc) = frame.source_location() {
            return Some(loc);
        }
    }
    None
}

/// When the error variant names a specific sub-expression, look up
/// the corresponding sub-span in the innermost Lua frame's
/// `InstrSpans` and return it.  This narrows `bad argument #N`
/// errors to point at argument N, and key errors to point at the
/// offending key expression.  Returns `None` when the error has no
/// applicable sub-span or no metadata is present.
///
/// Resolution order for the argument position:
///   1. `RuntimeError::arg_position`, set by host code via
///      `VmResultExt::with_arg_position` when an arbitrary
///      `VmError` (e.g. `IoError`, `HostError`, `LuaError`) is
///      attributable to a specific call argument.
///   2. The `position` field on `BadArgument` / `ArgError`,
///      populated by the auto-extracted-argument path or by
///      `VmResultExt::with_call_context`.
fn error_specific_location(err: &RuntimeError) -> Option<SourceLocation> {
    use shingetsu_vm::error::VmError;
    let frame = err
        .call_stack
        .iter()
        .rev()
        .find(|f| matches!(f, shingetsu_vm::StackFrame::Lua { .. }))?;
    let spans = frame.extra_spans()?;
    // Host-supplied argument attribution wins.
    if let Some(position) = err.arg_position {
        if let Some(idx) = position.checked_sub(1) {
            if let Some(loc) = spans.args.get(idx).cloned() {
                return Some(loc);
            }
        }
    }
    match &err.error {
        VmError::BadArgument { position, .. } | VmError::ArgError { position, .. } => {
            // `position` is 1-based.  `0` means "position not
            // applicable" and falls through to the instruction loc.
            if *position == 0 {
                return None;
            }
            let idx = (*position).checked_sub(1)?;
            spans.args.get(idx).cloned()
        }
        VmError::TableKeyIsNaN { .. } | VmError::TableKeyIsNil { .. } => spans.key.clone(),
        _ => None,
    }
}

/// Format a single stack frame into the traceback line (without the
/// leading `\n\t`).
fn format_frame(frame: &shingetsu_vm::StackFrame) -> String {
    use shingetsu_vm::StackFrame;
    match frame {
        StackFrame::Lua { function, .. } => {
            let loc = frame
                .source_location()
                .map(|l| format!("{}:{}", format_source_name(&l.source_name), l.line))
                .unwrap_or_else(|| "?".to_string());
            let name = String::from_utf8_lossy(&function.name);
            let source_name = String::from_utf8_lossy(&function.source);
            if name == source_name || name.is_empty() {
                format!("{loc}: in main chunk")
            } else {
                format!("{loc}: in function {name}()")
            }
        }
        StackFrame::Native { function_name } => {
            let name = String::from_utf8_lossy(function_name);
            format!("[Native]: in function {name}")
        }
    }
}

/// Maximum number of traceback lines to show at the top and bottom
/// of a long stack trace before truncating the middle.
const TRACEBACK_HEAD: usize = 10;
const TRACEBACK_TAIL: usize = 10;

/// Format the stack traceback from call stack frames.
///
/// Consecutive identical frames are collapsed into a single
/// `... (repeated N times)` line.  When the collapsed trace is still
/// longer than `TRACEBACK_HEAD + TRACEBACK_TAIL` lines, the middle
/// is elided with `... (N frames omitted)`.
fn format_traceback(call_stack: &[shingetsu_vm::StackFrame]) -> String {
    if call_stack.is_empty() {
        return String::new();
    }

    // Phase 1: format frames innermost-first and collapse consecutive
    // identical lines.
    let mut collapsed: Vec<(String, usize)> = Vec::new();
    for frame in call_stack.iter().rev() {
        let line = format_frame(frame);
        if let Some(last) = collapsed.last_mut() {
            if last.0 == line {
                last.1 += 1;
                continue;
            }
        }
        collapsed.push((line, 1));
    }

    // Phase 2: truncate if too many entries.
    let mut out = String::from("stack traceback:");
    let total = collapsed.len();
    let max_lines = TRACEBACK_HEAD + TRACEBACK_TAIL;

    if total <= max_lines {
        // Short enough to show everything.
        for (line, count) in &collapsed {
            write!(out, "\n\t{line}").ok();
            if *count > 1 {
                write!(out, "\n\t... (repeated {} times)", count - 1).ok();
            }
        }
    } else {
        // Show head, ellipsis, tail.
        for (line, count) in &collapsed[..TRACEBACK_HEAD] {
            write!(out, "\n\t{line}").ok();
            if *count > 1 {
                write!(out, "\n\t... (repeated {} times)", count - 1).ok();
            }
        }
        let omitted = total - max_lines;
        write!(out, "\n\t... ({omitted} frames omitted)").ok();
        for (line, count) in &collapsed[total - TRACEBACK_TAIL..] {
            write!(out, "\n\t{line}").ok();
            if *count > 1 {
                write!(out, "\n\t... (repeated {} times)", count - 1).ok();
            }
        }
    }

    out
}

/// Find the end of a token starting at `pos` in source text.
/// Used to expand a point span (byte_len=0) to cover at least
/// one meaningful token.
fn find_token_end(source: &str, pos: usize) -> usize {
    let bytes = source.as_bytes();
    if pos >= bytes.len() {
        return pos;
    }
    let mut end = pos;
    // Extend through word characters.
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    // If we didn't advance at all, take at least one character.
    if end == pos {
        end += 1;
    }
    end
}

use std::fmt::Write;
