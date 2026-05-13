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
use super::{registry, FUNCTION_CALL_EVENT, METHOD_CALL_EVENT};
use crate::sync::Mutex;
use crate::{GlobalEnv, Ud, VmError};
use shingetsu_compiler::lint_ir::{self, Block, Expr, ExprKind, Stmt, StmtKind};
use shingetsu_compiler::{Diagnostic, Severity};
use std::sync::Arc;

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

    walk_block(env, &session, &chunk.block).await?;

    let diags = std::mem::take(&mut *session.diagnostics.lock());
    let _ = Severity::Warning; // keep the import wired for future ctx.error use
    Ok(diags)
}

async fn walk_block(
    env: &GlobalEnv,
    session: &Arc<DispatchSession>,
    block: &Block,
) -> Result<(), VmError> {
    for stmt in &block.stmts {
        Box::pin(walk_stmt(env, session, stmt)).await?;
    }
    Ok(())
}

async fn walk_stmt(
    env: &GlobalEnv,
    session: &Arc<DispatchSession>,
    stmt: &Stmt,
) -> Result<(), VmError> {
    match &stmt.kind {
        StmtKind::Assign { targets, values } => {
            for t in targets {
                Box::pin(walk_expr(env, session, t)).await?;
            }
            for v in values {
                Box::pin(walk_expr(env, session, v)).await?;
            }
        }
        StmtKind::LocalAssign { values, .. } => {
            for v in values {
                Box::pin(walk_expr(env, session, v)).await?;
            }
        }
        StmtKind::CompoundAssign { target, value, .. } => {
            Box::pin(walk_expr(env, session, target)).await?;
            Box::pin(walk_expr(env, session, value)).await?;
        }
        StmtKind::ConstAssign { value, .. } => {
            Box::pin(walk_expr(env, session, value)).await?;
        }
        StmtKind::ExprStatement { expr } => {
            Box::pin(walk_expr(env, session, expr)).await?;
        }
        StmtKind::If {
            branches,
            else_block,
        } => {
            for b in branches {
                Box::pin(walk_expr(env, session, &b.cond)).await?;
                Box::pin(walk_block(env, session, &b.block)).await?;
            }
            if let Some(b) = else_block {
                Box::pin(walk_block(env, session, b)).await?;
            }
        }
        StmtKind::While { cond, block } => {
            Box::pin(walk_expr(env, session, cond)).await?;
            Box::pin(walk_block(env, session, block)).await?;
        }
        StmtKind::Repeat { block, cond } => {
            Box::pin(walk_block(env, session, block)).await?;
            Box::pin(walk_expr(env, session, cond)).await?;
        }
        StmtKind::NumericFor {
            start,
            stop,
            step,
            block,
            ..
        } => {
            Box::pin(walk_expr(env, session, start)).await?;
            Box::pin(walk_expr(env, session, stop)).await?;
            if let Some(s) = step {
                Box::pin(walk_expr(env, session, s)).await?;
            }
            Box::pin(walk_block(env, session, block)).await?;
        }
        StmtKind::GenericFor { exprs, block, .. } => {
            for e in exprs {
                Box::pin(walk_expr(env, session, e)).await?;
            }
            Box::pin(walk_block(env, session, block)).await?;
        }
        StmtKind::DoBlock { block } => {
            Box::pin(walk_block(env, session, block)).await?;
        }
        StmtKind::Return { values } => {
            for v in values {
                Box::pin(walk_expr(env, session, v)).await?;
            }
        }
        StmtKind::LocalFunction { body, .. }
        | StmtKind::FunctionDecl { body, .. }
        | StmtKind::ConstFunction { body, .. } => {
            Box::pin(walk_block(env, session, body)).await?;
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
            METHOD_CALL_EVENT
                .call(env, (Ud(Arc::new(mc.clone())), ctx()))
                .await?;
            Box::pin(walk_expr(env, session, &mc.receiver)).await?;
            for a in &mc.args {
                Box::pin(walk_expr(env, session, a)).await?;
            }
            return Ok(());
        }
        ExprKind::FunctionCall(fc) => {
            FUNCTION_CALL_EVENT
                .call(env, (Ud(Arc::new(fc.clone())), ctx()))
                .await?;
            Box::pin(walk_expr(env, session, &fc.callee)).await?;
            for a in &fc.args {
                Box::pin(walk_expr(env, session, a)).await?;
            }
            return Ok(());
        }
        _ => {}
    }

    match &expr.kind {
        ExprKind::BinOp { lhs, rhs, .. } => {
            Box::pin(walk_expr(env, session, lhs)).await?;
            Box::pin(walk_expr(env, session, rhs)).await?;
        }
        ExprKind::UnOp { operand, .. } => {
            Box::pin(walk_expr(env, session, operand)).await?;
        }
        ExprKind::Index { target, key } => {
            Box::pin(walk_expr(env, session, target)).await?;
            Box::pin(walk_expr(env, session, key)).await?;
        }
        ExprKind::Field { target, .. } => {
            Box::pin(walk_expr(env, session, target)).await?;
        }
        ExprKind::TableConstructor { entries } => {
            for e in entries {
                match &e.kind {
                    lint_ir::TableEntryKind::Array { value } => {
                        Box::pin(walk_expr(env, session, value)).await?;
                    }
                    lint_ir::TableEntryKind::Named { value, .. } => {
                        Box::pin(walk_expr(env, session, value)).await?;
                    }
                    lint_ir::TableEntryKind::Hash { key, value } => {
                        Box::pin(walk_expr(env, session, key)).await?;
                        Box::pin(walk_expr(env, session, value)).await?;
                    }
                }
            }
        }
        ExprKind::FunctionExpr { body, .. } => {
            Box::pin(walk_block(env, session, body)).await?;
        }
        ExprKind::IfExpression {
            branches,
            else_expr,
        } => {
            for b in branches {
                Box::pin(walk_expr(env, session, &b.cond)).await?;
                Box::pin(walk_expr(env, session, &b.value)).await?;
            }
            Box::pin(walk_expr(env, session, else_expr)).await?;
        }
        ExprKind::InterpString { parts } => {
            for p in parts {
                if let lint_ir::InterpPart::Expr(e) = p {
                    Box::pin(walk_expr(env, session, e)).await?;
                }
            }
        }
        ExprKind::TypeAssertion { expr, .. } => {
            Box::pin(walk_expr(env, session, expr)).await?;
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
