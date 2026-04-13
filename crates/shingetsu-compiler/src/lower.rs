//! Walks the `full_moon` AST and emits IR for a single `Proto`.
//!
//! Phase 1 supports: numeric literals, arithmetic, comparisons, logical
//! operators, local variables (`<const>` / no attribute), `if`, `while`,
//! `repeat`, numeric `for`, `do...end`, `goto`/`::label::`, function
//! definitions and calls, `return`, and multiple return values.
//!
//! Unsupported constructs (strings in non-trivial positions, tables, closures,
//! upvalues, `<close>`) produce `CompileError::UnsupportedFeature`.

use std::sync::Arc;

use bytes::Bytes;
use full_moon::{
    ast::{self, lua52 as ast52, Ast},
    tokenizer::{Token, TokenReference, TokenType},
};
use shingetsu_vm::{
    ir::Instruction,
    proto::Proto,
    types::{FunctionSignature, LocalAttr, ParamSpec},
};

use crate::{
    codegen::CodeGen,
    error::{CompileError, SourceLocation as CSourceLocation},
    scope::ScopeStack,
    CompileOptions,
};

// ---------------------------------------------------------------------------
// Function compiler state
// ---------------------------------------------------------------------------

struct FnCompiler<'opts> {
    opts: &'opts CompileOptions,
    cg: CodeGen,
    scope: ScopeStack,
    /// Named labels: `(name_bytes, target_pc)`.
    labels: Vec<(Bytes, usize)>,
    /// Pending gotos waiting for a label: `(name_bytes, jump_instr_idx, scope_depth)`.
    pending_gotos: Vec<(Bytes, usize, usize)>,
    /// Nested function bodies compiled separately.
    child_protos: Vec<Arc<Proto>>,
    /// Scratch register used to temporarily hold intermediate values.
    /// We manage a simple top-of-stack register allocator.
    temp_top: u8,
}

impl<'opts> FnCompiler<'opts> {
    fn new(opts: &'opts CompileOptions) -> Self {
        FnCompiler {
            opts,
            cg: CodeGen::new(),
            scope: ScopeStack::new(),
            labels: Vec::new(),
            pending_gotos: Vec::new(),
            child_protos: Vec::new(),
            temp_top: 0,
        }
    }

    fn loc(&self, pos: full_moon::tokenizer::Position) -> CSourceLocation {
        CSourceLocation {
            source_name: self.opts.source_name.clone(),
            line: pos.line() as u32,
            column: pos.character() as u32,
        }
    }

    fn unsupported(
        &self,
        pos: full_moon::tokenizer::Position,
        feature: &'static str,
    ) -> CompileError {
        CompileError::UnsupportedFeature {
            location: self.loc(pos),
            feature,
        }
    }

    /// Allocate a temporary register above the current locals.
    fn alloc_temp(&mut self) -> u8 {
        let slot = self.scope.current_slot() + self.temp_top;
        self.temp_top += 1;
        if slot + 1 > self.scope.max_slot + self.temp_top {
            // max_slot is updated by declare; for temps we track separately.
        }
        slot
    }

    /// Release the topmost temporary register.
    fn free_temp(&mut self) {
        if self.temp_top > 0 {
            self.temp_top -= 1;
        }
    }

    // -----------------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------------

    fn compile_block(&mut self, block: &ast::Block) -> Result<(), CompileError> {
        self.scope.push_scope();
        for stmt in block.stmts() {
            self.compile_stmt(stmt)?;
        }
        if let Some(last) = block.last_stmt() {
            self.compile_last_stmt(last)?;
        }
        self.scope.pop_scope();
        Ok(())
    }

    fn compile_stmt(&mut self, stmt: &ast::Stmt) -> Result<(), CompileError> {
        match stmt {
            ast::Stmt::LocalAssignment(la) => self.compile_local_assignment(la),
            ast::Stmt::Assignment(a) => self.compile_assignment(a),
            ast::Stmt::Do(d) => self.compile_do(d),
            ast::Stmt::While(w) => self.compile_while(w),
            ast::Stmt::Repeat(r) => self.compile_repeat(r),
            ast::Stmt::If(i) => self.compile_if(i),
            ast::Stmt::NumericFor(nf) => self.compile_numeric_for(nf),
            ast::Stmt::FunctionCall(fc) => self.compile_call_stmt(fc),
            ast::Stmt::LocalFunction(lf) => self.compile_local_function(lf),
            ast::Stmt::FunctionDeclaration(fd) => self.compile_function_decl(fd),
            ast::Stmt::Goto(g) => self.compile_goto(g),
            ast::Stmt::Label(l) => self.compile_label(l),
            ast::Stmt::GenericFor(gf) => {
                let token = gf.for_token();
                Err(self.unsupported(token.start_position(), "generic for"))
            }
            _ => {
                // Catch-all for any future AST variants (LuaU, etc.).
                Ok(())
            }
        }
    }

    fn compile_last_stmt(
        &mut self,
        stmt: &ast::LastStmt,
    ) -> Result<(), CompileError> {
        match stmt {
            ast::LastStmt::Return(r) => self.compile_return(r),
            ast::LastStmt::Break(b) => {
                Err(self.unsupported(b.start_position(), "break outside loop"))
            }
            ast::LastStmt::Continue(c) => {
                Err(self.unsupported(c.start_position(), "continue"))
            }
            _ => Ok(()),
        }
    }

    // -----------------------------------------------------------------------
    // Local assignment
    // -----------------------------------------------------------------------

    fn compile_local_assignment(
        &mut self,
        la: &ast::LocalAssignment,
    ) -> Result<(), CompileError> {
        let names: Vec<_> = la.names().iter().collect();
        let attrs: Vec<_> = la.attributes().collect();
        let exprs: Vec<_> = la.expressions().iter().collect();

        // Check for <close> attribute — unsupported in Phase 1.
        for (i, name_tok) in names.iter().enumerate() {
            if let Some(Some(attr)) = attrs.get(i) {
                let attr_str = tok_str(attr.name());
                if attr_str.as_ref() == b"close" {
                    return Err(self.unsupported(
                        name_tok.start_position(),
                        "<close> attribute",
                    ));
                }
            }
        }

        // Evaluate right-hand-side expressions into temporaries.
        let mut rhs_regs: Vec<u8> = Vec::new();
        for expr in &exprs {
            let tmp = self.alloc_temp();
            self.compile_expr(expr, tmp)?;
            rhs_regs.push(tmp);
        }

        // Declare local variables and move values in.
        for (i, name_tok) in names.iter().enumerate() {
            let attr = match attrs.get(i) {
                Some(Some(a)) => match tok_str(a.name()).as_ref() {
                    b"const" => LocalAttr::Const,
                    _ => LocalAttr::None,
                },
                _ => LocalAttr::None,
            };

            let name = tok_str(name_tok);
            let pc = self.cg.pc();
            let slot = self.scope.declare(name, attr, pc).map_err(|msg| {
                CompileError::Semantic {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    message: msg,
                }
            })?;

            if let Some(&rhs) = rhs_regs.get(i) {
                self.cg.emit(Instruction::Move { dst: slot, src: rhs });
            } else {
                self.cg.emit(Instruction::LoadNil { dst: slot });
            }
        }

        // Release temporaries (in reverse order).
        for _ in &exprs {
            self.free_temp();
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Assignment to existing variables / table fields
    // -----------------------------------------------------------------------

    fn compile_assignment(&mut self, a: &ast::Assignment) -> Result<(), CompileError> {
        let vars: Vec<_> = a.variables().iter().collect();
        let exprs: Vec<_> = a.expressions().iter().collect();

        // Evaluate RHS into temporaries.
        let mut rhs_regs: Vec<u8> = Vec::new();
        for expr in &exprs {
            let tmp = self.alloc_temp();
            self.compile_expr(expr, tmp)?;
            rhs_regs.push(tmp);
        }

        for (i, var) in vars.iter().enumerate() {
            let src = rhs_regs.get(i).copied();
            match var {
                ast::Var::Name(tok) => {
                    let name = tok_str(tok);
                    if let Some(local) = self.scope.resolve(&name) {
                        if local.attr == LocalAttr::Const {
                            return Err(CompileError::Semantic {
                                location: CSourceLocation {
                                    source_name: self.opts.source_name.clone(),
                                    line: tok.start_position().line() as u32,
                                    column: tok.start_position().character() as u32,
                                },
                                message: format!(
                                    "attempt to assign to const variable '{}'",
                                    String::from_utf8_lossy(&name)
                                ),
                            });
                        }
                        let slot = local.slot;
                        if let Some(src_reg) = src {
                            self.cg
                                .emit(Instruction::Move { dst: slot, src: src_reg });
                        } else {
                            self.cg.emit(Instruction::LoadNil { dst: slot });
                        }
                    } else {
                        // Global assignment.
                        let name_idx = self.cg.name(name);
                        let src_reg = if let Some(r) = src {
                            r
                        } else {
                            let tmp = self.alloc_temp();
                            self.cg.emit(Instruction::LoadNil { dst: tmp });
                            tmp
                        };
                        self.cg.emit(Instruction::SetGlobal {
                            name: name_idx,
                            src: src_reg,
                        });
                        if src.is_none() {
                            self.free_temp();
                        }
                    }
                }
                ast::Var::Expression(_ve) => {
                    // Table field assignment — Phase 2.
                    return Err(CompileError::UnsupportedFeature {
                        location: CSourceLocation {
                            source_name: self.opts.source_name.clone(),
                            line: 0,
                            column: 0,
                        },
                        feature: "table field assignment",
                    });
                }
                _ => {}
            }
        }

        for _ in &exprs {
            self.free_temp();
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Control flow
    // -----------------------------------------------------------------------

    fn compile_do(&mut self, d: &ast::Do) -> Result<(), CompileError> {
        self.compile_block(d.block())
    }

    fn compile_while(&mut self, w: &ast::While) -> Result<(), CompileError> {
        let cond_pc = self.cg.pc();
        let tmp = self.alloc_temp();
        self.compile_expr(w.condition(), tmp)?;
        let exit_jump = self.cg.emit_branch_false(tmp);
        self.free_temp();

        self.compile_block(w.block())?;
        // Jump back to condition.
        let back_jump = self.cg.emit_jump();
        self.cg.patch(back_jump, cond_pc);

        let exit_pc = self.cg.pc();
        self.cg.patch(exit_jump, exit_pc);
        Ok(())
    }

    fn compile_repeat(&mut self, r: &ast::Repeat) -> Result<(), CompileError> {
        let body_pc = self.cg.pc();
        self.compile_block(r.block())?;

        // `repeat ... until cond` loops until cond is truthy.
        let tmp = self.alloc_temp();
        self.compile_expr(r.until(), tmp)?;
        // If cond is false, jump back to body.
        let back_jump = self.cg.emit_branch_false(tmp);
        self.cg.patch(back_jump, body_pc);
        self.free_temp();
        Ok(())
    }

    fn compile_if(&mut self, stmt: &ast::If) -> Result<(), CompileError> {
        let mut end_jumps: Vec<usize> = Vec::new();

        // Condition.
        let tmp = self.alloc_temp();
        self.compile_expr(stmt.condition(), tmp)?;
        let else_jump = self.cg.emit_branch_false(tmp);
        self.free_temp();

        self.compile_block(stmt.block())?;

        // Process `elseif` chains.
        let mut next_else_jump = else_jump;
        for elseif in stmt.else_if().iter().flat_map(|e| e.iter()) {
            let end_jump = self.cg.emit_jump();
            end_jumps.push(end_jump);

            let elseif_pc = self.cg.pc();
            self.cg.patch(next_else_jump, elseif_pc);

            let tmp = self.alloc_temp();
            self.compile_expr(elseif.condition(), tmp)?;
            next_else_jump = self.cg.emit_branch_false(tmp);
            self.free_temp();

            self.compile_block(elseif.block())?;
        }

        // `else` branch.
        let end_jump = self.cg.emit_jump();
        end_jumps.push(end_jump);

        let else_pc = self.cg.pc();
        self.cg.patch(next_else_jump, else_pc);

        if let Some(else_block) = stmt.else_block() {
            self.compile_block(else_block)?;
        }

        let end_pc = self.cg.pc();
        for j in end_jumps {
            self.cg.patch(j, end_pc);
        }
        Ok(())
    }

    fn compile_numeric_for(&mut self, nf: &ast::NumericFor) -> Result<(), CompileError> {
        let var_name = tok_str(nf.index_variable());

        // Allocate three consecutive registers for counter, limit, step.
        let counter = self.scope.current_slot() + self.temp_top;
        let limit = counter + 1;
        let step = counter + 2;
        self.temp_top += 3;

        // Evaluate start, limit, step into these registers.
        self.compile_expr(nf.start(), counter)?;
        self.compile_expr(nf.end(), limit)?;
        if let Some(step_expr) = nf.step() {
            self.compile_expr(step_expr, step)?;
        } else {
            self.cg.emit(Instruction::LoadInt { dst: step, value: 1 });
        }

        // ForPrep: check if loop should execute.
        let for_prep_idx = self.cg.emit(Instruction::ForPrep {
            counter,
            limit,
            step,
            exit_offset: 0, // patched below
        });

        // Declare the loop variable as a local in the loop body scope.
        let body_pc = self.cg.pc();
        self.scope.push_scope();
        let slot = self.scope.declare(var_name, LocalAttr::None, body_pc).map_err(|msg| {
            CompileError::Semantic {
                location: CSourceLocation {
                    source_name: self.opts.source_name.clone(),
                    line: 0,
                    column: 0,
                },
                message: msg,
            }
        })?;
        // Copy counter value into loop variable.
        self.cg.emit(Instruction::Move { dst: slot, src: counter });

        self.compile_block_stmts(nf.block())?;

        self.scope.pop_scope();

        // ForStep: increment counter and branch back to body.
        let for_step_idx = self.cg.emit(Instruction::ForStep {
            counter,
            limit,
            step,
            body_offset: 0, // patched below
        });
        self.cg.patch_for_step(for_step_idx, body_pc);

        let exit_pc = self.cg.pc();
        self.cg.patch_for_prep(for_prep_idx, exit_pc);

        self.temp_top -= 3;
        Ok(())
    }

    /// Compile only the statements of a block (without opening a new scope).
    fn compile_block_stmts(&mut self, block: &ast::Block) -> Result<(), CompileError> {
        for stmt in block.stmts() {
            self.compile_stmt(stmt)?;
        }
        if let Some(last) = block.last_stmt() {
            self.compile_last_stmt(last)?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Return
    // -----------------------------------------------------------------------

    fn compile_return(&mut self, r: &ast::Return) -> Result<(), CompileError> {
        let exprs: Vec<_> = r.returns().iter().collect();
        if exprs.is_empty() {
            self.cg.emit(Instruction::Return { base: 0, nresults: 0 });
            return Ok(());
        }

        // Evaluate all return expressions into consecutive temporaries.
        let base = self.scope.current_slot() + self.temp_top;
        let last_idx = exprs.len() - 1;
        let mut count = 0i32;
        for (i, expr) in exprs.iter().enumerate() {
            let reg = base + count as u8;
            self.temp_top += 1;
            // The last expression may produce multiple values: use nresults=-1
            // when it is a function call so all results land in consecutive regs.
            let is_last_call = i == last_idx
                && matches!(
                    expr,
                    ast::Expression::FunctionCall(_)
                );
            if is_last_call {
                // Compile the call with nresults=-1 (expand all return values).
                if let ast::Expression::FunctionCall(fc) = expr {
                    self.compile_function_call(fc, reg, -1)?;
                }
                // nresults=-1 signals Return to return everything from base.
                self.cg.emit(Instruction::Return { base, nresults: -1 });
                self.temp_top -= count as u8 + 1;
                return Ok(());
            }
            self.compile_expr(expr, reg)?;
            count += 1;
        }
        self.cg.emit(Instruction::Return {
            base,
            nresults: count,
        });
        self.temp_top -= count as u8;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Goto / label
    // -----------------------------------------------------------------------

    fn compile_goto(&mut self, g: &ast52::Goto) -> Result<(), CompileError> {
        let label_name = tok_str(g.label_name());

        // Check if the label is already defined (backward goto).
        if let Some((_, target_pc)) =
            self.labels.iter().find(|(n, _)| n == &label_name)
        {
            let target_pc = *target_pc;
            let jump_idx = self.cg.emit_jump();
            self.cg.patch(jump_idx, target_pc);
        } else {
            // Forward goto — record for patching when label is encountered.
            let jump_idx = self.cg.emit_jump();
            let depth = self.scope.scope_depth();
            self.pending_gotos.push((label_name, jump_idx, depth));
        }
        Ok(())
    }

    fn compile_label(&mut self, l: &ast52::Label) -> Result<(), CompileError> {
        let label_name = tok_str(l.name());
        let target_pc = self.cg.pc();

        // Emit a runtime no-op (Label instruction is stripped by VM).
        let name_idx = self.cg.name(label_name.clone());
        self.cg.emit(Instruction::Label { name: name_idx });

        // Record for backward gotos.
        self.labels.push((label_name.clone(), target_pc));

        // Patch any pending forward gotos that refer to this label.
        let mut i = 0;
        while i < self.pending_gotos.len() {
            if self.pending_gotos[i].0 == label_name {
                let jump_idx = self.pending_gotos[i].1;
                self.cg.patch(jump_idx, target_pc);
                self.pending_gotos.remove(i);
            } else {
                i += 1;
            }
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Function declarations
    // -----------------------------------------------------------------------

    fn compile_local_function(
        &mut self,
        lf: &ast::LocalFunction,
    ) -> Result<(), CompileError> {
        let name = tok_str(lf.name());

        // Declare the local first (allows recursion).
        let pc = self.cg.pc();
        let slot = self.scope.declare(name.clone(), LocalAttr::None, pc).map_err(|msg| {
            CompileError::Semantic {
                location: CSourceLocation {
                    source_name: self.opts.source_name.clone(),
                    line: 0,
                    column: 0,
                },
                message: msg,
            }
        })?;

        let proto_idx = self.compile_function_body(name, lf.body())?;
        self.cg.emit(Instruction::NewClosure {
            dst: slot,
            proto_idx: proto_idx as u16,
        });
        Ok(())
    }

    fn compile_function_decl(
        &mut self,
        fd: &ast::FunctionDeclaration,
    ) -> Result<(), CompileError> {
        // Only support simple `function name(...)` at the top level.
        let func_name = fd.name();
        let names: Vec<_> = func_name.names().iter().collect();
        if names.len() != 1 || func_name.method_name().is_some() {
            return Err(CompileError::UnsupportedFeature {
                location: CSourceLocation {
                    source_name: self.opts.source_name.clone(),
                    line: 0,
                    column: 0,
                },
                feature: "method or dotted function declaration",
            });
        }
        let name = tok_str(names[0]);

        let tmp = self.alloc_temp();
        let proto_idx = self.compile_function_body(name.clone(), fd.body())?;
        self.cg.emit(Instruction::NewClosure {
            dst: tmp,
            proto_idx: proto_idx as u16,
        });

        // Assign to local or global.
        if let Some(local) = self.scope.resolve(&name) {
            if local.attr == LocalAttr::Const {
                return Err(CompileError::Semantic {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    message: format!(
                        "attempt to assign to const variable '{}'",
                        String::from_utf8_lossy(&name)
                    ),
                });
            }
            let slot = local.slot;
            self.cg.emit(Instruction::Move { dst: slot, src: tmp });
        } else {
            let name_idx = self.cg.name(name);
            self.cg.emit(Instruction::SetGlobal { name: name_idx, src: tmp });
        }
        self.free_temp();
        Ok(())
    }

    /// Compile a function body into a child `Proto`.  Returns the index in
    /// `self.child_protos`.
    fn compile_function_body(
        &mut self,
        name: Bytes,
        body: &ast::FunctionBody,
    ) -> Result<usize, CompileError> {
        let mut child = FnCompiler::new(self.opts);

        // Declare parameters as locals in the child's scope.
        let params: Vec<_> = body.parameters().iter().collect();
        let mut param_specs: Vec<ParamSpec> = Vec::new();
        let mut variadic = false;
        for param in &params {
            match param {
                ast::Parameter::Name(tok) => {
                    let pname = tok_str(tok);
                    let slot = child
                        .scope
                        .declare(pname.clone(), LocalAttr::None, 0)
                        .map_err(|msg| CompileError::Semantic {
                            location: CSourceLocation {
                                source_name: self.opts.source_name.clone(),
                                line: 0,
                                column: 0,
                            },
                            message: msg,
                        })?;
                    param_specs.push(ParamSpec {
                        name: Some(pname),
                        runtime_type: None,
                        lua_type: None,
                    });
                    let _ = slot;
                }
                ast::Parameter::Ellipsis(_) => {
                    variadic = true;
                }
                _ => {}
            }
        }

        // Compile the body block.
        child.compile_block_stmts(body.block())?;

        // Ensure there is always a Return at the end.
        if !matches!(
            child.cg.instructions.last(),
            Some(Instruction::Return { .. })
        ) {
            child.cg.emit(Instruction::Return { base: 0, nresults: 0 });
        }

        let sig = Arc::new(FunctionSignature {
            name,
            type_params: vec![],
            params: param_specs,
            variadic,
            returns: None,
            lua_returns: None,
        });

        let proto = Arc::new(Proto {
            signature: sig,
            instructions: child.cg.instructions,
            constants: child.cg.constants,
            locals: vec![],
            upvalues: vec![],
            protos: child.child_protos,
            source_locations: vec![],
        });

        let idx = self.child_protos.len();
        self.child_protos.push(proto);
        Ok(idx)
    }

    // -----------------------------------------------------------------------
    // Function calls (as statements)
    // -----------------------------------------------------------------------

    fn compile_call_stmt(
        &mut self,
        fc: &ast::FunctionCall,
    ) -> Result<(), CompileError> {
        let tmp = self.alloc_temp();
        self.compile_function_call(fc, tmp, 0)?;
        self.free_temp();
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    /// Compile an expression and place its result in `dst`.
    fn compile_expr(
        &mut self,
        expr: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        match expr {
            ast::Expression::Number(tok) => {
                self.compile_number(tok, dst)?;
            }
            ast::Expression::String(tok) => {
                let s = parse_string_literal(tok);
                let idx = self.cg.constant(s);
                self.cg.emit(Instruction::LoadK { dst, idx });
            }
            ast::Expression::Symbol(tok) => {
                match tok.token().to_string().as_str() {
                    "nil" => {
                        self.cg.emit(Instruction::LoadNil { dst });
                    }
                    "true" => {
                        self.cg.emit(Instruction::LoadBool { dst, value: true });
                    }
                    "false" => {
                        self.cg.emit(Instruction::LoadBool { dst, value: false });
                    }
                    _ => {
                        return Err(self.unsupported(
                            tok.start_position(),
                            "unknown symbol expression",
                        ));
                    }
                }
            }
            ast::Expression::Var(var) => {
                self.compile_var_expr(var, dst)?;
            }
            ast::Expression::BinaryOperator { lhs, binop, rhs } => {
                self.compile_binop(lhs, binop, rhs, dst)?;
            }
            ast::Expression::UnaryOperator { unop, expression } => {
                self.compile_unop(unop, expression, dst)?;
            }
            ast::Expression::FunctionCall(fc) => {
                self.compile_function_call(fc, dst, 1)?;
            }
            ast::Expression::Function(anon) => {
                let name = Bytes::from_static(b"<anonymous>");
                let proto_idx = self.compile_function_body(name, anon.body())?;
                self.cg.emit(Instruction::NewClosure {
                    dst,
                    proto_idx: proto_idx as u16,
                });
            }
            ast::Expression::Parentheses { expression, .. } => {
                self.compile_expr(expression, dst)?;
            }
            ast::Expression::TableConstructor(_tc) => {
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    feature: "table constructor",
                });
            }
            _ => {
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    feature: "unsupported expression",
                });
            }
        }
        Ok(())
    }

    fn compile_var_expr(
        &mut self,
        var: &ast::Var,
        dst: u8,
    ) -> Result<(), CompileError> {
        match var {
            ast::Var::Name(tok) => {
                let name = tok_str(tok);
                if let Some(local) = self.scope.resolve(&name) {
                    let slot = local.slot;
                    if slot != dst {
                        self.cg.emit(Instruction::Move { dst, src: slot });
                    }
                } else {
                    let name_idx = self.cg.name(name);
                    self.cg.emit(Instruction::GetGlobal { dst, name: name_idx });
                }
            }
            ast::Var::Expression(_ve) => {
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    feature: "table index expression",
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn compile_number(
        &mut self,
        tok: &full_moon::tokenizer::TokenReference,
        dst: u8,
    ) -> Result<(), CompileError> {
        let s_bytes = tok_str(tok);
        let s = std::str::from_utf8(s_bytes.as_ref()).unwrap_or("");
        // Try integer first.
        if let Ok(i) = parse_integer(s) {
            self.cg.emit(Instruction::LoadInt { dst, value: i });
            return Ok(());
        }
        // Fall back to float.
        if let Ok(f) = s.parse::<f64>() {
            self.cg.emit(Instruction::LoadFloat { dst, value: f });
            return Ok(());
        }
        Err(CompileError::Semantic {
            location: CSourceLocation {
                source_name: self.opts.source_name.clone(),
                line: tok.start_position().line() as u32,
                column: tok.start_position().character() as u32,
            },
            message: format!("cannot parse number literal: {s}"),
        })
    }

    fn compile_binop(
        &mut self,
        lhs: &ast::Expression,
        binop: &ast::BinOp,
        rhs: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        use ast::BinOp;

        // Short-circuit `and` / `or`.
        match binop {
            BinOp::And(_) => return self.compile_and(lhs, rhs, dst),
            BinOp::Or(_) => return self.compile_or(lhs, rhs, dst),
            _ => {}
        }

        let l = self.alloc_temp();
        self.compile_expr(lhs, l)?;
        let r = self.alloc_temp();
        self.compile_expr(rhs, r)?;

        let instr = match binop {
            BinOp::Plus(_) => Instruction::Add { dst, lhs: l, rhs: r },
            BinOp::Minus(_) => Instruction::Sub { dst, lhs: l, rhs: r },
            BinOp::Star(_) => Instruction::Mul { dst, lhs: l, rhs: r },
            BinOp::Slash(_) => Instruction::Div { dst, lhs: l, rhs: r },
            BinOp::DoubleSlash(_) => Instruction::IDiv { dst, lhs: l, rhs: r },
            BinOp::Percent(_) => Instruction::Mod { dst, lhs: l, rhs: r },
            BinOp::Caret(_) => Instruction::Pow { dst, lhs: l, rhs: r },
            BinOp::Ampersand(_) => Instruction::BAnd { dst, lhs: l, rhs: r },
            BinOp::Pipe(_) => Instruction::BOr { dst, lhs: l, rhs: r },
            BinOp::Tilde(_) => Instruction::BXor { dst, lhs: l, rhs: r },
            BinOp::DoubleLessThan(_) => Instruction::Shl { dst, lhs: l, rhs: r },
            BinOp::DoubleGreaterThan(_) => Instruction::Shr { dst, lhs: l, rhs: r },
            BinOp::TwoEqual(_) => Instruction::Eq { dst, lhs: l, rhs: r },
            BinOp::TildeEqual(_) => Instruction::Ne { dst, lhs: l, rhs: r },
            BinOp::LessThan(_) => Instruction::Lt { dst, lhs: l, rhs: r },
            BinOp::LessThanEqual(_) => Instruction::Le { dst, lhs: l, rhs: r },
            BinOp::GreaterThan(_) => Instruction::Gt { dst, lhs: l, rhs: r },
            BinOp::GreaterThanEqual(_) => Instruction::Ge { dst, lhs: l, rhs: r },
            BinOp::TwoDots(_) => {
                // String concatenation — Phase 1 uses Concat.
                // For exactly two operands it's straightforward.
                self.free_temp(); // r
                self.free_temp(); // l
                // Re-allocate in order.
                let base = self.alloc_temp();
                self.compile_expr(lhs, base)?;
                let r2 = self.alloc_temp();
                self.compile_expr(rhs, r2)?;
                self.cg.emit(Instruction::Concat { dst, base, count: 2 });
                self.free_temp();
                self.free_temp();
                return Ok(());
            }
            _ => {
                self.free_temp();
                self.free_temp();
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    feature: "unsupported binary operator",
                });
            }
        };

        self.cg.emit(instr);
        self.free_temp(); // r
        self.free_temp(); // l
        Ok(())
    }

    fn compile_and(
        &mut self,
        lhs: &ast::Expression,
        rhs: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        // `a and b` → if a is falsy, result = a; else result = b.
        self.compile_expr(lhs, dst)?;
        let skip_rhs = self.cg.emit_branch_false(dst);
        self.compile_expr(rhs, dst)?;
        let end_pc = self.cg.pc();
        self.cg.patch(skip_rhs, end_pc);
        Ok(())
    }

    fn compile_or(
        &mut self,
        lhs: &ast::Expression,
        rhs: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        // `a or b` → if a is truthy, result = a; else result = b.
        self.compile_expr(lhs, dst)?;
        let skip_rhs = self.cg.emit_branch_true(dst);
        self.compile_expr(rhs, dst)?;
        let end_pc = self.cg.pc();
        self.cg.patch(skip_rhs, end_pc);
        Ok(())
    }

    fn compile_unop(
        &mut self,
        unop: &ast::UnOp,
        expr: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        let tmp = self.alloc_temp();
        self.compile_expr(expr, tmp)?;
        let instr = match unop {
            ast::UnOp::Minus(_) => Instruction::Neg { dst, src: tmp },
            ast::UnOp::Not(_) => Instruction::Not { dst, src: tmp },
            ast::UnOp::Hash(_) => Instruction::Len { dst, src: tmp },
            ast::UnOp::Tilde(_) => Instruction::BNot { dst, src: tmp },
            _ => {
                self.free_temp();
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    feature: "unsupported unary operator",
                });
            }
        };
        self.cg.emit(instr);
        self.free_temp();
        Ok(())
    }

    /// Compile a function call.  `nresults` is how many return values to keep
    /// (1 = single value into `dst`; 0 = called as statement).
    fn compile_function_call(
        &mut self,
        fc: &ast::FunctionCall,
        dst: u8,
        nresults: i32,
    ) -> Result<(), CompileError> {
        // Collect all suffixes so we can handle chained calls.
        let suffixes: Vec<_> = fc.suffixes().collect();

        // Last suffix must be a Call.
        let call_args = match suffixes.last() {
            Some(ast::Suffix::Call(ast::Call::AnonymousCall(args))) => args,
            Some(ast::Suffix::Call(ast::Call::MethodCall(_))) => {
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    feature: "method call",
                });
            }
            _ => {
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation {
                        source_name: self.opts.source_name.clone(),
                        line: 0,
                        column: 0,
                    },
                    feature: "non-call suffix",
                });
            }
        };

        // Phase 1: only support simple `name(args)` — no chained indexing.
        if suffixes.len() > 1 {
            return Err(CompileError::UnsupportedFeature {
                location: CSourceLocation {
                    source_name: self.opts.source_name.clone(),
                    line: 0,
                    column: 0,
                },
                feature: "chained call (a.b() or a[b]())",
            });
        }

        // Evaluate function value into `dst`.
        self.compile_prefix_expr(fc.prefix(), dst)?;

        // Evaluate arguments.
        let mut nargs = 0i32;
        if let ast::FunctionArgs::Parentheses { arguments, .. } = call_args {
            for arg in arguments.iter() {
                let arg_reg = dst + 1 + nargs as u8;
                self.compile_expr(arg, arg_reg)?;
                nargs += 1;
            }
        } else if let ast::FunctionArgs::String(s) = call_args {
            // f "string" shorthand.
            let bytes = parse_string_literal(s);
            let idx = self.cg.constant(bytes);
            self.cg.emit(Instruction::LoadK { dst: dst + 1, idx });
            nargs = 1;
        }

        self.cg.emit(Instruction::Call {
            func: dst,
            nargs,
            nresults,
        });
        Ok(())
    }

    fn compile_prefix_expr(
        &mut self,
        prefix: &ast::Prefix,
        dst: u8,
    ) -> Result<(), CompileError> {
        match prefix {
            ast::Prefix::Name(tok) => {
                let name = tok_str(tok);
                if let Some(local) = self.scope.resolve(&name) {
                    let slot = local.slot;
                    if slot != dst {
                        self.cg.emit(Instruction::Move { dst, src: slot });
                    }
                } else {
                    let name_idx = self.cg.name(name);
                    self.cg.emit(Instruction::GetGlobal { dst, name: name_idx });
                }
            }
            ast::Prefix::Expression(e) => {
                self.compile_expr(e, dst)?;
            }
            _ => {}
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Finish
    // -----------------------------------------------------------------------

    fn finish(mut self, name: Bytes, params: Vec<ParamSpec>, variadic: bool) -> Proto {
        // Ensure every path ends with a Return.
        if !matches!(
            self.cg.instructions.last(),
            Some(Instruction::Return { .. })
        ) {
            self.cg.emit(Instruction::Return { base: 0, nresults: 0 });
        }

        let sig = Arc::new(FunctionSignature {
            name,
            type_params: vec![],
            params,
            variadic,
            returns: None,
            lua_returns: None,
        });

        Proto {
            signature: sig,
            instructions: self.cg.instructions,
            constants: self.cg.constants,
            locals: vec![],
            upvalues: vec![],
            protos: self.child_protos,
            source_locations: vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the raw identifier text from a `Token`, asserting it is an `Identifier`.
fn ident(tok: &Token) -> &str {
    match tok.token_type() {
        TokenType::Identifier { identifier } => identifier.as_str(),
        _ => unreachable!("expected {:?} to be an Identifier token", tok),
    }
}

/// Extract identifier text from a `TokenReference` as owned `Bytes`.
/// For identifier tokens, delegates to `ident`; general tokens fall back to
/// `to_string()` (used for numeric literals, etc.).
fn tok_str(tok: &TokenReference) -> Bytes {
    match tok.token().token_type() {
        TokenType::Identifier { .. } => Bytes::copy_from_slice(ident(tok.token()).as_bytes()),
        _ => Bytes::copy_from_slice(tok.token().to_string().as_bytes()),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn lower_chunk(ast: &Ast, opts: &CompileOptions) -> Result<Proto, CompileError> {
    let mut compiler = FnCompiler::new(opts);

    // The top-level chunk is an implicit function with no parameters.
    for stmt in ast.nodes().stmts() {
        compiler.compile_stmt(stmt)?;
    }
    if let Some(last) = ast.nodes().last_stmt() {
        compiler.compile_last_stmt(last)?;
    }

    Ok(compiler.finish(
        Bytes::copy_from_slice(opts.source_name.as_bytes()),
        vec![],
        true, // top-level chunk is variadic
    ))
}

// ---------------------------------------------------------------------------
// Number parsing helpers
// ---------------------------------------------------------------------------

fn parse_integer(s: &str) -> Result<i64, ()> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).map_err(|_| ())
    } else if s.contains('.') || s.contains('e') || s.contains('E') {
        Err(())
    } else {
        s.parse::<i64>().map_err(|_| ())
    }
}

fn parse_string_literal(tok: &full_moon::tokenizer::TokenReference) -> Bytes {
    let raw = tok.token().to_string();
    // Strip surrounding quotes and handle basic escape sequences.
    let inner = if raw.starts_with('"') || raw.starts_with('\'') {
        &raw[1..raw.len() - 1]
    } else if raw.starts_with('[') {
        // Long string: find `[[` / `[=[` etc.
        let level = raw.chars().take_while(|&c| c == '=').count();
        let skip = level + 2; // opening [=*[
        &raw[skip..raw.len() - skip]
    } else {
        raw.as_str()
    };
    // TODO: proper escape handling in Phase 2 (strings).
    Bytes::copy_from_slice(inner.as_bytes())
}
