use std::collections::HashMap;
use std::ops::Range;

use full_moon::ast;
use full_moon::node::Node;
use full_moon::tokenizer::TokenType;

use crate::error::{Diagnostic, LintId, Severity, SourceLocation};

/// A severity override for a specific lint, scoped to a byte range.
#[derive(Debug, Clone)]
pub struct StatementOverride {
    pub byte_range: Range<u32>,
    pub lint: LintId,
    pub severity: Severity,
}

/// Collected lint directives parsed from source comments.
#[derive(Debug, Clone, Default)]
pub struct LintDirectives {
    /// Project-level overrides from `shingetsu.toml`.
    pub project_overrides: HashMap<LintId, Severity>,
    /// File-level overrides from `--# shingetsu:` directives.
    pub file_overrides: HashMap<LintId, Severity>,
    /// Statement-level overrides from `-- shingetsu:` directives,
    /// each scoped to the byte range of the statement they precede.
    pub statement_overrides: Vec<StatementOverride>,
}

impl LintDirectives {
    /// Resolve the effective severity for a diagnostic.
    ///
    /// Priority: statement-level > file-level > compiled-in default.
    /// Returns `None` if the diagnostic should be suppressed (`allow`).
    pub fn effective_severity(&self, diag: &Diagnostic) -> Option<Severity> {
        // Check statement-level overrides first (most specific).
        let byte = diag.location.byte_offset;
        for so in &self.statement_overrides {
            if so.lint == diag.lint && so.byte_range.contains(&byte) {
                return match so.severity {
                    Severity::Allow => None,
                    s => Some(s),
                };
            }
        }
        // Then file-level overrides.
        if let Some(&sev) = self.file_overrides.get(&diag.lint) {
            return match sev {
                Severity::Allow => None,
                s => Some(s),
            };
        }
        // Then project-level overrides.
        if let Some(&sev) = self.project_overrides.get(&diag.lint) {
            return match sev {
                Severity::Allow => None,
                s => Some(s),
            };
        }
        // Fall back to compiled-in default.
        Some(diag.severity)
    }

    /// Filter and adjust a list of diagnostics according to these directives.
    ///
    /// Suppressed diagnostics are removed; others have their severity
    /// adjusted to the effective level.
    pub fn filter(&self, diagnostics: Vec<Diagnostic>) -> Vec<Diagnostic> {
        diagnostics
            .into_iter()
            .filter_map(|mut diag| {
                let sev = self.effective_severity(&diag)?;
                diag.severity = sev;
                Some(diag)
            })
            .collect()
    }
}

/// Parse a severity keyword.
fn parse_severity(s: &str) -> Option<Severity> {
    match s {
        "allow" => Some(Severity::Allow),
        "warn" => Some(Severity::Warning),
        "deny" => Some(Severity::Error),
        _ => None,
    }
}

/// A parsed but not yet resolved directive from a comment.
struct RawDirective {
    is_file_level: bool,
    action: Severity,
    lints: Vec<String>,
}

/// Parse a single comment string into a directive, if it matches the syntax.
///
/// Expected formats:
///   `# shingetsu: allow(lint1, lint2)`  (file-level, leading `--` already stripped)
///   ` shingetsu: allow(lint1, lint2)`   (statement-level, leading `--` already stripped)
fn parse_comment(comment: &str) -> Option<RawDirective> {
    let trimmed = comment.trim_start();
    let (is_file_level, rest) = if let Some(rest) = trimmed.strip_prefix('#') {
        (true, rest.trim_start())
    } else {
        (false, trimmed)
    };

    let rest = rest.strip_prefix("shingetsu:")?;
    let rest = rest.trim_start();

    // Parse action: allow | warn | deny
    let (action_str, rest) = rest.split_once('(')?;
    let action_str = action_str.trim();
    let action = parse_severity(action_str)?;

    // Parse lint list inside parentheses.
    let rest = rest.strip_suffix(')')?.trim();
    if rest.is_empty() {
        return None;
    }

    let lints: Vec<String> = rest
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();

    Some(RawDirective {
        is_file_level,
        action,
        lints,
    })
}

/// Extract lint directives from an AST and produce diagnostics for
/// unknown lint names or misplaced file-level directives.
pub fn extract_directives(
    ast: &ast::Ast,
    source_name: &str,
    source_text: &str,
) -> (LintDirectives, Vec<Diagnostic>) {
    let mut directives = LintDirectives::default();
    let mut diagnostics = Vec::new();

    // Determine the byte offset of the first non-comment token to validate
    // file-level directive placement.
    let first_code_byte = ast
        .nodes()
        .stmts()
        .next()
        .and_then(|s| Node::start_position(s))
        .map(|p| p.bytes() as u32)
        .or_else(|| {
            ast.nodes()
                .last_stmt()
                .and_then(|s| Node::start_position(s))
                .map(|p| p.bytes() as u32)
        });

    // Walk statements and collect directives from leading trivia.
    let stmts: Vec<_> = ast.nodes().stmts().collect();
    for stmt in &stmts {
        let stmt_range = node_byte_range(*stmt);
        let (leading, _) = Node::surrounding_trivia(*stmt);
        process_trivia(
            &leading,
            &stmt_range,
            first_code_byte,
            source_name,
            source_text,
            &mut directives,
            &mut diagnostics,
        );
    }

    // Also check the last_stmt (return/break).
    if let Some(last) = ast.nodes().last_stmt() {
        let stmt_range = node_byte_range(last);
        let (leading, _) = Node::surrounding_trivia(last);
        process_trivia(
            &leading,
            &stmt_range,
            first_code_byte,
            source_name,
            source_text,
            &mut directives,
            &mut diagnostics,
        );
    }

    // If there are no statements at all, check the EOF token's leading trivia
    // for file-level directives.
    if stmts.is_empty() && ast.nodes().last_stmt().is_none() {
        let eof = ast.eof();
        for trivia in eof.leading_trivia() {
            if let TokenType::SingleLineComment { comment } = trivia.token_type() {
                let comment_str = comment.as_str();
                if let Some(raw) = parse_comment(comment_str) {
                    if raw.is_file_level {
                        apply_file_directive(
                            &raw,
                            source_name,
                            &mut directives,
                            &mut diagnostics,
                        );
                    }
                }
            }
        }
    }

    (directives, diagnostics)
}

fn process_trivia(
    leading: &[&full_moon::tokenizer::Token],
    stmt_range: &Range<u32>,
    first_code_byte: Option<u32>,
    source_name: &str,
    source_text: &str,
    directives: &mut LintDirectives,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for trivia in leading {
        if let TokenType::SingleLineComment { comment } = trivia.token_type() {
            let comment_str = comment.as_str();
            let byte_offset = trivia.start_position().bytes() as u32;
            if let Some(raw) = parse_comment(comment_str) {
                if raw.is_file_level {
                    // Validate placement: must be before first code.
                    if let Some(first_byte) = first_code_byte {
                        if byte_offset >= first_byte {
                            diagnostics.push(Diagnostic {
                                lint: LintId::UnknownLint,
                                severity: Severity::Error,
                                location: loc_from_byte(
                                    source_name,
                                    source_text,
                                    byte_offset,
                                ),
                                message:
                                    "file-level directive must appear before any code"
                                        .to_string(),
                            });
                            continue;
                        }
                    }
                    apply_file_directive(&raw, source_name, directives, diagnostics);
                } else {
                    // Statement-level directive.
                    apply_statement_directive(
                        &raw,
                        stmt_range,
                        source_name,
                        directives,
                        diagnostics,
                    );
                }
            }
        }
    }
}

fn apply_file_directive(
    raw: &RawDirective,
    source_name: &str,
    directives: &mut LintDirectives,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for name in &raw.lints {
        if let Some(lint) = LintId::from_name(name) {
            directives.file_overrides.insert(lint, raw.action);
        } else {
            diagnostics.push(Diagnostic {
                lint: LintId::UnknownLint,
                severity: Severity::Warning,
                location: SourceLocation::unknown(source_name),
                message: format!("unknown lint '{name}'"),
            });
        }
    }
}

fn apply_statement_directive(
    raw: &RawDirective,
    stmt_range: &Range<u32>,
    source_name: &str,
    directives: &mut LintDirectives,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for name in &raw.lints {
        if let Some(lint) = LintId::from_name(name) {
            directives.statement_overrides.push(StatementOverride {
                byte_range: stmt_range.clone(),
                lint,
                severity: raw.action,
            });
        } else {
            diagnostics.push(Diagnostic {
                lint: LintId::UnknownLint,
                severity: Severity::Warning,
                location: SourceLocation::unknown(source_name),
                message: format!("unknown lint '{name}'"),
            });
        }
    }
}

/// Get the byte range of an AST node.
fn node_byte_range(node: &dyn Node) -> Range<u32> {
    let start = node
        .start_position()
        .map(|p| p.bytes() as u32)
        .unwrap_or(0);
    let end = node
        .end_position()
        .map(|p| p.bytes() as u32)
        .unwrap_or(start);
    start..end
}

/// Build a SourceLocation from a byte offset by scanning the source text.
fn loc_from_byte(source_name: &str, source_text: &str, byte_offset: u32) -> SourceLocation {
    let offset = byte_offset as usize;
    let mut line = 1u32;
    let mut col = 1u32;
    for (i, ch) in source_text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    SourceLocation {
        source_name: source_name.to_string(),
        line,
        column: col,
        byte_offset,
        byte_len: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> (LintDirectives, Vec<Diagnostic>) {
        let ast = full_moon::parse(src).expect("parse failed");
        extract_directives(&ast, "test.lua", src)
    }

    #[test]
    fn file_level_allow() {
        let (dirs, diags) = parse("--# shingetsu: allow(shadowing)\nlocal x = 1");
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::Shadowing),
            Some(&Severity::Allow)
        );
    }

    #[test]
    fn file_level_deny() {
        let (dirs, diags) = parse("--# shingetsu: deny(unused_variable)\nlocal x = 1");
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::UnusedVariable),
            Some(&Severity::Error)
        );
    }

    #[test]
    fn file_level_warn() {
        let (dirs, diags) = parse("--# shingetsu: warn(arg_count)\nlocal x = 1");
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::ArgCount),
            Some(&Severity::Warning)
        );
    }

    #[test]
    fn file_level_multiple_lints() {
        let (dirs, diags) =
            parse("--# shingetsu: allow(shadowing, unused_variable)\nlocal x = 1");
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::Shadowing),
            Some(&Severity::Allow)
        );
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::UnusedVariable),
            Some(&Severity::Allow)
        );
    }

    #[test]
    fn file_level_unknown_lint() {
        let (dirs, diags) = parse("--# shingetsu: allow(bogus_lint)\nlocal x = 1");
        k9::assert_equal!(dirs.file_overrides.len(), 0);
        k9::assert_equal!(diags.len(), 1);
        k9::assert_equal!(diags[0].message, "unknown lint 'bogus_lint'");
        k9::assert_equal!(diags[0].lint, LintId::UnknownLint);
    }

    #[test]
    fn statement_level_allow() {
        let src = "-- shingetsu: allow(shadowing)\nlocal x = 1\nlocal x = 2";
        let (dirs, diags) = parse(src);
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(dirs.statement_overrides.len(), 1);
        k9::assert_equal!(dirs.statement_overrides[0].lint, LintId::Shadowing);
        k9::assert_equal!(dirs.statement_overrides[0].severity, Severity::Allow);
    }

    #[test]
    fn statement_level_multiple_lints() {
        let src = "-- shingetsu: allow(shadowing, unused_variable)\nlocal x = 1";
        let (dirs, diags) = parse(src);
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(dirs.statement_overrides.len(), 2);
    }

    #[test]
    fn statement_level_unknown_lint() {
        let src = "-- shingetsu: allow(not_a_lint)\nlocal x = 1";
        let (dirs, diags) = parse(src);
        k9::assert_equal!(dirs.statement_overrides.len(), 0);
        k9::assert_equal!(diags.len(), 1);
        k9::assert_equal!(diags[0].message, "unknown lint 'not_a_lint'");
    }

    #[test]
    fn non_directive_comment_ignored() {
        let src = "-- this is a normal comment\nlocal x = 1";
        let (dirs, diags) = parse(src);
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(dirs.file_overrides.len(), 0);
        k9::assert_equal!(dirs.statement_overrides.len(), 0);
    }

    #[test]
    fn file_level_only_in_empty_file() {
        let (dirs, diags) = parse("--# shingetsu: allow(shadowing)");
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::Shadowing),
            Some(&Severity::Allow)
        );
    }

    #[test]
    fn multiple_file_level_directives() {
        let src = "--# shingetsu: allow(shadowing)\n--# shingetsu: deny(arg_count)\nlocal x = 1";
        let (dirs, diags) = parse(src);
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::Shadowing),
            Some(&Severity::Allow)
        );
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::ArgCount),
            Some(&Severity::Error)
        );
    }

    #[test]
    fn statement_level_warn() {
        let src = "-- shingetsu: warn(arg_count)\nlocal x = 1";
        let (dirs, diags) = parse(src);
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(dirs.statement_overrides.len(), 1);
        k9::assert_equal!(dirs.statement_overrides[0].lint, LintId::ArgCount);
        k9::assert_equal!(dirs.statement_overrides[0].severity, Severity::Warning);
    }

    #[test]
    fn statement_level_deny() {
        let src = "-- shingetsu: deny(unused_variable)\nlocal x = 1";
        let (dirs, diags) = parse(src);
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(dirs.statement_overrides.len(), 1);
        k9::assert_equal!(dirs.statement_overrides[0].lint, LintId::UnusedVariable);
        k9::assert_equal!(dirs.statement_overrides[0].severity, Severity::Error);
    }

    #[test]
    fn whitespace_variations() {
        let src = "--#   shingetsu:   allow(  shadowing  ,  unused_variable  )\nlocal x = 1";
        let (dirs, diags) = parse(src);
        k9::assert_equal!(diags.len(), 0);
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::Shadowing),
            Some(&Severity::Allow)
        );
        k9::assert_equal!(
            dirs.file_overrides.get(&LintId::UnusedVariable),
            Some(&Severity::Allow)
        );
    }
}
