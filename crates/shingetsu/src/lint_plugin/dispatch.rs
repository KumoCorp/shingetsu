//! Walk a [`lint_ir::Chunk`] and fire visitor events against a
//! plugin's `GlobalEnv`.
//!
//! Traversal is pre-order: each node fires its event *before*
//! recursing into children.  This matches the expectations of
//! lint frameworks that surface a `walker:skip()` later (a parent
//! handler should see its node before any child handler has a
//! chance to inspect it).
//!
//! Only the `method_call` event is wired in this MVP cut.
//! `function_call` and `assign` follow once the corresponding
//! event-payload userdata land.  Until they're wired the walker
//! still descends into call/assign nodes -- only the firing is
//! skipped.

use super::node::{AncestorKind, DispatchSession, LintContext};
use super::{
    registry, ASSIGN_EVENT, BINOP_EVENT, BREAK_EVENT, CHUNK_BEGIN_EVENT, CHUNK_END_EVENT,
    CONTINUE_EVENT, DO_BLOCK_EVENT, EXPR_STATEMENT_EVENT, FUNCTION_CALL_EVENT, FUNCTION_DECL_EVENT,
    FUNCTION_EXPR_EVENT, GENERIC_FOR_EVENT, GLOBAL_DECL_EVENT, GLOBAL_READ_EVENT,
    GLOBAL_WRITE_EVENT, GOTO_EVENT, IF_EVENT, INTERP_STRING_EVENT, LABEL_EVENT, LOCAL_ASSIGN_EVENT,
    LOCAL_FUNCTION_EVENT, METHOD_CALL_EVENT, NAME_EVENT, NUMBER_LITERAL_EVENT, NUMERIC_FOR_EVENT,
    REPEAT_EVENT, REQUIRE_EVENT, RETURN_EVENT, STATEMENT_EVENT, STRING_LITERAL_EVENT,
    TABLE_CONSTRUCTOR_EVENT, UNOP_EVENT, WHILE_EVENT,
};
use crate::error::RuntimeError;
use crate::sync::Mutex;
use crate::{GlobalEnv, Ud, Value, VmError};
use shingetsu_compiler::lint_ir::{self, Block, Expr, ExprKind, Span, Stmt, StmtKind};
use shingetsu_compiler::{Diagnostic, LintId, Severity};
use std::sync::Arc;

/// Convert a `toml::Value` to a Lua `Value` by round-tripping through
/// `serde_json::Value` and using the existing JSON bridge.
fn toml_to_lua(val: &toml::Value) -> Option<crate::Value> {
    let json = serde_json::to_value(val).ok()?;
    shingetsu_vm::serde_bridge::value_from_json(json).ok()
}

/// Convert a callback failure to a `Warning`-severity diagnostic
/// anchored at the visited node's span, so a buggy plugin can't
/// halt the rest of the dispatch.
fn report_handler_error(session: &DispatchSession, span: Span, err: RuntimeError) {
    let location = span.to_source_location(&session.source_name);
    // Annotate the linted node's span with the handler's raised
    // message.  The node span is the useful anchor for the plugin's
    // user, so the plugin's own traceback is flattened to a single line.
    let display = err.to_string();
    let _ = Value::Nil; // keep the Value import wired alongside other re-exports
    session.diagnostics.lock().push(Diagnostic {
        lint: LintId::Plugin(Arc::clone(&session.plugin_name)),
        severity: Severity::Warning,
        location,
        message: format!(
            "lint plugin '{}' handler raised: {display}",
            session.plugin_name,
        ),
        help: None,
        primary_label: None,
        secondary_spans: vec![],
    });
}

/// Walk `chunk` and fire visitor events against every callback
/// registered on `env`.  Returns every diagnostic any `ctx:warn`
/// / `ctx:error` call produced.
///
/// `source_name` is what diagnostic anchors render the chunk under
/// -- the same value the compiler used as
/// [`shingetsu_compiler::CompileOptions::source_name`] when it
/// built the chunk's bytecode.
///
/// `plugin_config` is the optional per-plugin TOML block from
/// `[check.plugin_configs.<name>]`.  Converted to a Lua table and
/// exposed as `ctx.config`; `nil` when absent or conversion fails.
pub async fn dispatch_chunk(
    env: &GlobalEnv,
    source_name: Arc<String>,
    chunk: &lint_ir::Chunk,
    plugin_config: Option<&toml::Value>,
) -> Result<Vec<Diagnostic>, VmError> {
    // The plugin loaded into this env has its declaration recorded
    // on the env's plugin registry.  Without a declaration the env
    // doesn't have a plugin to attribute diagnostics to; that's a
    // programming error in the orchestrator, not a runtime
    // condition the user can fix.
    let reg = registry(env);
    let decls = reg.declarations();
    let Some(decl) = decls.first() else {
        return Err(VmError::LuaError {
            display: "dispatch_chunk called against an env with no loaded plugin".to_string(),
            value: crate::Value::string(
                "dispatch_chunk called against an env with no loaded plugin",
            ),
        });
    };

    let session = Arc::new(DispatchSession {
        plugin_name: Arc::<str>::from(decl.name.as_str()),
        default_severity: decl.default_severity,
        source_name,
        diagnostics: Mutex::new(Vec::new()),
        ancestors: Mutex::new(Vec::new()),
        config: plugin_config.and_then(toml_to_lua),
    });

    let ctx = || {
        Ud(Arc::new(LintContext {
            session: Arc::clone(&session),
        }))
    };
    session.push_ancestor(AncestorKind::Chunk, chunk.span);
    if let Err(e) = CHUNK_BEGIN_EVENT.call(env, (ctx(),)).await {
        report_handler_error(&session, chunk.span, e);
    }
    walk_block(env, &session, &chunk.block, None).await?;
    if let Err(e) = CHUNK_END_EVENT.call(env, (ctx(),)).await {
        report_handler_error(&session, chunk.span, e);
    }
    session.pop_ancestor();

    let diags = std::mem::take(&mut *session.diagnostics.lock());
    let _ = Severity::Warning; // keep the import wired for future ctx.error use
    Ok(diags)
}

async fn walk_block(
    env: &GlobalEnv,
    session: &Arc<DispatchSession>,
    block: &Block,
    enclosing_doc: Option<&str>,
) -> Result<(), VmError> {
    for stmt in &block.stmts {
        Box::pin(walk_stmt(env, session, stmt, enclosing_doc)).await?;
    }
    Ok(())
}

async fn walk_stmt(
    env: &GlobalEnv,
    session: &Arc<DispatchSession>,
    stmt: &Stmt,
    enclosing_doc: Option<&str>,
) -> Result<(), VmError> {
    // The doc-comment visible to events fired inside this stmt is
    // the stmt's own `---` block when present, otherwise inherited
    // from the enclosing context.
    let stmt_doc: Option<&str> = stmt.doc_comment.as_deref().or(enclosing_doc);

    let ctx = || {
        Ud(Arc::new(LintContext {
            session: Arc::clone(session),
        }))
    };

    // `statement` fires before every kind-specific event.
    let stmt_ud = || Ud(Arc::new(stmt.clone()));
    if let Err(e) = STATEMENT_EVENT.call(env, (stmt_ud(), ctx())).await {
        report_handler_error(session, stmt.span, e);
    }

    match &stmt.kind {
        StmtKind::Assign(a) => {
            let mut payload = a.clone();
            payload.doc_comment = stmt.doc_comment.clone();
            if let Err(e) = ASSIGN_EVENT.call(env, (Ud(Arc::new(payload)), ctx())).await {
                report_handler_error(session, a.span, e);
            }
            // global_write for global-name targets.
            for t in &a.targets {
                if let ExprKind::Name {
                    is_global: true, ..
                } = &t.kind
                {
                    let expr_ud = Ud(Arc::new(t.clone()));
                    if let Err(e) = GLOBAL_WRITE_EVENT.call(env, (expr_ud.clone(), ctx())).await {
                        report_handler_error(session, t.span, e);
                    }
                    if let Err(e) = NAME_EVENT.call(env, (expr_ud, ctx())).await {
                        report_handler_error(session, t.span, e);
                    }
                } else {
                    Box::pin(walk_expr(env, session, t, stmt_doc, false)).await?;
                }
            }
            for v in &a.values {
                Box::pin(walk_expr(env, session, v, stmt_doc, false)).await?;
            }
        }
        StmtKind::LocalAssign { values, .. } => {
            if let Err(e) = LOCAL_ASSIGN_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            for v in values {
                Box::pin(walk_expr(env, session, v, stmt_doc, false)).await?;
            }
        }
        StmtKind::CompoundAssign { target, value, .. } => {
            // compound_assign is a specialised assign; no separate event
            // in Phase 6, but we fire assign-like traversal.
            if let ExprKind::Name {
                is_global: true, ..
            } = &target.kind
            {
                let expr_ud = Ud(Arc::new(target.clone()));
                if let Err(e) = GLOBAL_WRITE_EVENT.call(env, (expr_ud.clone(), ctx())).await {
                    report_handler_error(session, target.span, e);
                }
                if let Err(e) = NAME_EVENT.call(env, (expr_ud, ctx())).await {
                    report_handler_error(session, target.span, e);
                }
            } else {
                Box::pin(walk_expr(env, session, target, stmt_doc, false)).await?;
            }
            Box::pin(walk_expr(env, session, value, stmt_doc, false)).await?;
        }
        StmtKind::ConstAssign { value, .. } => {
            Box::pin(walk_expr(env, session, value, stmt_doc, false)).await?;
        }
        StmtKind::ExprStatement { expr } => {
            if let Err(e) = EXPR_STATEMENT_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            Box::pin(walk_expr(env, session, expr, stmt_doc, false)).await?;
        }
        StmtKind::If {
            branches,
            else_block,
        } => {
            if let Err(e) = IF_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            for b in branches {
                Box::pin(walk_expr(env, session, &b.cond, stmt_doc, false)).await?;
                session.push_ancestor(AncestorKind::Branch, b.cond.span);
                Box::pin(walk_block(env, session, &b.block, stmt_doc)).await?;
                session.pop_ancestor();
            }
            if let Some(b) = else_block {
                session.push_ancestor(AncestorKind::Branch, stmt.span);
                Box::pin(walk_block(env, session, b, stmt_doc)).await?;
                session.pop_ancestor();
            }
        }
        StmtKind::While { cond, block } => {
            if let Err(e) = WHILE_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            Box::pin(walk_expr(env, session, cond, stmt_doc, false)).await?;
            session.push_ancestor(AncestorKind::Loop, stmt.span);
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
            session.pop_ancestor();
        }
        StmtKind::Repeat { block, cond } => {
            if let Err(e) = REPEAT_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            session.push_ancestor(AncestorKind::Loop, stmt.span);
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
            session.pop_ancestor();
            Box::pin(walk_expr(env, session, cond, stmt_doc, false)).await?;
        }
        StmtKind::NumericFor {
            start,
            stop,
            step,
            block,
            ..
        } => {
            if let Err(e) = NUMERIC_FOR_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            Box::pin(walk_expr(env, session, start, stmt_doc, false)).await?;
            Box::pin(walk_expr(env, session, stop, stmt_doc, false)).await?;
            if let Some(s) = step {
                Box::pin(walk_expr(env, session, s, stmt_doc, false)).await?;
            }
            session.push_ancestor(AncestorKind::Loop, stmt.span);
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
            session.pop_ancestor();
        }
        StmtKind::GenericFor { exprs, block, .. } => {
            if let Err(e) = GENERIC_FOR_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            for e in exprs {
                Box::pin(walk_expr(env, session, e, stmt_doc, false)).await?;
            }
            session.push_ancestor(AncestorKind::Loop, stmt.span);
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
            session.pop_ancestor();
        }
        StmtKind::DoBlock { block } => {
            if let Err(e) = DO_BLOCK_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            session.push_ancestor(AncestorKind::DoBlock, stmt.span);
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
            session.pop_ancestor();
        }
        StmtKind::Return { values } => {
            if let Err(e) = RETURN_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            for v in values {
                Box::pin(walk_expr(env, session, v, stmt_doc, false)).await?;
            }
        }
        StmtKind::LocalFunction { body, .. } | StmtKind::ConstFunction { body, .. } => {
            if let Err(e) = LOCAL_FUNCTION_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            session.push_ancestor(AncestorKind::Function, stmt.span);
            Box::pin(walk_block(env, session, body, stmt_doc)).await?;
            session.pop_ancestor();
        }
        StmtKind::FunctionDecl { body, .. } => {
            if let Err(e) = FUNCTION_DECL_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
            session.push_ancestor(AncestorKind::Function, stmt.span);
            Box::pin(walk_block(env, session, body, stmt_doc)).await?;
            session.pop_ancestor();
        }
        StmtKind::GlobalDecl { .. } => {
            if let Err(e) = GLOBAL_DECL_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
        }
        StmtKind::Break => {
            if let Err(e) = BREAK_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
        }
        StmtKind::Continue => {
            if let Err(e) = CONTINUE_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
        }
        StmtKind::Goto { .. } => {
            if let Err(e) = GOTO_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
        }
        StmtKind::Label { .. } => {
            if let Err(e) = LABEL_EVENT.call(env, (stmt_ud(), ctx())).await {
                report_handler_error(session, stmt.span, e);
            }
        }
        StmtKind::TypeAlias { .. } => {}
    }
    Ok(())
}

async fn walk_expr(
    env: &GlobalEnv,
    session: &Arc<DispatchSession>,
    expr: &Expr,
    enclosing_doc: Option<&str>,
    // Used by assignment-target walks to suppress global_read and
    // fire global_write instead.  Callers outside assignment targets
    // always pass `false`.
    _is_lvalue: bool,
) -> Result<(), VmError> {
    // Fire the event *before* recursing so a future `walker:skip`
    // can stop descent.
    let ctx = || {
        Ud(Arc::new(LintContext {
            session: Arc::clone(session),
        }))
    };
    let expr_ud = || Ud(Arc::new(expr.clone()));

    match &expr.kind {
        ExprKind::MethodCall(mc) => {
            let mut payload = mc.clone();
            payload.doc_comment = enclosing_doc.map(String::from);
            if let Err(e) = METHOD_CALL_EVENT
                .call(env, (Ud(Arc::new(payload)), ctx()))
                .await
            {
                report_handler_error(session, mc.span, e);
            }
            Box::pin(walk_expr(env, session, &mc.receiver, enclosing_doc, false)).await?;
            for a in &mc.args {
                Box::pin(walk_expr(env, session, a, enclosing_doc, false)).await?;
            }
            return Ok(());
        }
        ExprKind::FunctionCall(fc) => {
            let is_require = matches!(
                fc.callee.kind,
                ExprKind::Name { ref name, is_global: true, .. }
                if name.as_ref() == b"require"
            );
            let mut payload = fc.clone();
            payload.doc_comment = enclosing_doc.map(String::from);
            let payload_ud = Ud(Arc::new(payload));
            if let Err(e) = FUNCTION_CALL_EVENT
                .call(env, (payload_ud.clone(), ctx()))
                .await
            {
                report_handler_error(session, fc.span, e);
            }
            if is_require {
                if let Err(e) = REQUIRE_EVENT.call(env, (payload_ud, ctx())).await {
                    report_handler_error(session, fc.span, e);
                }
            }
            Box::pin(walk_expr(env, session, &fc.callee, enclosing_doc, false)).await?;
            for a in &fc.args {
                Box::pin(walk_expr(env, session, a, enclosing_doc, false)).await?;
            }
            return Ok(());
        }
        ExprKind::Name { is_global, .. } => {
            if let Err(e) = NAME_EVENT.call(env, (expr_ud(), ctx())).await {
                report_handler_error(session, expr.span, e);
            }
            if *is_global {
                if let Err(e) = GLOBAL_READ_EVENT.call(env, (expr_ud(), ctx())).await {
                    report_handler_error(session, expr.span, e);
                }
            }
            return Ok(());
        }
        ExprKind::StringLiteral { .. } => {
            if let Err(e) = STRING_LITERAL_EVENT.call(env, (expr_ud(), ctx())).await {
                report_handler_error(session, expr.span, e);
            }
            return Ok(());
        }
        ExprKind::NumberLiteral { .. } => {
            if let Err(e) = NUMBER_LITERAL_EVENT.call(env, (expr_ud(), ctx())).await {
                report_handler_error(session, expr.span, e);
            }
            return Ok(());
        }
        ExprKind::InterpString { parts } => {
            if let Err(e) = INTERP_STRING_EVENT.call(env, (expr_ud(), ctx())).await {
                report_handler_error(session, expr.span, e);
            }
            for p in parts {
                if let lint_ir::InterpPart::Expr(e) = p {
                    Box::pin(walk_expr(env, session, e, enclosing_doc, false)).await?;
                }
            }
            return Ok(());
        }
        ExprKind::BinOp { lhs, rhs, .. } => {
            if let Err(e) = BINOP_EVENT.call(env, (expr_ud(), ctx())).await {
                report_handler_error(session, expr.span, e);
            }
            Box::pin(walk_expr(env, session, lhs, enclosing_doc, false)).await?;
            Box::pin(walk_expr(env, session, rhs, enclosing_doc, false)).await?;
            return Ok(());
        }
        ExprKind::UnOp { operand, .. } => {
            if let Err(e) = UNOP_EVENT.call(env, (expr_ud(), ctx())).await {
                report_handler_error(session, expr.span, e);
            }
            Box::pin(walk_expr(env, session, operand, enclosing_doc, false)).await?;
            return Ok(());
        }
        ExprKind::TableConstructor { entries } => {
            if let Err(e) = TABLE_CONSTRUCTOR_EVENT.call(env, (expr_ud(), ctx())).await {
                report_handler_error(session, expr.span, e);
            }
            for e in entries {
                match &e.kind {
                    lint_ir::TableEntryKind::Array { value } => {
                        Box::pin(walk_expr(env, session, value, enclosing_doc, false)).await?;
                    }
                    lint_ir::TableEntryKind::Named { value, .. } => {
                        Box::pin(walk_expr(env, session, value, enclosing_doc, false)).await?;
                    }
                    lint_ir::TableEntryKind::Hash { key, value } => {
                        Box::pin(walk_expr(env, session, key, enclosing_doc, false)).await?;
                        Box::pin(walk_expr(env, session, value, enclosing_doc, false)).await?;
                    }
                }
            }
            return Ok(());
        }
        ExprKind::FunctionExpr { body, .. } => {
            if let Err(e) = FUNCTION_EXPR_EVENT.call(env, (expr_ud(), ctx())).await {
                report_handler_error(session, expr.span, e);
            }
            session.push_ancestor(AncestorKind::Function, expr.span);
            Box::pin(walk_block(env, session, body, enclosing_doc)).await?;
            session.pop_ancestor();
            return Ok(());
        }
        _ => {}
    }

    // Remaining kinds with child expressions but no dedicated event.
    match &expr.kind {
        ExprKind::Index { target, key } => {
            Box::pin(walk_expr(env, session, target, enclosing_doc, false)).await?;
            Box::pin(walk_expr(env, session, key, enclosing_doc, false)).await?;
        }
        ExprKind::Field { target, .. } => {
            Box::pin(walk_expr(env, session, target, enclosing_doc, false)).await?;
        }
        ExprKind::IfExpression {
            branches,
            else_expr,
        } => {
            for b in branches {
                Box::pin(walk_expr(env, session, &b.cond, enclosing_doc, false)).await?;
                Box::pin(walk_expr(env, session, &b.value, enclosing_doc, false)).await?;
            }
            Box::pin(walk_expr(env, session, else_expr, enclosing_doc, false)).await?;
        }
        ExprKind::TypeAssertion { expr, .. } => {
            Box::pin(walk_expr(env, session, expr, enclosing_doc, false)).await?;
        }
        // Terminal kinds with no children and no dedicated event.
        ExprKind::BoolLiteral(_) | ExprKind::Nil | ExprKind::Vararg => {}
        // Already handled above with early returns.
        ExprKind::FunctionCall(_)
        | ExprKind::MethodCall(_)
        | ExprKind::Name { .. }
        | ExprKind::StringLiteral { .. }
        | ExprKind::NumberLiteral { .. }
        | ExprKind::InterpString { .. }
        | ExprKind::BinOp { .. }
        | ExprKind::UnOp { .. }
        | ExprKind::TableConstructor { .. }
        | ExprKind::FunctionExpr { .. } => unreachable!(),
    }
    Ok(())
}
