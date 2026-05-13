//! Lower full_moon AST nodes to the lint IR.
//!
//! This pass walks the full_moon parse tree and produces the
//! sugar-removed [`Chunk`] / [`Stmt`] / [`Expr`] tree that the lint
//! plugin layer consumes.  Scope tracking is minimal: a stack of
//! frames mapping local names to a monotonically-increasing
//! `binding_id`, sufficient to populate
//! [`ExprKind::Name::binding_id`] and the `is_local` / `is_global`
//! discriminators.
//!
//! What this pass does *not* do:
//!
//! - Doc-comment harvesting -- `Stmt::doc_comment` is left `None`.
//!   The follow-up that wires `compile_with_ast` will harvest doc
//!   comments using the same logic the compiler already uses for
//!   `local_assign` / `TableField` (`harvest_doc_comment` in
//!   `lower.rs`), extended to the additional doc-able statement
//!   kinds called out in `notes/LINT.md`.
//! - Type-annotation parsing -- annotations are captured as their
//!   literal source spelling.  Semantic type queries belong to
//!   `ctx.type_of(node)` in the plugin API.
//! - `has_trailing_multret` detection for `f "string"` and
//!   `f { table }` sugar -- both forms always produce a single
//!   argument, so the flag is `false` for them.  Only
//!   parenthesised call forms with a trailing call/vararg expression
//!   set the flag.
//!
//! ## Unsupported nodes
//!
//! full_moon's AST enums (`Stmt`, `LastStmt`, `Expression`, `Var`,
//! `Prefix`, `Suffix`, `Field`, `Parameter`, `BinOp`, `UnOp`,
//! `CompoundOp`) are `#[non_exhaustive]`, so exhaustive matching
//! from outside their crate is impossible.  Each catch-all arm
//! records an [`UnsupportedNode`] entry on the lowering state and
//! produces a placeholder IR node (e.g. `ExprKind::Nil` for an
//! unrecognised expression, `BinOp::Add` for an unrecognised
//! operator) so the surrounding tree remains walkable.  The
//! placeholder is *not* a silent fallback: [`Lowered::unsupported`]
//! collects every such site, and `compile_with_ast` surfaces them
//! as compiler diagnostics so a future full_moon variant cannot
//! drift past us unnoticed.

use super::*;
use full_moon::ast;
use full_moon::node::Node;
use full_moon::tokenizer::{Position, TokenReference, TokenType};
use shingetsu_vm::Bytes;
use std::collections::HashMap;

/// Output of [`lower`]: the resulting [`Chunk`] plus a list of
/// AST nodes the lowering didn't recognise.
///
/// The `unsupported` list is the load-bearing protection against
/// silent drift if full_moon adds variants we haven't taught the
/// lint IR about.  Callers should surface every entry as a
/// diagnostic (typically a [`crate::error::BuiltInLintId`]-flavoured
/// warning) rather than ignoring it.
pub struct Lowered {
    pub chunk: Chunk,
    pub unsupported: Vec<UnsupportedNode>,
}

/// One AST node the lowering pass didn't recognise.  Carries enough
/// context (kind name, source span, original source spelling) to
/// emit a useful diagnostic without the caller re-walking the AST.
#[derive(Debug, Clone, PartialEq)]
pub struct UnsupportedNode {
    /// Human-readable variant identifier, e.g. `"ast::Stmt"` or
    /// `"ast::BinOp"`.  Static for the well-known catch-all sites.
    pub kind_name: &'static str,
    /// Source text covered by the unrecognised node.  Useful in
    /// diagnostics; truncated by the renderer when very long.
    pub source_text: String,
    pub span: Span,
}

/// Lower a full_moon AST to the lint IR.  Returns the resulting
/// [`Lowered`] including any [`UnsupportedNode`] entries the pass
/// produced.
pub fn lower(ast: &ast::Ast) -> Lowered {
    let mut lowering = Lowering::default();
    let span = node_span(ast).unwrap_or(EMPTY_SPAN);
    lowering.push_scope();
    let block = lowering.lower_block(ast.nodes());
    lowering.pop_scope();
    Lowered {
        chunk: Chunk { block, span },
        unsupported: lowering.unsupported,
    }
}

const EMPTY_SPAN: Span = Span {
    start_byte: 0,
    end_byte: 0,
    start_line: 0,
    start_col: 0,
    end_line: 0,
    end_col: 0,
};

/// Per-lowering state: a binding-id counter, a stack of scopes, and
/// a collector for unsupported AST nodes.
#[derive(Default)]
struct Lowering {
    next_binding_id: u32,
    scopes: Vec<HashMap<Bytes, u32>>,
    unsupported: Vec<UnsupportedNode>,
}

impl Lowering {
    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Declare a new local in the innermost scope.  Returns the
    /// allocated binding id.  Shadows any outer binding of the same
    /// name within the innermost scope; the outer binding becomes
    /// invisible until this scope pops.
    fn declare_local(&mut self, name: impl Into<Bytes>) -> u32 {
        let id = self.next_binding_id;
        self.next_binding_id += 1;
        if let Some(frame) = self.scopes.last_mut() {
            frame.insert(name.into(), id);
        }
        id
    }

    /// Resolve a name reference.  Searches scopes innermost-first;
    /// returns `None` if the name is not bound in any scope (i.e.
    /// it's a global).
    fn resolve(&self, name: &Bytes) -> Option<u32> {
        for scope in self.scopes.iter().rev() {
            if let Some(id) = scope.get(name) {
                return Some(*id);
            }
        }
        None
    }

    /// Record that a full_moon node of `kind_name` couldn't be
    /// lowered.  The caller is responsible for emitting a
    /// placeholder IR node so the surrounding tree stays walkable.
    fn track_unsupported<N: Node + std::fmt::Display>(
        &mut self,
        kind_name: &'static str,
        node: &N,
    ) {
        let span = node_span(node).unwrap_or(EMPTY_SPAN);
        self.unsupported.push(UnsupportedNode {
            kind_name,
            source_text: node.to_string(),
            span,
        });
    }

    /// Like [`Self::track_unsupported`] but for cases where we have
    /// a span up front and a stringly description (e.g. an
    /// operator we don't recognise where the surrounding context
    /// already knows the span).
    fn track_unsupported_at(&mut self, kind_name: &'static str, source_text: String, span: Span) {
        self.unsupported.push(UnsupportedNode {
            kind_name,
            source_text,
            span,
        });
    }

    // ----- blocks and statements ----------------------------------------

    fn lower_block(&mut self, block: &ast::Block) -> Block {
        let span = node_span(block).unwrap_or(EMPTY_SPAN);
        let mut stmts: Vec<Stmt> = Vec::new();
        for s in block.stmts() {
            stmts.push(self.lower_stmt(s));
        }
        if let Some(last) = block.last_stmt() {
            stmts.push(self.lower_last_stmt(last));
        }
        Block { stmts, span }
    }

    fn lower_stmt(&mut self, stmt: &ast::Stmt) -> Stmt {
        let span = node_span(stmt).unwrap_or(EMPTY_SPAN);
        let doc_comment = crate::lower::doc_text(stmt);
        let kind = self.lower_stmt_kind(stmt, span);
        Stmt {
            kind,
            span,
            doc_comment,
        }
    }

    fn lower_stmt_kind(&mut self, stmt: &ast::Stmt, span: Span) -> StmtKind {
        match stmt {
            ast::Stmt::Assignment(a) => {
                let targets: Vec<Expr> = a.variables().iter().map(|v| self.lower_var(v)).collect();
                let values: Vec<Expr> =
                    a.expressions().iter().map(|e| self.lower_expr(e)).collect();
                StmtKind::Assign(Assign {
                    targets,
                    values,
                    span,
                    doc_comment: None,
                })
            }
            ast::Stmt::LocalAssignment(la) => {
                let values: Vec<Expr> = la
                    .expressions()
                    .iter()
                    .map(|e| self.lower_expr(e))
                    .collect();
                let mut names: Vec<Param> = Vec::new();
                for (i, tok) in la.names().iter().enumerate() {
                    let name = tok_str(tok);
                    let name_span = tok_span(tok);
                    let type_annotation = la
                        .type_specifiers()
                        .nth(i)
                        .and_then(|ts| ts.map(type_annotation_from));
                    let attribute = la
                        .attributes()
                        .nth(i)
                        .and_then(|a| a.map(|attr| self.attribute_from_token(attr.name())))
                        .flatten();
                    self.declare_local(name.clone());
                    names.push(Param {
                        name,
                        name_span,
                        type_annotation,
                        default: None,
                        attribute,
                    });
                }
                StmtKind::LocalAssign { names, values }
            }
            ast::Stmt::FunctionCall(fc) => {
                let expr = self.lower_function_call(fc);
                StmtKind::ExprStatement { expr }
            }
            ast::Stmt::Do(d) => {
                self.push_scope();
                let block = self.lower_block(d.block());
                self.pop_scope();
                StmtKind::DoBlock { block }
            }
            ast::Stmt::While(w) => {
                let cond = self.lower_expr(w.condition());
                self.push_scope();
                let block = self.lower_block(w.block());
                self.pop_scope();
                StmtKind::While { cond, block }
            }
            ast::Stmt::Repeat(r) => {
                // `repeat ... until cond`: the condition can see
                // locals introduced inside the body, so we keep the
                // scope alive while lowering the condition and pop
                // after.
                self.push_scope();
                let block = self.lower_block(r.block());
                let cond = self.lower_expr(r.until());
                self.pop_scope();
                StmtKind::Repeat { block, cond }
            }
            ast::Stmt::If(i) => {
                let cond = self.lower_expr(i.condition());
                self.push_scope();
                let block = self.lower_block(i.block());
                self.pop_scope();
                let mut branches = vec![Branch { cond, block }];
                if let Some(elseifs) = i.else_if() {
                    for elif in elseifs {
                        let cond = self.lower_expr(elif.condition());
                        self.push_scope();
                        let block = self.lower_block(elif.block());
                        self.pop_scope();
                        branches.push(Branch { cond, block });
                    }
                }
                let else_block = i.else_block().map(|b| {
                    self.push_scope();
                    let blk = self.lower_block(b);
                    self.pop_scope();
                    blk
                });
                StmtKind::If {
                    branches,
                    else_block,
                }
            }
            ast::Stmt::NumericFor(nf) => {
                let start = self.lower_expr(nf.start());
                let stop = self.lower_expr(nf.end());
                let step = nf.step().map(|e| self.lower_expr(e));
                self.push_scope();
                let var_name = tok_str(nf.index_variable());
                let var_span = tok_span(nf.index_variable());
                let var_type = nf.type_specifier().map(type_annotation_from);
                self.declare_local(var_name.clone());
                let var = Param {
                    name: var_name,
                    name_span: var_span,
                    type_annotation: var_type,
                    default: None,
                    attribute: None,
                };
                let block = self.lower_block(nf.block());
                self.pop_scope();
                StmtKind::NumericFor {
                    var,
                    start,
                    stop,
                    step,
                    block,
                }
            }
            ast::Stmt::GenericFor(gf) => {
                let exprs: Vec<Expr> = gf
                    .expressions()
                    .iter()
                    .map(|e| self.lower_expr(e))
                    .collect();
                self.push_scope();
                let mut vars: Vec<Param> = Vec::new();
                for (i, tok) in gf.names().iter().enumerate() {
                    let name = tok_str(tok);
                    let name_span = tok_span(tok);
                    let ty = gf
                        .type_specifiers()
                        .nth(i)
                        .and_then(|t| t.map(type_annotation_from));
                    self.declare_local(name.clone());
                    vars.push(Param {
                        name,
                        name_span,
                        type_annotation: ty,
                        default: None,
                        attribute: None,
                    });
                }
                let block = self.lower_block(gf.block());
                self.pop_scope();
                StmtKind::GenericFor { vars, exprs, block }
            }
            ast::Stmt::FunctionDeclaration(fd) => {
                let target = self.lower_function_decl_target(fd.name());
                let is_method = fd.name().method_name().is_some();
                let (params, is_variadic, generics, return_type, body) =
                    self.lower_function_body(fd.body(), is_method);
                StmtKind::FunctionDecl {
                    target,
                    is_method,
                    params,
                    is_variadic,
                    generics,
                    return_type,
                    body,
                }
            }
            ast::Stmt::LocalFunction(lf) => {
                let name = tok_str(lf.name());
                let name_span = tok_span(lf.name());
                self.declare_local(name.clone());
                let (params, is_variadic, generics, return_type, body) =
                    self.lower_function_body(lf.body(), false);
                StmtKind::LocalFunction {
                    name,
                    name_span,
                    params,
                    is_variadic,
                    generics,
                    return_type,
                    body,
                }
            }
            ast::Stmt::Goto(g) => {
                let label = tok_str(g.label_name());
                let label_span = tok_span(g.label_name());
                StmtKind::Goto { label, label_span }
            }
            ast::Stmt::Label(l) => {
                let name = tok_str(l.name());
                StmtKind::Label { name }
            }
            ast::Stmt::CompoundAssignment(ca) => {
                let target = self.lower_var(ca.lhs());
                let value = self.lower_expr(ca.rhs());
                let (op, op_span) = self.compound_op_to_binop(ca.compound_operator());
                StmtKind::CompoundAssign {
                    target,
                    op,
                    op_span,
                    value,
                }
            }
            ast::Stmt::ConstAssignment(ca) => {
                // Luau's `const a, b = x, y` accepts the same multi-name
                // shape as a `local` assignment.  We preserve the
                // syntactic distinction only when the source used the
                // single-name spelling (`const x = v`); multi-name cases
                // lower to LocalAssign with each Param's attribute set
                // to `Attribute::Const` since the underlying semantics
                // are identical and lints generally want to see the
                // common shape.
                let values: Vec<Expr> = ca
                    .expressions()
                    .iter()
                    .map(|e| self.lower_expr(e))
                    .collect();
                let mut names: Vec<Param> = Vec::new();
                for (i, tok) in ca.names().iter().enumerate() {
                    let name = tok_str(tok);
                    let name_span = tok_span(tok);
                    let type_annotation = ca
                        .type_specifiers()
                        .nth(i)
                        .and_then(|t| t.map(type_annotation_from));
                    self.declare_local(name.clone());
                    names.push(Param {
                        name,
                        name_span,
                        type_annotation,
                        default: None,
                        attribute: Some(Attribute::Const),
                    });
                }
                if names.len() == 1 {
                    let name = names.into_iter().next().expect("len == 1");
                    let value = values.into_iter().next().unwrap_or(Expr {
                        kind: ExprKind::Nil,
                        span: EMPTY_SPAN,
                        was_parenthesized: false,
                    });
                    StmtKind::ConstAssign { name, value }
                } else {
                    StmtKind::LocalAssign { names, values }
                }
            }
            ast::Stmt::ConstFunction(cf) => {
                let name_tok = cf.name();
                let name = tok_str(name_tok);
                let name_span = tok_span(name_tok);
                self.declare_local(name.clone());
                let (params, is_variadic, generics, return_type, body) =
                    self.lower_function_body(cf.body(), false);
                StmtKind::ConstFunction {
                    name,
                    name_span,
                    params,
                    is_variadic,
                    generics,
                    return_type,
                    body,
                }
            }
            ast::Stmt::Global(g) => match g.as_ref() {
                full_moon::ast::lua55::Global::Assignment(ga) => {
                    let mut names: Vec<Bytes> = Vec::new();
                    let mut name_spans: Vec<Span> = Vec::new();
                    let mut type_annotations: Vec<Option<TypeAnnotation>> = Vec::new();
                    for (i, tok) in ga.names().iter().enumerate() {
                        names.push(tok_str(tok));
                        name_spans.push(tok_span(tok));
                        type_annotations.push(
                            ga.type_specifiers()
                                .nth(i)
                                .and_then(|t| t.map(type_annotation_from)),
                        );
                    }
                    StmtKind::GlobalDecl {
                        names,
                        name_spans,
                        type_annotations,
                    }
                }
                // `global *` -- the wildcard form has no per-name
                // surface to lower.  Represent it as an empty
                // [`StmtKind::GlobalDecl`]; plugins can still see
                // the surrounding statement span.  Not tracked as
                // unsupported because we intentionally chose this
                // representation.
                full_moon::ast::lua55::Global::Wildcard(_) => StmtKind::GlobalDecl {
                    names: vec![],
                    name_spans: vec![],
                    type_annotations: vec![],
                },
                other => {
                    self.track_unsupported_at("ast::Stmt::Global", other.to_string(), span);
                    StmtKind::GlobalDecl {
                        names: vec![],
                        name_spans: vec![],
                        type_annotations: vec![],
                    }
                }
            },
            ast::Stmt::TypeDeclaration(td) => {
                let name_tok = td.type_name();
                let name = tok_str(name_tok);
                let name_span = tok_span(name_tok);
                let generics = self.lower_generic_decl(td.generics());
                let body = TypeAnnotation {
                    source: td.type_definition().to_string(),
                    span: node_span(td.type_definition()).unwrap_or(EMPTY_SPAN),
                };
                StmtKind::TypeAlias {
                    name,
                    name_span,
                    generics,
                    body,
                    exported: false,
                }
            }
            ast::Stmt::ExportedTypeDeclaration(etd) => {
                let td = etd.type_declaration();
                let name_tok = td.type_name();
                let name = tok_str(name_tok);
                let name_span = tok_span(name_tok);
                let generics = self.lower_generic_decl(td.generics());
                let body = TypeAnnotation {
                    source: td.type_definition().to_string(),
                    span: node_span(td.type_definition()).unwrap_or(EMPTY_SPAN),
                };
                StmtKind::TypeAlias {
                    name,
                    name_span,
                    generics,
                    body,
                    exported: true,
                }
            }
            other => {
                self.track_unsupported("ast::Stmt", other);
                StmtKind::DoBlock {
                    block: Block {
                        stmts: vec![],
                        span,
                    },
                }
            }
        }
    }

    fn lower_last_stmt(&mut self, stmt: &ast::LastStmt) -> Stmt {
        let span = node_span(stmt).unwrap_or(EMPTY_SPAN);
        let kind = match stmt {
            ast::LastStmt::Return(r) => {
                let values: Vec<Expr> = r.returns().iter().map(|e| self.lower_expr(e)).collect();
                StmtKind::Return { values }
            }
            ast::LastStmt::Break(_) => StmtKind::Break,
            ast::LastStmt::Continue(_) => StmtKind::Continue,
            other => {
                self.track_unsupported("ast::LastStmt", other);
                StmtKind::Break
            }
        };
        Stmt {
            kind,
            span,
            doc_comment: None,
        }
    }

    // ----- expressions --------------------------------------------------

    fn lower_expr(&mut self, e: &ast::Expression) -> Expr {
        let span = node_span(e).unwrap_or(EMPTY_SPAN);
        self.lower_expr_kind(e, span)
    }

    /// Lower an [`ast::Expression`] into an [`Expr`] with the given
    /// outer span.  The `Parentheses` arm reuses the inner
    /// expression's kind and rewrites its span / parenthesization
    /// flag; every other arm constructs a fresh [`Expr`] at the
    /// supplied span.
    fn lower_expr_kind(&mut self, e: &ast::Expression, span: Span) -> Expr {
        match e {
            ast::Expression::Parentheses { expression, .. } => {
                let mut inner = self.lower_expr(expression);
                inner.span = span;
                inner.was_parenthesized = true;
                inner
            }
            ast::Expression::Number(tok) => {
                let raw = tok.token().to_string();
                let value = raw.parse::<f64>().unwrap_or(0.0);
                Expr {
                    kind: ExprKind::NumberLiteral { value, raw },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::String(tok) => {
                let (raw, value, long_depth) = string_literal_parts(tok);
                Expr {
                    kind: ExprKind::StringLiteral {
                        raw,
                        value,
                        long_depth,
                    },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::Symbol(tok) => {
                let kind = match tok.token().token_type() {
                    TokenType::Symbol { symbol } => match symbol.to_string().as_str() {
                        "nil" => ExprKind::Nil,
                        "true" => ExprKind::BoolLiteral(true),
                        "false" => ExprKind::BoolLiteral(false),
                        "..." => ExprKind::Vararg,
                        _ => {
                            self.track_unsupported_at(
                                "ast::Expression::Symbol",
                                tok.to_string(),
                                span,
                            );
                            ExprKind::Nil
                        }
                    },
                    _ => {
                        self.track_unsupported_at("ast::Expression::Symbol", tok.to_string(), span);
                        ExprKind::Nil
                    }
                };
                Expr {
                    kind,
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::TableConstructor(tc) => {
                let entries = self.lower_table_constructor(tc);
                Expr {
                    kind: ExprKind::TableConstructor { entries },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::Function(anon) => {
                let body = anon.body();
                let (params, is_variadic, generics, return_type, block) =
                    self.lower_function_body(body, false);
                Expr {
                    kind: ExprKind::FunctionExpr {
                        params,
                        is_variadic,
                        generics,
                        return_type,
                        body: block,
                    },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::FunctionCall(fc) => self.lower_function_call(fc),
            ast::Expression::Var(v) => self.lower_var(v),
            ast::Expression::BinaryOperator { lhs, binop, rhs } => {
                let lhs = Box::new(self.lower_expr(lhs));
                let rhs = Box::new(self.lower_expr(rhs));
                let (op, op_span) = self.bin_op_to_ir(binop);
                Expr {
                    kind: ExprKind::BinOp {
                        op,
                        op_span,
                        lhs,
                        rhs,
                    },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::UnaryOperator { unop, expression } => {
                let operand = Box::new(self.lower_expr(expression));
                let (op, op_span) = self.un_op_to_ir(unop);
                Expr {
                    kind: ExprKind::UnOp {
                        op,
                        op_span,
                        operand,
                    },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::IfExpression(ie) => {
                let cond = self.lower_expr(ie.condition());
                let value = self.lower_expr(ie.if_expression());
                let mut branches = vec![ExprBranch { cond, value }];
                if let Some(elseifs) = ie.else_if_expressions() {
                    for elif in elseifs {
                        branches.push(ExprBranch {
                            cond: self.lower_expr(elif.condition()),
                            value: self.lower_expr(elif.expression()),
                        });
                    }
                }
                let else_expr = Box::new(self.lower_expr(ie.else_expression()));
                Expr {
                    kind: ExprKind::IfExpression {
                        branches,
                        else_expr,
                    },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::InterpolatedString(is) => {
                let mut parts: Vec<InterpPart> = Vec::new();
                for segment in is.segments() {
                    let raw = segment.literal.token().to_string();
                    if !raw.is_empty() {
                        parts.push(InterpPart::Literal(Bytes::from(raw.as_bytes())));
                    }
                    parts.push(InterpPart::Expr(self.lower_expr(&segment.expression)));
                }
                let tail = is.last_string().token().to_string();
                if !tail.is_empty() {
                    parts.push(InterpPart::Literal(Bytes::from(tail.as_bytes())));
                }
                Expr {
                    kind: ExprKind::InterpString { parts },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Expression::TypeAssertion {
                expression,
                type_assertion,
            } => {
                let expr = Box::new(self.lower_expr(expression));
                let annotation = TypeAnnotation {
                    source: type_assertion.cast_to().to_string(),
                    span: node_span(type_assertion.cast_to()).unwrap_or(EMPTY_SPAN),
                };
                Expr {
                    kind: ExprKind::TypeAssertion { expr, annotation },
                    span,
                    was_parenthesized: false,
                }
            }
            other => {
                self.track_unsupported("ast::Expression", other);
                Expr {
                    kind: ExprKind::Nil,
                    span,
                    was_parenthesized: false,
                }
            }
        }
    }

    /// Lower a [`ast::Var`] to an IR expression.  `Var::Name` is a
    /// bare identifier; `Var::Expression` is a prefix + suffix chain
    /// whose last suffix is an index (the chain is the lvalue).
    fn lower_var(&mut self, v: &ast::Var) -> Expr {
        match v {
            ast::Var::Name(tok) => {
                let span = tok_span(tok);
                let name = tok_str(tok);
                let kind = self.name_expr_kind(name);
                Expr {
                    kind,
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Var::Expression(ve) => {
                let mut current = self.lower_prefix(ve.prefix());
                for suffix in ve.suffixes() {
                    current = self.apply_suffix(current, suffix);
                }
                current
            }
            other => {
                self.track_unsupported("ast::Var", other);
                Expr {
                    kind: ExprKind::Nil,
                    span: node_span(other).unwrap_or(EMPTY_SPAN),
                    was_parenthesized: false,
                }
            }
        }
    }

    fn name_expr_kind(&self, name: Bytes) -> ExprKind {
        let binding_id = self.resolve(&name);
        let is_local = binding_id.is_some();
        ExprKind::Name {
            name,
            is_global: !is_local,
            is_local,
            binding_id,
        }
    }

    fn lower_prefix(&mut self, p: &ast::Prefix) -> Expr {
        match p {
            ast::Prefix::Name(tok) => {
                let span = tok_span(tok);
                let name = tok_str(tok);
                let kind = self.name_expr_kind(name);
                Expr {
                    kind,
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Prefix::Expression(e) => self.lower_expr(e),
            other => {
                self.track_unsupported("ast::Prefix", other);
                Expr {
                    kind: ExprKind::Nil,
                    span: node_span(other).unwrap_or(EMPTY_SPAN),
                    was_parenthesized: false,
                }
            }
        }
    }

    /// Apply a single suffix (`.foo`, `[k]`, `(args)`, or `:m(args)`)
    /// to a target expression, producing a new expression that
    /// includes the suffix.  Span derivation reuses the target's
    /// start position via struct-update syntax.
    fn apply_suffix(&mut self, target: Expr, suffix: &ast::Suffix) -> Expr {
        match suffix {
            ast::Suffix::Index(ast::Index::Dot { name, .. }) => {
                let name_str = tok_str(name);
                let name_span = tok_span(name);
                let span = Span {
                    end_byte: name_span.end_byte,
                    end_line: name_span.end_line,
                    end_col: name_span.end_col,
                    ..target.span
                };
                Expr {
                    kind: ExprKind::Field {
                        target: Box::new(target),
                        name: name_str,
                        name_span,
                    },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Suffix::Index(ast::Index::Brackets { expression, .. }) => {
                let key = self.lower_expr(expression);
                let span = Span {
                    end_byte: key.span.end_byte.saturating_add(1),
                    end_line: key.span.end_line,
                    end_col: key.span.end_col.saturating_add(1),
                    ..target.span
                };
                Expr {
                    kind: ExprKind::Index {
                        target: Box::new(target),
                        key: Box::new(key),
                    },
                    span,
                    was_parenthesized: false,
                }
            }
            ast::Suffix::Call(ast::Call::AnonymousCall(args)) => {
                let (args, has_trailing_multret) = self.lower_function_args(args);
                let end_byte = args
                    .last()
                    .map(|a| a.span.end_byte)
                    .unwrap_or(target.span.end_byte)
                    .saturating_add(1);
                let span = Span {
                    end_byte,
                    ..target.span
                };
                Expr {
                    kind: ExprKind::FunctionCall(FunctionCall {
                        callee: Box::new(target),
                        args,
                        has_trailing_multret,
                        span,
                        doc_comment: None,
                    }),
                    span,
                    was_parenthesized: false,
                }
            }
            // Luau type instantiation `f<<T>>(...)` -- the
            // generic-args suffix.  It carries no run-time
            // observable structure of its own; semantic type info
            // attaches to the surrounding call via `ctx.type_of`.
            // Lower as a no-op (pass the target through) so the
            // following call suffix produces a regular
            // FunctionCall / MethodCall.
            ast::Suffix::TypeInstantiation(_) => target,
            ast::Suffix::Call(ast::Call::MethodCall(mc)) => {
                let method = tok_str(mc.name());
                let method_span = tok_span(mc.name());
                let (args, has_trailing_multret) = self.lower_function_args(mc.args());
                let end_byte = args
                    .last()
                    .map(|a| a.span.end_byte)
                    .unwrap_or(method_span.end_byte)
                    .saturating_add(1);
                let span = Span {
                    end_byte,
                    end_line: method_span.end_line,
                    end_col: method_span.end_col,
                    ..target.span
                };
                Expr {
                    kind: ExprKind::MethodCall(MethodCall {
                        receiver: Box::new(target),
                        method,
                        method_span,
                        args,
                        has_trailing_multret,
                        span,
                        doc_comment: None,
                    }),
                    span,
                    was_parenthesized: false,
                }
            }
            other => {
                self.track_unsupported("ast::Suffix", other);
                target
            }
        }
    }

    /// Lower a `function_call` AST node.  The result is always a
    /// [`ExprKind::FunctionCall`] or [`ExprKind::MethodCall`]
    /// (whichever the trailing suffix produces).
    fn lower_function_call(&mut self, fc: &ast::FunctionCall) -> Expr {
        let mut current = self.lower_prefix(fc.prefix());
        for suffix in fc.suffixes() {
            current = self.apply_suffix(current, suffix);
        }
        current
    }

    fn lower_function_args(&mut self, args: &ast::FunctionArgs) -> (Vec<Expr>, bool) {
        match args {
            ast::FunctionArgs::Parentheses { arguments, .. } => {
                let exprs: Vec<Expr> = arguments.iter().map(|a| self.lower_expr(a)).collect();
                let trailing = exprs.last().map(is_multret_kind).unwrap_or(false);
                (exprs, trailing)
            }
            ast::FunctionArgs::String(tok) => {
                let (raw, value, long_depth) = string_literal_parts(tok);
                let span = tok_span(tok);
                let arg = Expr {
                    kind: ExprKind::StringLiteral {
                        raw,
                        value,
                        long_depth,
                    },
                    span,
                    was_parenthesized: false,
                };
                (vec![arg], false)
            }
            ast::FunctionArgs::TableConstructor(tc) => {
                let entries = self.lower_table_constructor(tc);
                let span = node_span(tc).unwrap_or(EMPTY_SPAN);
                let arg = Expr {
                    kind: ExprKind::TableConstructor { entries },
                    span,
                    was_parenthesized: false,
                };
                (vec![arg], false)
            }
            other => {
                self.track_unsupported("ast::FunctionArgs", other);
                (vec![], false)
            }
        }
    }

    fn lower_table_constructor(&mut self, tc: &ast::TableConstructor) -> Vec<TableEntry> {
        let mut out: Vec<TableEntry> = Vec::new();
        for (field, _sep) in tc.fields().pairs().map(|p| (p.value(), p.punctuation())) {
            let span = node_span(field).unwrap_or(EMPTY_SPAN);
            let kind = match field {
                ast::Field::ExpressionKey { key, value, .. } => TableEntryKind::Hash {
                    key: self.lower_expr(key),
                    value: self.lower_expr(value),
                },
                ast::Field::NameKey { key, value, .. } => TableEntryKind::Named {
                    name: tok_str(key),
                    name_span: tok_span(key),
                    value: self.lower_expr(value),
                },
                ast::Field::NoKey(e) => TableEntryKind::Array {
                    value: self.lower_expr(e),
                },
                other => {
                    self.track_unsupported("ast::Field", other);
                    TableEntryKind::Array {
                        value: Expr {
                            kind: ExprKind::Nil,
                            span,
                            was_parenthesized: false,
                        },
                    }
                }
            };
            out.push(TableEntry { span, kind });
        }
        out
    }

    /// Lower a function body.  Declares each named parameter in a
    /// fresh scope; the caller is responsible for `push_scope` /
    /// `pop_scope` for any binding the function itself introduces.
    /// `is_method` adds an implicit `self` parameter declaration
    /// (not surfaced in the returned `params` list).
    fn lower_function_body(
        &mut self,
        body: &ast::FunctionBody,
        is_method: bool,
    ) -> (
        Vec<Param>,
        bool,
        Vec<TypeParam>,
        Option<TypeAnnotation>,
        Block,
    ) {
        self.push_scope();
        if is_method {
            self.declare_local("self");
        }
        let mut params: Vec<Param> = Vec::new();
        let mut is_variadic = false;
        for (i, p) in body.parameters().iter().enumerate() {
            match p {
                ast::Parameter::Name(tok) => {
                    let name = tok_str(tok);
                    let name_span = tok_span(tok);
                    let type_annotation = body
                        .type_specifiers()
                        .nth(i)
                        .and_then(|t| t.map(type_annotation_from));
                    self.declare_local(name.clone());
                    params.push(Param {
                        name,
                        name_span,
                        type_annotation,
                        default: None,
                        attribute: None,
                    });
                }
                ast::Parameter::Ellipsis(_) => {
                    is_variadic = true;
                }
                other => {
                    self.track_unsupported("ast::Parameter", other);
                }
            }
        }
        let generics = self.lower_generic_decl(body.generics());
        let return_type = body
            .return_type()
            .map(|t| type_annotation_from(t.type_info()));
        let block = self.lower_block(body.block());
        self.pop_scope();
        (params, is_variadic, generics, return_type, block)
    }

    /// Lower the LHS of a `function foo.bar:baz(...) ... end`
    /// declaration into a chain of [`ExprKind::Field`] (with the
    /// method-colon name folded onto the chain as a final field, so
    /// the IR expression's shape mirrors the source).  The
    /// surrounding [`StmtKind::FunctionDecl::is_method`] flag tells
    /// plugins whether the colon was used.
    fn lower_function_decl_target(&mut self, name: &ast::FunctionName) -> Expr {
        let mut iter = name.names().iter();
        let first = iter.next().expect("function declaration name is non-empty");
        let mut current = {
            let span = tok_span(first);
            let n = tok_str(first);
            Expr {
                kind: self.name_expr_kind(n),
                span,
                was_parenthesized: false,
            }
        };
        for tok in iter {
            let n = tok_str(tok);
            let name_span = tok_span(tok);
            let span = Span {
                end_byte: name_span.end_byte,
                end_line: name_span.end_line,
                end_col: name_span.end_col,
                ..current.span
            };
            current = Expr {
                kind: ExprKind::Field {
                    target: Box::new(current),
                    name: n,
                    name_span,
                },
                span,
                was_parenthesized: false,
            };
        }
        if let Some(method_tok) = name.method_name() {
            let n = tok_str(method_tok);
            let name_span = tok_span(method_tok);
            let span = Span {
                end_byte: name_span.end_byte,
                end_line: name_span.end_line,
                end_col: name_span.end_col,
                ..current.span
            };
            current = Expr {
                kind: ExprKind::Field {
                    target: Box::new(current),
                    name: n,
                    name_span,
                },
                span,
                was_parenthesized: false,
            };
        }
        current
    }

    // ----- operator mappings --------------------------------------------

    fn bin_op_to_ir(&mut self, op: &ast::BinOp) -> (BinOp, Span) {
        let tok = op.token();
        let span = tok_span(tok);
        let mapped = match op {
            ast::BinOp::And(_) => BinOp::And,
            ast::BinOp::Caret(_) => BinOp::Pow,
            ast::BinOp::GreaterThan(_) => BinOp::Gt,
            ast::BinOp::GreaterThanEqual(_) => BinOp::GtEq,
            ast::BinOp::LessThan(_) => BinOp::Lt,
            ast::BinOp::LessThanEqual(_) => BinOp::LtEq,
            ast::BinOp::Minus(_) => BinOp::Sub,
            ast::BinOp::Or(_) => BinOp::Or,
            ast::BinOp::Percent(_) => BinOp::Mod,
            ast::BinOp::Plus(_) => BinOp::Add,
            ast::BinOp::Slash(_) => BinOp::Div,
            ast::BinOp::DoubleSlash(_) => BinOp::FloorDiv,
            ast::BinOp::Star(_) => BinOp::Mul,
            ast::BinOp::TildeEqual(_) => BinOp::NotEq,
            ast::BinOp::TwoDots(_) => BinOp::Concat,
            ast::BinOp::TwoEqual(_) => BinOp::Eq,
            ast::BinOp::Ampersand(_) => BinOp::BitAnd,
            ast::BinOp::DoubleGreaterThan(_) => BinOp::Shr,
            ast::BinOp::DoubleLessThan(_) => BinOp::Shl,
            ast::BinOp::Pipe(_) => BinOp::BitOr,
            ast::BinOp::Tilde(_) => BinOp::BitXor,
            other => {
                self.track_unsupported_at("ast::BinOp", other.to_string(), span);
                BinOp::Add
            }
        };
        (mapped, span)
    }

    fn un_op_to_ir(&mut self, op: &ast::UnOp) -> (UnOp, Span) {
        let span = tok_span(op.token());
        let mapped = match op {
            ast::UnOp::Minus(_) => UnOp::Neg,
            ast::UnOp::Not(_) => UnOp::Not,
            ast::UnOp::Hash(_) => UnOp::Len,
            ast::UnOp::Tilde(_) => UnOp::BitNot,
            other => {
                self.track_unsupported_at("ast::UnOp", other.to_string(), span);
                UnOp::Not
            }
        };
        (mapped, span)
    }

    fn compound_op_to_binop(&mut self, op: &ast::CompoundOp) -> (BinOp, Span) {
        let span = tok_span(op.token());
        let mapped = match op {
            ast::CompoundOp::PlusEqual(_) => BinOp::Add,
            ast::CompoundOp::MinusEqual(_) => BinOp::Sub,
            ast::CompoundOp::StarEqual(_) => BinOp::Mul,
            ast::CompoundOp::SlashEqual(_) => BinOp::Div,
            ast::CompoundOp::CaretEqual(_) => BinOp::Pow,
            ast::CompoundOp::DoubleSlashEqual(_) => BinOp::FloorDiv,
            ast::CompoundOp::PercentEqual(_) => BinOp::Mod,
            ast::CompoundOp::TwoDotsEqual(_) => BinOp::Concat,
            other => {
                self.track_unsupported_at("ast::CompoundOp", other.to_string(), span);
                BinOp::Add
            }
        };
        (mapped, span)
    }

    fn attribute_from_token(&mut self, tok: &TokenReference) -> Option<Attribute> {
        match tok.token().to_string().as_str() {
            "const" => Some(Attribute::Const),
            "close" => Some(Attribute::Close),
            other => {
                let span = tok_span(tok);
                self.track_unsupported_at("local-attribute", other.to_string(), span);
                None
            }
        }
    }

    fn lower_generic_decl(&mut self, g: Option<&ast::luau::GenericDeclaration>) -> Vec<TypeParam> {
        let Some(g) = g else { return Vec::new() };
        let mut out: Vec<TypeParam> = Vec::new();
        for decl in g.generics().iter() {
            let param = decl.parameter();
            let name_tok = match param {
                ast::luau::GenericParameterInfo::Name(t) => t,
                ast::luau::GenericParameterInfo::Variadic { name, .. } => name,
                other => {
                    self.track_unsupported("ast::luau::GenericParameterInfo", other);
                    continue;
                }
            };
            let name = tok_str(name_tok);
            let name_span = tok_span(name_tok);
            let default = decl.default_type().map(|t| type_annotation_from(t));
            out.push(TypeParam {
                name,
                name_span,
                default,
            });
        }
        out
    }
}

// ----- helper free functions ------------------------------------------------

fn node_span<N: Node>(n: &N) -> Option<Span> {
    let (start, end) = n.range()?;
    Some(span_from_positions(start, end))
}

fn tok_span(tok: &TokenReference) -> Span {
    let token = tok.token();
    span_from_positions(token.start_position(), token.end_position())
}

fn span_from_positions(start: Position, end: Position) -> Span {
    Span {
        start_byte: start.bytes() as u32,
        end_byte: end.bytes() as u32,
        start_line: start.line() as u32,
        start_col: start.character() as u32,
        end_line: end.line() as u32,
        end_col: end.character() as u32,
    }
}

fn tok_str(tok: &TokenReference) -> Bytes {
    Bytes::from(tok.token().to_string().as_bytes())
}

fn string_literal_parts(tok: &TokenReference) -> (Bytes, Bytes, Option<u32>) {
    let token = tok.token();
    match token.token_type() {
        TokenType::StringLiteral {
            literal,
            multi_line_depth,
            quote_type,
        } => {
            let raw = Bytes::from(literal.as_str().as_bytes());
            let is_long = matches!(
                quote_type,
                full_moon::tokenizer::StringLiteralQuoteType::Brackets
            );
            let long_depth = if is_long {
                Some(*multi_line_depth as u32)
            } else {
                None
            };
            // For long-bracket strings escape processing is a no-op
            // (Lua spec); reuse the literal bytes.  For short strings
            // we route through the compiler's escape processor so the
            // value matches what the runtime would actually see.
            let value = if is_long {
                raw.clone()
            } else {
                crate::lower::parse_string_literal(tok).unwrap_or_else(|_| raw.clone())
            };
            (raw, value, long_depth)
        }
        _ => (Bytes::default(), Bytes::default(), None),
    }
}

fn type_annotation_from<N: Node + std::fmt::Display>(n: &N) -> TypeAnnotation {
    TypeAnnotation {
        source: n.to_string(),
        span: node_span(n).unwrap_or(EMPTY_SPAN),
    }
}

/// Does this expression produce multi-ret (function call or `...`)?
/// Used to set [`ExprKind::FunctionCall::has_trailing_multret`].
fn is_multret_kind(e: &Expr) -> bool {
    matches!(
        e.kind,
        ExprKind::FunctionCall(_) | ExprKind::MethodCall(_) | ExprKind::Vararg
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lower `src` and assert against a full Debug-format snapshot
    /// of the resulting [`Chunk`].  Compares the entire tree --
    /// kinds, spans, names, attributes -- so any drift (a new
    /// field, a renamed variant, a span miscalculation) surfaces
    /// as a diff.  Also asserts the lowering produced no
    /// `UnsupportedNode` entries; well-formed input must round-
    /// trip through every recognised arm.
    ///
    /// To refresh snapshots after an intentional change: run
    /// `cargo nextest run -p shingetsu-compiler dump_all
    /// --run-ignored only`, paste the relevant `Chunk { ... }`
    /// block into the expected string.
    #[track_caller]
    fn assert_chunk_snapshot(src: &str, expected: &str) {
        let ast = full_moon::parse(src).expect("parse");
        let l = super::lower(&ast);
        k9::assert_equal!(l.unsupported, Vec::<UnsupportedNode>::new());
        k9::assert_equal!(format!("{:#?}", l.chunk), expected);
    }

    #[test]
    #[ignore = "debug-dump helper; run with --ignored when refreshing snapshots"]
    fn dump_all_for_snapshot_refresh() {
        let sources = &[
            "local x = 1",
            "foo.bar(1, 2)",
            "local x = 1; print(x)",
            "local x <const> = 1",
            "local s = \"hello\\n\"",
            "local s = [==[hi]==]",
            "local x = (1 + 2)",
            "for i = 1, 10 do end",
            "t:m(a, b)",
        ];
        for src in sources {
            let ast = full_moon::parse(src).expect("parse");
            let l = super::lower(&ast);
            eprintln!("=== {src} ===");
            eprintln!("unsupported: {}", l.unsupported.len());
            eprintln!("{:#?}", l.chunk);
            eprintln!();
        }
        panic!("dump-only test: output is in stderr above");
    }

    /// `local x = 1` -- baseline single-binding declaration.
    #[test]
    fn local_assign_lowers_to_local_assign() {
        assert_chunk_snapshot(
            "local x = 1",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: LocalAssign {
                    names: [
                        Param {
                            name: "x",
                            name_span: Span(1:7..1:8 6..7),
                            type_annotation: None,
                            default: None,
                            attribute: None,
                        },
                    ],
                    values: [
                        Expr {
                            kind: NumberLiteral {
                                value: 1.0,
                                raw: "1",
                            },
                            span: Span(1:11..1:12 10..11),
                            was_parenthesized: false,
                        },
                    ],
                },
                span: Span(1:1..1:12 0..11),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:12 0..11),
    },
    span: Span(1:1..1:12 0..11),
}"#,
        );
    }

    /// `foo.bar(1, 2)` -- prefix+suffix chain unfolds to nested
    /// Field / FunctionCall and `foo` resolves as a free global.
    #[test]
    fn function_call_through_field_lowers_correctly() {
        assert_chunk_snapshot(
            "foo.bar(1, 2)",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: ExprStatement {
                    expr: Expr {
                        kind: FunctionCall(
                            FunctionCall {
                                callee: Expr {
                                    kind: Field {
                                        target: Expr {
                                            kind: Name {
                                                name: "foo",
                                                is_global: true,
                                                is_local: false,
                                                binding_id: None,
                                            },
                                            span: Span(1:1..1:4 0..3),
                                            was_parenthesized: false,
                                        },
                                        name: "bar",
                                        name_span: Span(1:5..1:8 4..7),
                                    },
                                    span: Span(1:1..1:8 0..7),
                                    was_parenthesized: false,
                                },
                                args: [
                                    Expr {
                                        kind: NumberLiteral {
                                            value: 1.0,
                                            raw: "1",
                                        },
                                        span: Span(1:9..1:10 8..9),
                                        was_parenthesized: false,
                                    },
                                    Expr {
                                        kind: NumberLiteral {
                                            value: 2.0,
                                            raw: "2",
                                        },
                                        span: Span(1:12..1:13 11..12),
                                        was_parenthesized: false,
                                    },
                                ],
                                has_trailing_multret: false,
                                span: Span(1:1..1:8 0..13),
                                doc_comment: None,
                            },
                        ),
                        span: Span(1:1..1:8 0..13),
                        was_parenthesized: false,
                    },
                },
                span: Span(1:1..1:14 0..13),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:14 0..13),
    },
    span: Span(1:1..1:14 0..13),
}"#,
        );
    }

    /// `local x = 1; print(x)` -- scope tracker classifies `print`
    /// as a free global and the later `x` as a resolved local.
    #[test]
    fn name_resolution_distinguishes_local_and_global() {
        assert_chunk_snapshot(
            "local x = 1; print(x)",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: LocalAssign {
                    names: [
                        Param {
                            name: "x",
                            name_span: Span(1:7..1:8 6..7),
                            type_annotation: None,
                            default: None,
                            attribute: None,
                        },
                    ],
                    values: [
                        Expr {
                            kind: NumberLiteral {
                                value: 1.0,
                                raw: "1",
                            },
                            span: Span(1:11..1:12 10..11),
                            was_parenthesized: false,
                        },
                    ],
                },
                span: Span(1:1..1:12 0..11),
                doc_comment: None,
            },
            Stmt {
                kind: ExprStatement {
                    expr: Expr {
                        kind: FunctionCall(
                            FunctionCall {
                                callee: Expr {
                                    kind: Name {
                                        name: "print",
                                        is_global: true,
                                        is_local: false,
                                        binding_id: None,
                                    },
                                    span: Span(1:14..1:19 13..18),
                                    was_parenthesized: false,
                                },
                                args: [
                                    Expr {
                                        kind: Name {
                                            name: "x",
                                            is_global: false,
                                            is_local: true,
                                            binding_id: Some(
                                                0,
                                            ),
                                        },
                                        span: Span(1:20..1:21 19..20),
                                        was_parenthesized: false,
                                    },
                                ],
                                has_trailing_multret: false,
                                span: Span(1:14..1:19 13..21),
                                doc_comment: None,
                            },
                        ),
                        span: Span(1:14..1:19 13..21),
                        was_parenthesized: false,
                    },
                },
                span: Span(1:14..1:22 13..21),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:22 0..21),
    },
    span: Span(1:1..1:22 0..21),
}"#,
        );
    }

    /// `local x <const> = 1` -- the `<const>` attribute attaches to
    /// the [`Param`] rather than a flat attribs list on the stmt.
    #[test]
    fn const_attribute_attaches_to_param() {
        assert_chunk_snapshot(
            "local x <const> = 1",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: LocalAssign {
                    names: [
                        Param {
                            name: "x",
                            name_span: Span(1:7..1:8 6..7),
                            type_annotation: None,
                            default: None,
                            attribute: Some(
                                Const,
                            ),
                        },
                    ],
                    values: [
                        Expr {
                            kind: NumberLiteral {
                                value: 1.0,
                                raw: "1",
                            },
                            span: Span(1:19..1:20 18..19),
                            was_parenthesized: false,
                        },
                    ],
                },
                span: Span(1:1..1:20 0..19),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:20 0..19),
    },
    span: Span(1:1..1:20 0..19),
}"#,
        );
    }

    /// `"hello\n"` -- `raw` keeps the two-char `\n` sequence;
    /// `value` is post-escape, containing the real LF byte.
    #[test]
    fn string_literal_raw_and_value_diverge_for_escapes() {
        assert_chunk_snapshot(
            "local s = \"hello\\n\"",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: LocalAssign {
                    names: [
                        Param {
                            name: "s",
                            name_span: Span(1:7..1:8 6..7),
                            type_annotation: None,
                            default: None,
                            attribute: None,
                        },
                    ],
                    values: [
                        Expr {
                            kind: StringLiteral {
                                raw: "hello\\n",
                                value: "hello\n",
                                long_depth: None,
                            },
                            span: Span(1:11..1:20 10..19),
                            was_parenthesized: false,
                        },
                    ],
                },
                span: Span(1:1..1:20 0..19),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:20 0..19),
    },
    span: Span(1:1..1:20 0..19),
}"#,
        );
    }

    /// `[==[hi]==]` -- long-bracket string, depth 2; no escape
    /// processing, so `raw == value`.
    #[test]
    fn long_bracket_string_records_depth() {
        assert_chunk_snapshot(
            "local s = [==[hi]==]",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: LocalAssign {
                    names: [
                        Param {
                            name: "s",
                            name_span: Span(1:7..1:8 6..7),
                            type_annotation: None,
                            default: None,
                            attribute: None,
                        },
                    ],
                    values: [
                        Expr {
                            kind: StringLiteral {
                                raw: "hi",
                                value: "hi",
                                long_depth: Some(
                                    2,
                                ),
                            },
                            span: Span(1:11..1:21 10..20),
                            was_parenthesized: false,
                        },
                    ],
                },
                span: Span(1:1..1:21 0..20),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:21 0..20),
    },
    span: Span(1:1..1:21 0..20),
}"#,
        );
    }

    /// `(1 + 2)` -- inner BinOp carries `was_parenthesized = true`
    /// and its span covers the parens.
    #[test]
    fn parentheses_set_was_parenthesized_flag() {
        assert_chunk_snapshot(
            "local x = (1 + 2)",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: LocalAssign {
                    names: [
                        Param {
                            name: "x",
                            name_span: Span(1:7..1:8 6..7),
                            type_annotation: None,
                            default: None,
                            attribute: None,
                        },
                    ],
                    values: [
                        Expr {
                            kind: BinOp {
                                op: Add,
                                op_span: Span(1:14..1:15 13..14),
                                lhs: Expr {
                                    kind: NumberLiteral {
                                        value: 1.0,
                                        raw: "1",
                                    },
                                    span: Span(1:12..1:13 11..12),
                                    was_parenthesized: false,
                                },
                                rhs: Expr {
                                    kind: NumberLiteral {
                                        value: 2.0,
                                        raw: "2",
                                    },
                                    span: Span(1:16..1:17 15..16),
                                    was_parenthesized: false,
                                },
                            },
                            span: Span(1:11..1:18 10..17),
                            was_parenthesized: true,
                        },
                    ],
                },
                span: Span(1:1..1:18 0..17),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:18 0..17),
    },
    span: Span(1:1..1:18 0..17),
}"#,
        );
    }

    /// `for i = 1, 10 do end` -- declares `i` inside the loop
    /// scope.  full_moon reports no range for the empty body
    /// block, so the IR records it as the all-zero span.
    #[test]
    fn numeric_for_declares_loop_variable() {
        assert_chunk_snapshot(
            "for i = 1, 10 do end",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: NumericFor {
                    var: Param {
                        name: "i",
                        name_span: Span(1:5..1:6 4..5),
                        type_annotation: None,
                        default: None,
                        attribute: None,
                    },
                    start: Expr {
                        kind: NumberLiteral {
                            value: 1.0,
                            raw: "1",
                        },
                        span: Span(1:9..1:10 8..9),
                        was_parenthesized: false,
                    },
                    stop: Expr {
                        kind: NumberLiteral {
                            value: 10.0,
                            raw: "10",
                        },
                        span: Span(1:12..1:14 11..13),
                        was_parenthesized: false,
                    },
                    step: None,
                    block: Block {
                        stmts: [],
                        span: Span(0:0..0:0 0..0),
                    },
                },
                span: Span(1:1..1:21 0..20),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:21 0..20),
    },
    span: Span(1:1..1:21 0..20),
}"#,
        );
    }

    /// `t:m(a, b)` -- method call.  Receiver and method name are
    /// distinct fields; the args list excludes the implicit self.
    #[test]
    fn method_call_is_distinct_from_function_call() {
        assert_chunk_snapshot(
            "t:m(a, b)",
            r#"Chunk {
    block: Block {
        stmts: [
            Stmt {
                kind: ExprStatement {
                    expr: Expr {
                        kind: MethodCall(
                            MethodCall {
                                receiver: Expr {
                                    kind: Name {
                                        name: "t",
                                        is_global: true,
                                        is_local: false,
                                        binding_id: None,
                                    },
                                    span: Span(1:1..1:2 0..1),
                                    was_parenthesized: false,
                                },
                                method: "m",
                                method_span: Span(1:3..1:4 2..3),
                                args: [
                                    Expr {
                                        kind: Name {
                                            name: "a",
                                            is_global: true,
                                            is_local: false,
                                            binding_id: None,
                                        },
                                        span: Span(1:5..1:6 4..5),
                                        was_parenthesized: false,
                                    },
                                    Expr {
                                        kind: Name {
                                            name: "b",
                                            is_global: true,
                                            is_local: false,
                                            binding_id: None,
                                        },
                                        span: Span(1:8..1:9 7..8),
                                        was_parenthesized: false,
                                    },
                                ],
                                has_trailing_multret: false,
                                span: Span(1:1..1:4 0..9),
                                doc_comment: None,
                            },
                        ),
                        span: Span(1:1..1:4 0..9),
                        was_parenthesized: false,
                    },
                },
                span: Span(1:1..1:10 0..9),
                doc_comment: None,
            },
        ],
        span: Span(1:1..1:10 0..9),
    },
    span: Span(1:1..1:10 0..9),
}"#,
        );
    }
}
