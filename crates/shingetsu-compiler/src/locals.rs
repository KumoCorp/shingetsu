//! Cursor-aware extraction of local bindings from a Lua source string.
//!
//! Used by REPL completion to surface locals and function parameters that
//! are visible at a given cursor position. Designed to be tolerant of
//! incomplete input — `parse_fallible` recovers; we walk whatever AST it
//! produces.
//!
//! ## Scope handling
//!
//! The current implementation uses a coarse "any local declared earlier in
//! the source is visible" rule plus "function parameters are visible if the
//! cursor falls inside the function body's range". This over-includes for
//! deeply-nested code (e.g. a local in a sibling function leaks into
//! another), but the alternative — full lexical-scope analysis — duplicates
//! work the lowering pass already does. For REPL completion the trade-off
//! favours simplicity: extra candidates are a smaller harm than missing
//! ones.

use full_moon::ast::luau::TypeSpecifier;
use full_moon::ast::{FunctionBody, LocalAssignment, Parameter};
use full_moon::node::Node;
use full_moon::tokenizer::{Lexer, LexerResult, Symbol, TokenType};
use full_moon::visitors::Visitor;
use shingetsu_vm::types::LuaType;
use shingetsu_vm::Bytes;

use crate::type_convert::{convert_type_specifier_ctx, TypeContext};

#[cfg(test)]
mod tests;

/// Return the local bindings visible at byte position `cursor` in `source`.
///
/// Includes:
/// - `local x [: T] = …` declarations whose start byte is at or before
///   `cursor`.
/// - Function parameters of any function body whose byte range contains
///   `cursor`.
///
/// Each returned entry is `(name, type)`. Names may repeat when shadowed;
/// later entries shadow earlier ones — callers that build a name→type map
/// should iterate in order and overwrite.
///
/// The returned [`LuaType`]s come from explicit Luau annotations where
/// present. Without an annotation, `LuaType::Any` is used; literal-RHS
/// inference is intentionally not performed here (it would conflate
/// compile-time intent with runtime value).
pub fn locals_at_cursor(source: &str, cursor: usize) -> Vec<(Bytes, LuaType)> {
    let lua_version = full_moon::LuaVersion::lua55().with_luau();
    let result = full_moon::parse_fallible(source, lua_version);
    let ast = result.ast();

    let mut collector = LocalsCollector {
        cursor,
        locals: Vec::new(),
        found_enclosing_function: false,
    };
    collector.visit_ast(ast);
    let mut locals = collector.locals;

    // Fallback: when the body of a function the cursor is inside contains
    // unparseable content (very common while live-typing), parse_fallible
    // produces a degenerate AST that omits the parameters. Run the
    // token-based scanner only if the visitor didn't already find an
    // enclosing function body, so we don't duplicate entries.
    if !collector.found_enclosing_function {
        let from_tokens = scan_enclosing_function_params(source, cursor);
        locals.extend(from_tokens);
    }
    locals
}

/// Token-based fallback for parameter recovery. Scans the source up to
/// `cursor`, tracks open `function`/`do`/etc. blocks vs `end`s, and if the
/// cursor is inside an unclosed function, returns its parameters. Robust
/// against parser failures because the lexer recovers from invalid input
/// far more gracefully than `parse_fallible`.
fn scan_enclosing_function_params(source: &str, cursor: usize) -> Vec<(Bytes, LuaType)> {
    let lua_version = full_moon::LuaVersion::lua55().with_luau();
    let tokens = match Lexer::new(source, lua_version).collect() {
        LexerResult::Ok(t) | LexerResult::Recovered(t, _) => t,
        LexerResult::Fatal(_) => return Vec::new(),
    };

    // Identify each `function` keyword whose start is before the cursor and
    // whose matching `end` (counting nesting) is after the cursor.
    // Iterate from the end of the relevant tokens, looking for the most
    // recent `function` whose `end` hasn't been seen yet.
    //
    // Strategy: walk forward, push a `function`'s position onto a stack;
    // pop on each `end` (whether it pairs with `do`, `function`, etc. —
    // we treat all `end`-bearing constructs as one stack, which is fine
    // because `do`/`for`/`while` blocks don't introduce parameter-bearing
    // scopes we care about). At cursor, the topmost `function` on the
    // stack is the one whose params should be visible.
    let mut stack: Vec<usize> = Vec::new(); // indices into `tokens`
    let mut seen_funcs: Vec<usize> = Vec::new(); // function token indices on stack

    for (i, tok) in tokens.iter().enumerate() {
        let pos = tok.start_position().bytes();
        if pos >= cursor {
            break;
        }
        match tok.token_type() {
            TokenType::Symbol {
                symbol: Symbol::Function,
            } => {
                stack.push(i);
                seen_funcs.push(i);
            }
            TokenType::Symbol {
                symbol: Symbol::Do | Symbol::Then | Symbol::Repeat,
            } => {
                // We push these too so that nested `end`s don't pop
                // function entries prematurely. Use a sentinel index.
                stack.push(usize::MAX);
            }
            TokenType::Symbol {
                symbol: Symbol::End | Symbol::Until,
            } => {
                if let Some(top) = stack.pop() {
                    if top != usize::MAX {
                        // It was a function — it's now closed.
                        if let Some(p) = seen_funcs.iter().rposition(|&i| i == top) {
                            seen_funcs.remove(p);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Most recent unclosed function.
    let Some(&fn_idx) = seen_funcs.last() else {
        return Vec::new();
    };

    // Find the parameter list: walk forward from `fn_idx` looking for the
    // first `(`. Then collect parameters until the matching `)`.
    let mut paren_idx = None;
    for (i, tok) in tokens.iter().enumerate().skip(fn_idx + 1) {
        if matches!(
            tok.token_type(),
            TokenType::Symbol {
                symbol: Symbol::LeftParen
            }
        ) {
            paren_idx = Some(i);
            break;
        }
    }
    let Some(paren_idx) = paren_idx else {
        return Vec::new();
    };

    // Collect parameter (name, type) pairs. We track the most recent
    // identifier and watch for a `:` followed by type tokens up to the
    // next `,` or `)`. Type info is recovered as a textual run and then
    // re-parsed by full_moon to a TypeInfo — but for the MVP we just keep
    // names and use `LuaType::Any` for the type. Annotations are recovered
    // separately when the full AST visitor succeeds; this fallback only
    // ensures the names are visible for completion at all.
    let mut params: Vec<(Bytes, LuaType)> = Vec::new();
    let mut depth = 0;
    let mut current_name: Option<Bytes> = None;
    let mut current_type: LuaType = LuaType::Any;
    let mut in_type_annotation = false;
    let mut type_name_captured = false;
    for tok in tokens.iter().skip(paren_idx + 1) {
        match tok.token_type() {
            TokenType::Symbol {
                symbol: Symbol::LeftParen,
            } => depth += 1,
            TokenType::Symbol {
                symbol: Symbol::RightParen,
            } => {
                if depth == 0 {
                    if let Some(name) = current_name.take() {
                        params.push((name, current_type.clone()));
                    }
                    break;
                }
                depth -= 1;
            }
            TokenType::Symbol {
                symbol: Symbol::Comma,
            } if depth == 0 => {
                if let Some(name) = current_name.take() {
                    params.push((name, current_type.clone()));
                }
                in_type_annotation = false;
                type_name_captured = false;
                current_type = LuaType::Any;
            }
            TokenType::Symbol {
                symbol: Symbol::Colon,
            } if depth == 0 => {
                in_type_annotation = true;
            }
            TokenType::Identifier { identifier } if !in_type_annotation && depth == 0 => {
                current_name = Some(Bytes::from(identifier.as_str()));
            }
            // First identifier after `:` is the basic type name. We don't
            // attempt to parse complex type expressions (Generic<>, Union
            // `|`, Optional `?`, etc.) — those can wait for parser
            // recovery to give us a proper TypeInfo.
            TokenType::Identifier { identifier }
                if in_type_annotation && depth == 0 && !type_name_captured =>
            {
                current_type = LuaType::from_basic_name(identifier.as_str());
                type_name_captured = true;
            }
            _ => {}
        }
    }

    params
}

struct LocalsCollector {
    cursor: usize,
    locals: Vec<(Bytes, LuaType)>,
    /// Set to true when a function body whose range contains the cursor
    /// is visited successfully — used to suppress the token-based fallback.
    found_enclosing_function: bool,
}

impl LocalsCollector {
    fn ts_to_type(ts: Option<&TypeSpecifier>) -> LuaType {
        ts.map(|t| convert_type_specifier_ctx(t, &TypeContext::empty()))
            .unwrap_or(LuaType::Any)
    }

    fn token_to_bytes(tok: &full_moon::tokenizer::TokenReference) -> Bytes {
        // TokenReference's Display is the token text; for an identifier this
        // is exactly the variable name.
        Bytes::from(tok.token().to_string())
    }
}

impl Visitor for LocalsCollector {
    fn visit_local_assignment(&mut self, node: &LocalAssignment) {
        // Skip if the declaration begins at or after the cursor — a local
        // isn't visible at the byte where its `local` keyword starts.
        let start = match node.start_position() {
            Some(p) => p.bytes(),
            None => return,
        };
        if start >= self.cursor {
            return;
        }
        let names = node.names();
        let type_specs: Vec<Option<&TypeSpecifier>> = node.type_specifiers().collect();
        for (i, name_tok) in names.iter().enumerate() {
            let name = Self::token_to_bytes(name_tok);
            let ty = Self::ts_to_type(type_specs.get(i).copied().flatten());
            self.locals.push((name, ty));
        }
    }

    fn visit_function_body(&mut self, node: &FunctionBody) {
        // Function parameters are visible only if the cursor falls inside
        // this body's textual range.
        let (start, end) = match (node.start_position(), node.end_position()) {
            (Some(s), Some(e)) => (s.bytes(), e.bytes()),
            _ => return,
        };
        if self.cursor < start || self.cursor > end {
            return;
        }
        self.found_enclosing_function = true;
        let params = node.parameters();
        let type_specs: Vec<Option<&TypeSpecifier>> = node.type_specifiers().collect();
        for (i, param) in params.iter().enumerate() {
            let name = match param {
                Parameter::Name(t) => Self::token_to_bytes(t),
                Parameter::Ellipsis(_) => continue,
                _ => continue, // future variants
            };
            let ty = Self::ts_to_type(type_specs.get(i).copied().flatten());
            self.locals.push((name, ty));
        }
    }
}
