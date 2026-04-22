//! Diagnostic rendering for compile-time and runtime errors.
//!
//! Uses `annotate-snippets` to produce source-annotated error messages
//! with underlines and labels pointing to the exact location of the
//! problem.

use annotate_snippets::{AnnotationKind, Group, Level, Renderer, Snippet};
use shingetsu_compiler::{CompileError, Diagnostic, Severity};
use shingetsu_vm::error::RuntimeError;
use shingetsu_vm::proto::{format_source_name, SourceLocation};

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

        let group = Level::ERROR.primary_title(&message).element(snippet);
        let report: &[Group<'_>] = &[group];
        renderer.render(report)
    } else {
        // No location info — just render the message.
        let group: Group<'_> = Group::with_title(Level::ERROR.primary_title(&message));
        let report: &[Group<'_>] = &[group];
        renderer.render(report)
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

            let snippet = Snippet::source(source_text).path(&display_name).annotation(
                AnnotationKind::Primary
                    .span(span_start..span_end)
                    .label(&diag.message),
            );
            groups.push(
                Group::with_title(level.primary_title(&diag.message).id(diag.lint.name()))
                    .element(snippet),
            );
        } else {
            groups.push(Group::with_title(
                level.primary_title(&diag.message).id(diag.lint.name()),
            ));
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
    let location = innermost_lua_location(err);

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
                        snippet = snippet.annotation(
                            AnnotationKind::Context
                                .span(def_start..def_end)
                                .label("defined here"),
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
    use shingetsu_vm::StackFrame;
    for frame in err.call_stack.iter().rev() {
        if let StackFrame::Lua {
            source_location: Some(loc),
            ..
        } = frame
        {
            return Some(loc.clone());
        }
    }
    None
}

/// Format a single stack frame into the traceback line (without the
/// leading `\n\t`).
fn format_frame(frame: &shingetsu_vm::StackFrame) -> String {
    use shingetsu_vm::StackFrame;
    match frame {
        StackFrame::Lua {
            function,
            source_location,
            ..
        } => {
            let loc = source_location
                .as_ref()
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
