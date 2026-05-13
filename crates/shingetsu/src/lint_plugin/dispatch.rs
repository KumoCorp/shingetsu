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

use super::node::{DispatchSession, LintContext};
use super::{registry, ASSIGN_EVENT, FUNCTION_CALL_EVENT, METHOD_CALL_EVENT};
use crate::sync::Mutex;
use crate::{GlobalEnv, Ud, Value, VmError};
use shingetsu_compiler::lint_ir::{self, Block, Expr, ExprKind, Span, Stmt, StmtKind};
use shingetsu_compiler::{Diagnostic, LintId, Severity};
use std::sync::Arc;

/// Convert a callback failure to a `Warning`-severity diagnostic
/// anchored at the visited node's span, so a buggy plugin can't
/// halt the rest of the dispatch.
fn report_handler_error(session: &DispatchSession, span: Span, err: VmError) {
    let location = span.to_source_location(&session.source_name);
    let display = match &err {
        VmError::LuaError { display, .. } => display.clone(),
        other => other.to_string(),
    };
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
pub async fn dispatch_chunk(
    env: &GlobalEnv,
    source_name: Arc<String>,
    chunk: &lint_ir::Chunk,
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
    });

    walk_block(env, &session, &chunk.block, None).await?;

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
    match &stmt.kind {
        StmtKind::Assign(a) => {
            let ctx = Ud(Arc::new(LintContext {
                session: Arc::clone(session),
            }));
            let mut payload = a.clone();
            payload.doc_comment = stmt.doc_comment.clone();
            if let Err(e) = ASSIGN_EVENT.call(env, (Ud(Arc::new(payload)), ctx)).await {
                report_handler_error(session, a.span, e);
            }
            for t in &a.targets {
                Box::pin(walk_expr(env, session, t, stmt_doc)).await?;
            }
            for v in &a.values {
                Box::pin(walk_expr(env, session, v, stmt_doc)).await?;
            }
        }
        StmtKind::LocalAssign { values, .. } => {
            for v in values {
                Box::pin(walk_expr(env, session, v, stmt_doc)).await?;
            }
        }
        StmtKind::CompoundAssign { target, value, .. } => {
            Box::pin(walk_expr(env, session, target, stmt_doc)).await?;
            Box::pin(walk_expr(env, session, value, stmt_doc)).await?;
        }
        StmtKind::ConstAssign { value, .. } => {
            Box::pin(walk_expr(env, session, value, stmt_doc)).await?;
        }
        StmtKind::ExprStatement { expr } => {
            Box::pin(walk_expr(env, session, expr, stmt_doc)).await?;
        }
        StmtKind::If {
            branches,
            else_block,
        } => {
            for b in branches {
                Box::pin(walk_expr(env, session, &b.cond, stmt_doc)).await?;
                Box::pin(walk_block(env, session, &b.block, stmt_doc)).await?;
            }
            if let Some(b) = else_block {
                Box::pin(walk_block(env, session, b, stmt_doc)).await?;
            }
        }
        StmtKind::While { cond, block } => {
            Box::pin(walk_expr(env, session, cond, stmt_doc)).await?;
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
        }
        StmtKind::Repeat { block, cond } => {
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
            Box::pin(walk_expr(env, session, cond, stmt_doc)).await?;
        }
        StmtKind::NumericFor {
            start,
            stop,
            step,
            block,
            ..
        } => {
            Box::pin(walk_expr(env, session, start, stmt_doc)).await?;
            Box::pin(walk_expr(env, session, stop, stmt_doc)).await?;
            if let Some(s) = step {
                Box::pin(walk_expr(env, session, s, stmt_doc)).await?;
            }
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
        }
        StmtKind::GenericFor { exprs, block, .. } => {
            for e in exprs {
                Box::pin(walk_expr(env, session, e, stmt_doc)).await?;
            }
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
        }
        StmtKind::DoBlock { block } => {
            Box::pin(walk_block(env, session, block, stmt_doc)).await?;
        }
        StmtKind::Return { values } => {
            for v in values {
                Box::pin(walk_expr(env, session, v, stmt_doc)).await?;
            }
        }
        StmtKind::LocalFunction { body, .. }
        | StmtKind::FunctionDecl { body, .. }
        | StmtKind::ConstFunction { body, .. } => {
            Box::pin(walk_block(env, session, body, stmt_doc)).await?;
        }
        // Statements with no child expressions / blocks: nothing
        // to recurse into.  Their event firing (when wired) would
        // still happen on the way down -- not in this MVP.
        StmtKind::Break
        | StmtKind::Continue
        | StmtKind::Goto { .. }
        | StmtKind::Label { .. }
        | StmtKind::GlobalDecl { .. }
        | StmtKind::TypeAlias { .. } => {}
    }
    Ok(())
}

async fn walk_expr(
    env: &GlobalEnv,
    session: &Arc<DispatchSession>,
    expr: &Expr,
    enclosing_doc: Option<&str>,
) -> Result<(), VmError> {
    // Fire the event *before* recursing so a future `walker:skip`
    // can stop descent.
    let ctx = || {
        Ud(Arc::new(LintContext {
            session: Arc::clone(session),
        }))
    };
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
            Box::pin(walk_expr(env, session, &mc.receiver, enclosing_doc)).await?;
            for a in &mc.args {
                Box::pin(walk_expr(env, session, a, enclosing_doc)).await?;
            }
            return Ok(());
        }
        ExprKind::FunctionCall(fc) => {
            let mut payload = fc.clone();
            payload.doc_comment = enclosing_doc.map(String::from);
            if let Err(e) = FUNCTION_CALL_EVENT
                .call(env, (Ud(Arc::new(payload)), ctx()))
                .await
            {
                report_handler_error(session, fc.span, e);
            }
            Box::pin(walk_expr(env, session, &fc.callee, enclosing_doc)).await?;
            for a in &fc.args {
                Box::pin(walk_expr(env, session, a, enclosing_doc)).await?;
            }
            return Ok(());
        }
        _ => {}
    }

    match &expr.kind {
        ExprKind::BinOp { lhs, rhs, .. } => {
            Box::pin(walk_expr(env, session, lhs, enclosing_doc)).await?;
            Box::pin(walk_expr(env, session, rhs, enclosing_doc)).await?;
        }
        ExprKind::UnOp { operand, .. } => {
            Box::pin(walk_expr(env, session, operand, enclosing_doc)).await?;
        }
        ExprKind::Index { target, key } => {
            Box::pin(walk_expr(env, session, target, enclosing_doc)).await?;
            Box::pin(walk_expr(env, session, key, enclosing_doc)).await?;
        }
        ExprKind::Field { target, .. } => {
            Box::pin(walk_expr(env, session, target, enclosing_doc)).await?;
        }
        ExprKind::TableConstructor { entries } => {
            for e in entries {
                match &e.kind {
                    lint_ir::TableEntryKind::Array { value } => {
                        Box::pin(walk_expr(env, session, value, enclosing_doc)).await?;
                    }
                    lint_ir::TableEntryKind::Named { value, .. } => {
                        Box::pin(walk_expr(env, session, value, enclosing_doc)).await?;
                    }
                    lint_ir::TableEntryKind::Hash { key, value } => {
                        Box::pin(walk_expr(env, session, key, enclosing_doc)).await?;
                        Box::pin(walk_expr(env, session, value, enclosing_doc)).await?;
                    }
                }
            }
        }
        ExprKind::FunctionExpr { body, .. } => {
            Box::pin(walk_block(env, session, body, enclosing_doc)).await?;
        }
        ExprKind::IfExpression {
            branches,
            else_expr,
        } => {
            for b in branches {
                Box::pin(walk_expr(env, session, &b.cond, enclosing_doc)).await?;
                Box::pin(walk_expr(env, session, &b.value, enclosing_doc)).await?;
            }
            Box::pin(walk_expr(env, session, else_expr, enclosing_doc)).await?;
        }
        ExprKind::InterpString { parts } => {
            for p in parts {
                if let lint_ir::InterpPart::Expr(e) = p {
                    Box::pin(walk_expr(env, session, e, enclosing_doc)).await?;
                }
            }
        }
        ExprKind::TypeAssertion { expr, .. } => {
            Box::pin(walk_expr(env, session, expr, enclosing_doc)).await?;
        }
        // Terminal expression kinds: nothing to recurse into.
        // FunctionCall / MethodCall already handled above with
        // an early return after firing their event.
        ExprKind::StringLiteral { .. }
        | ExprKind::NumberLiteral { .. }
        | ExprKind::BoolLiteral(_)
        | ExprKind::Nil
        | ExprKind::Vararg
        | ExprKind::Name { .. }
        | ExprKind::FunctionCall(_)
        | ExprKind::MethodCall(_) => {}
    }
    Ok(())
}
