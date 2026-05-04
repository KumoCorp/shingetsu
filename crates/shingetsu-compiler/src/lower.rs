//! Walks the `full_moon` AST and emits IR for a single `Proto`.
//!
//! Supports: numeric and string literals, arithmetic, comparisons, logical
//! operators, local variables (`<const>`, `<close>`, no attribute), `if`,
//! `while`, `repeat`, numeric `for`, `do...end`, `goto`/`::label::`,
//! function definitions and calls (including method calls and chained
//! indexing), tables (constructors, field reads, field writes), `break`,
//! `return`, and multiple return values.
//!
//! Unsupported constructs produce `CompileError::UnsupportedFeature`.

use parking_lot::Mutex;
use std::sync::Arc;

use full_moon::ast::{self, lua52 as ast52, Ast};
use full_moon::tokenizer::{Token, TokenReference, TokenType};
use shingetsu_vm::ir::Instruction;
use shingetsu_vm::proto::Proto;
use shingetsu_vm::types::{FunctionSignature, LocalAttr, ParamSpec, TypeAlias};
use shingetsu_vm::value::Value;
use shingetsu_vm::{bytecode, Bytes};

use shingetsu_vm::proto::{LocalDesc, UpvalueDesc};

use crate::codegen::CodeGen;
use crate::error::{CompileError, Diagnostic, LintId, SourceLocation as CSourceLocation};
use crate::scope::ScopeStack;
use crate::Compiler;

// ---------------------------------------------------------------------------
// Function compiler state
// ---------------------------------------------------------------------------

/// Tracks the registers of a pending `break` statement that needs patching
/// once the loop exit PC is known.
struct BreakInfo {
    /// Instruction indices of the placeholder `Jump` instructions emitted by
    /// `break` statements inside this loop.
    patch_list: Vec<usize>,
    /// Instruction indices of the placeholder `Jump` instructions emitted by
    /// `continue` statements inside this loop.
    continue_patch_list: Vec<usize>,
    /// Scope depth at the point the loop was entered, used to determine which
    /// `<close>` variables must be closed when `break` or `continue` executes.
    scope_depth: usize,
    /// First register slot whose open upvalues need closing on `break` /
    /// `continue` exit, so each iteration's closures get their own upvalue
    /// identity (Lua 5.4 §3.3.5).  `None` when the loop has no per-iteration
    /// local that closures might capture (e.g. `while`/`repeat`).
    close_upvalues_from: Option<u8>,
}

struct FnCompiler<'a> {
    compiler: &'a Compiler,
    cg: CodeGen,
    scope: ScopeStack,
    /// Named labels: `(name_bytes, target_pc)`.
    labels: Vec<(Bytes, usize)>,
    /// Pending gotos waiting for a label: `(name_bytes, jump_instr_idx, scope_depth)`.
    pending_gotos: Vec<(Bytes, usize, usize)>,
    /// Nested function bodies compiled separately.
    child_protos: Vec<Arc<Proto>>,
    /// Simple top-of-stack temporary register allocator.
    temp_top: u8,
    /// High-water mark for register slots used by this function.
    max_stack_size: u16,
    /// Stack of active loops; each entry tracks break-jump patch sites and
    /// the scope depth at loop entry.
    break_stacks: Vec<BreakInfo>,
    /// Upvalue descriptors discovered for this function during compilation.
    /// Wrapped in `Rc<RefCell<>>` so that child compilers can add entries to
    /// an ancestor's list when threading a multi-level capture.
    upvalue_descs: Arc<Mutex<Vec<UpvalueDesc>>>,
    /// Shared upvalue descriptor lists for each ancestor function.
    /// Index 0 = direct parent's list, 1 = grandparent's, …
    /// A child compiler holds `Rc` clones of these so that `resolve_upvalue`
    /// can insert descriptors into intermediate levels as needed.
    ancestor_upvalue_descs: Vec<Arc<Mutex<Vec<UpvalueDesc>>>>,
    /// Live locals from ancestor functions, for upvalue resolution.
    /// Index 0 = direct parent's locals (name, slot, attr), 1 = grandparent's, …
    /// `attr` is propagated so that const violations in nested closures
    /// (`local x <const> = 1; function() x = 2 end`) can be reported at
    /// compile time.
    ancestor_locals: Vec<Vec<(Bytes, u8, LocalAttr)>>,
    /// Whether this function accepts varargs (`...` parameter or top-level chunk).
    is_variadic: bool,
    /// `LocalDesc` entries for `<close>` locals, collected during compilation
    /// and written to `Proto::locals` in `finish()`.  Used at runtime to
    /// find in-scope `<close>` values when unwinding errors through `pcall`.
    close_local_descs: Vec<LocalDesc>,
    /// `LocalDesc` entries for all locals (when debug_info is enabled),
    /// used to provide variable names in error messages at runtime.
    debug_local_descs: Vec<LocalDesc>,
    /// `type Name<...> = ...` aliases declared in this function scope.
    type_aliases: std::collections::HashMap<Bytes, TypeAlias>,
    /// Non-fatal diagnostics collected during compilation.
    diagnostics: Vec<Diagnostic>,
    /// Effective `package.path` search path, updated when the script
    /// assigns to `package.path` with a statically-evaluable RHS.
    /// Initialized from `Compiler::package_path`.
    effective_package_path: Option<String>,
    /// True when control flow has unconditionally exited the current
    /// block (via `return`, `break`, or `goto`).  Reset when control
    /// flow becomes reachable again (e.g. after an `if` without an
    /// else, or at a label target).
    exited: bool,
}

impl<'a> FnCompiler<'a> {
    fn new(compiler: &'a Compiler) -> Self {
        Self::new_with_ancestors(compiler, Vec::new(), Vec::new())
    }

    fn new_with_ancestors(
        compiler: &'a Compiler,
        ancestor_locals: Vec<Vec<(Bytes, u8, LocalAttr)>>,
        ancestor_upvalue_descs: Vec<Arc<Mutex<Vec<UpvalueDesc>>>>,
    ) -> Self {
        FnCompiler {
            compiler,
            cg: CodeGen::new(compiler.opts.debug_info),
            scope: ScopeStack::new(),
            labels: Vec::new(),
            pending_gotos: Vec::new(),
            child_protos: Vec::new(),
            temp_top: 0,
            max_stack_size: 0,
            break_stacks: Vec::new(),
            upvalue_descs: Arc::new(Mutex::new(Vec::new())),
            ancestor_upvalue_descs,
            ancestor_locals,
            is_variadic: false,
            close_local_descs: Vec::new(),
            debug_local_descs: Vec::new(),
            type_aliases: std::collections::HashMap::new(),
            diagnostics: Vec::new(),
            effective_package_path: compiler.package_path.clone(),
            exited: false,
        }
    }

    /// Shorthand for the compile options.
    fn opts(&self) -> &crate::CompileOptions {
        &self.compiler.opts
    }

    /// Look up `name` as an upvalue from an enclosing function.  Returns the
    /// upvalue index (into this function's upvalue list) if found, registering
    /// a new descriptor if this is the first reference.
    ///
    /// Correctly handles multi-level capture: when the variable lives in a
    /// grandparent (level > 0), each intermediate function gets an upvalue
    /// descriptor threaded through it so `NewClosure` can chain the live
    /// `Arc<RwLock<Value>>` cells all the way down.
    fn resolve_upvalue(&mut self, name: &[u8]) -> Option<u8> {
        // Already registered in this function?
        {
            let descs = self.upvalue_descs.lock();
            if let Some(idx) = descs.iter().position(|u| u.name.as_ref() == name) {
                return Some(idx as u8);
            }
        }

        // Walk ancestor locals to find where the variable lives.
        for (level, ancestor) in self.ancestor_locals.iter().enumerate() {
            if let Some((_, slot, _)) = ancestor.iter().find(|(n, _, _)| n.as_ref() == name) {
                // `level` == 0: variable is a local of the direct parent.
                // `level` > 0: variable lives in a grandparent (level + 1 deep).
                //
                // Build the upvalue chain from the variable's home down to
                // this function.  Each intermediate ancestor registers the
                // variable as an upvalue of the level above it.
                //
                // ancestor_upvalue_descs[j] is the upvalue list of the
                // ancestor at level j (j=0 is the direct parent).

                let name_bytes = Bytes::from(name);

                let final_idx = if level == 0 {
                    // Direct parent has the variable as a local: simple in-stack capture.
                    let mut descs = self.upvalue_descs.lock();
                    let idx = descs.len() as u8;
                    descs.push(UpvalueDesc {
                        name: name_bytes,
                        in_stack: true,
                        index: *slot,
                    });
                    idx
                } else {
                    // Step 1: register in ancestor_upvalue_descs[level-1]
                    // (the function that owns the local's immediate consumer).
                    // That ancestor captures directly from registers (in_stack: true).
                    let mut prev_idx = {
                        let mut descs = self.ancestor_upvalue_descs[level - 1].lock();
                        if let Some(idx) = descs.iter().position(|u| u.name.as_ref() == name) {
                            idx as u8
                        } else {
                            let idx = descs.len() as u8;
                            descs.push(UpvalueDesc {
                                name: name_bytes.clone(),
                                in_stack: true,
                                index: *slot,
                            });
                            idx
                        }
                    };

                    // Step 2: propagate as upvalue-of-upvalue through
                    // ancestor_upvalue_descs[level-2] down to [0].
                    for l in (0..level - 1).rev() {
                        let mut descs = self.ancestor_upvalue_descs[l].lock();
                        prev_idx =
                            if let Some(idx) = descs.iter().position(|u| u.name.as_ref() == name) {
                                idx as u8
                            } else {
                                let idx = descs.len() as u8;
                                descs.push(UpvalueDesc {
                                    name: name_bytes.clone(),
                                    in_stack: false,
                                    index: prev_idx,
                                });
                                idx
                            };
                    }

                    // Step 3: register in this function pointing to the
                    // direct parent's upvalue.
                    let mut descs = self.upvalue_descs.lock();
                    let idx = descs.len() as u8;
                    descs.push(UpvalueDesc {
                        name: name_bytes,
                        in_stack: false,
                        index: prev_idx,
                    });
                    idx
                };

                return Some(final_idx);
            }
        }
        // The identifier `_ENV` is implicit: if no ancestor has it as a
        // local (`local _ENV = ...`), fall back to the synthetic root
        // env upvalue.  Reaching this point for any other name means
        // the variable is genuinely free (a global).
        if name == b"_ENV" {
            return Some(self.ensure_env_upvalue());
        }
        None
    }

    /// Lazily register the synthetic `_ENV` upvalue, returning its
    /// index in this function's upvalue list.  Recurses up the
    /// ancestor chain so each enclosing function also has `_ENV`
    /// registered (with the appropriate `in_stack: false` capture
    /// pointing at the level above).  At the root (main chunk)
    /// `_ENV` is registered as a synthetic upvalue with `index: 0` —
    /// the runtime (`Task::new` / `Function::lua_with_env`) supplies
    /// the actual cell.
    fn ensure_env_upvalue(&mut self) -> u8 {
        {
            let descs = self.upvalue_descs.lock();
            if let Some(idx) = descs.iter().position(|d| d.name == "_ENV") {
                return idx as u8;
            }
        }
        let depth = self.ancestor_upvalue_descs.len();
        let prev_idx: u8 = if depth == 0 {
            // Root: synthetic.  `index` is unused; the runtime
            // (`Task::new` / `Function::lua_with_env`) supplies the cell.
            0
        } else {
            // Ensure `_ENV` exists in the deepest ancestor first, then
            // propagate down through each intermediate ancestor to the
            // direct parent.  `prev` is the index of `_ENV` in the
            // most recently visited ancestor's upvalue list.
            let root = depth - 1;
            let mut prev = ensure_env_in_ancestor(&self.ancestor_upvalue_descs[root], 0);
            for l in (0..root).rev() {
                prev = ensure_env_in_ancestor(&self.ancestor_upvalue_descs[l], prev);
            }
            prev
        };
        let mut descs = self.upvalue_descs.lock();
        let idx = descs.len() as u8;
        descs.push(UpvalueDesc {
            name: "_ENV".into(),
            in_stack: false,
            index: prev_idx,
        });
        idx
    }

    /// If a local named `_ENV` is currently in scope, return its
    /// register slot.  Free-name (global) reads and writes within that
    /// scope must lower to `GetTable`/`SetTable` against this register
    /// rather than `GetGlobal`/`SetGlobal` against the upvalue env.
    fn local_env_slot(&self) -> Option<u8> {
        self.scope.resolve(b"_ENV").map(|l| l.slot)
    }

    /// Emit code to read the free name at constant index `name_idx`
    /// into register `dst`.  Routes through a `local _ENV` if one is
    /// in scope; otherwise falls back to `GetGlobal` (which the
    /// runtime resolves via the env upvalue).  In the latter case we
    /// invoke `resolve_upvalue` so that an ancestor's `local _ENV`
    /// becomes a normal in-stack upvalue capture rather than a
    /// synthetic-root entry.
    fn emit_global_read(&mut self, name_idx: u16, dst: u8) -> Result<(), CompileError> {
        if let Some(env_slot) = self.local_env_slot() {
            let key_reg = self.alloc_temp()?;
            self.cg.emit(Instruction::LoadK {
                dst: key_reg,
                idx: name_idx,
            });
            self.cg.emit(Instruction::GetTable {
                dst,
                table: env_slot,
                key: key_reg,
            });
            self.free_temp();
        } else {
            self.resolve_upvalue(b"_ENV");
            self.cg.emit(Instruction::GetGlobal {
                dst,
                name: name_idx,
            });
        }
        Ok(())
    }

    /// Emit code to write the value in register `src` to the free
    /// name at constant index `name_idx`.  Mirrors `emit_global_read`
    /// for assignment.
    fn emit_global_write(&mut self, name_idx: u16, src: u8) -> Result<(), CompileError> {
        if let Some(env_slot) = self.local_env_slot() {
            let key_reg = self.alloc_temp()?;
            self.cg.emit(Instruction::LoadK {
                dst: key_reg,
                idx: name_idx,
            });
            self.cg.emit(Instruction::SetTable {
                table: env_slot,
                key: key_reg,
                src,
            });
            self.free_temp();
        } else {
            self.resolve_upvalue(b"_ENV");
            self.cg.emit(Instruction::SetGlobal {
                name: name_idx,
                src,
            });
        }
        Ok(())
    }

    fn loc(&self, pos: full_moon::tokenizer::Position) -> CSourceLocation {
        CSourceLocation::from_pos(&self.opts().source_name, pos)
    }

    /// Build a `CSourceLocation` spanning the full extent of an AST node.
    /// Falls back to an unknown location if the node has no position info.
    fn node_loc(&self, node: &impl full_moon::node::Node) -> CSourceLocation {
        match (node.start_position(), node.end_position()) {
            (Some(start), Some(end)) => {
                CSourceLocation::from_span(&self.opts().source_name, start, end)
            }
            (Some(pos), None) => CSourceLocation::from_pos(&self.opts().source_name, pos),
            _ => CSourceLocation::unknown(&self.opts().source_name),
        }
    }

    /// Set the current debug source location from an AST node's span.
    fn set_node_loc(&mut self, node: &impl full_moon::node::Node) {
        let loc = self.node_loc(node);
        if loc.line > 0 || loc.byte_offset > 0 {
            self.cg.set_loc(Some(loc.into()));
        }
    }

    /// Set the current debug source location from a span defined by two nodes.
    fn set_span_loc(
        &mut self,
        start_node: &impl full_moon::node::Node,
        end_node: &impl full_moon::node::Node,
    ) {
        let start = full_moon::node::Node::start_position(start_node);
        let end = full_moon::node::Node::end_position(end_node);
        if let (Some(start), Some(end)) = (start, end) {
            let loc = CSourceLocation::from_span(&self.opts().source_name, start, end);
            self.cg.set_loc(Some(loc.into()));
        }
    }

    /// Build the standard "attempt to assign to const variable 'x'" error
    /// pointing at `pos` (typically the name token of the assignment LHS).
    fn const_assign_error(
        &self,
        name: &Bytes,
        pos: full_moon::tokenizer::Position,
    ) -> CompileError {
        CompileError::Semantic {
            location: self.loc(pos),
            message: format!("attempt to assign to const variable '{name}'"),
            help: None,
        }
    }

    /// Look up the originating `LocalAttr` for an upvalue by walking the
    /// ancestor-locals chain.  Used so that writes to a captured `<const>`
    /// local can be rejected at compile time in the same way as direct
    /// writes to a local in the current function.  Returns `None` if the
    /// name isn't an ancestor local (e.g. it's a global).
    fn ancestor_local_attr(&self, name: &[u8]) -> Option<LocalAttr> {
        for ancestor in &self.ancestor_locals {
            if let Some((_, _, attr)) = ancestor.iter().find(|(n, _, _)| n.as_ref() == name) {
                return Some(*attr);
            }
        }
        None
    }

    fn unsupported(
        &self,
        pos: full_moon::tokenizer::Position,
        feature: &'static str,
    ) -> CompileError {
        CompileError::UnsupportedFeature {
            location: self.loc(pos),
            feature: feature.to_string(),
            help: None,
        }
    }

    /// Allocate a temporary register above the current locals.
    /// Record that register `slot` is used, updating the high-water mark.
    #[inline]
    fn note_reg_use(&mut self, slot: u8) {
        let needed = slot as u16 + 1;
        if needed > self.max_stack_size {
            self.max_stack_size = needed;
        }
    }

    fn alloc_temp(&mut self) -> Result<u8, CompileError> {
        let slot = (self.scope.current_slot() as u16) + (self.temp_top as u16);
        if slot > u8::MAX as u16 {
            return Err(CompileError::Semantic {
                location: self.cg.current_loc().map_or_else(
                    || CSourceLocation::unknown(&self.opts().source_name),
                    |loc| CSourceLocation {
                        source_name: self.opts().source_name.clone(),
                        line: loc.line,
                        column: loc.column,
                        byte_offset: loc.byte_offset,
                        byte_len: loc.byte_len,
                    },
                ),
                message: format!(
                    "too many local variables or temporaries (register limit is {}); \
                     consider refactoring into smaller functions",
                    u8::MAX
                ),
                help: None,
            });
        }
        self.temp_top += 1;
        self.note_reg_use(slot as u8);
        Ok(slot as u8)
    }

    /// Release the topmost temporary register.
    fn free_temp(&mut self) {
        if self.temp_top > 0 {
            self.temp_top -= 1;
        }
    }

    /// Pop the innermost scope and record `LocalDesc` entries for debug info.
    /// Skips `<close>` locals since those are already tracked in
    /// `close_local_descs` and would be double-counted at runtime.
    fn pop_scope_with_debug(&mut self) {
        let end_pc = self.cg.instructions.len();
        let locals = self.scope.pop_scope();
        for local in &locals {
            // Emit unused-variable warnings.
            self.check_unused_local(local);

            if self.opts().debug_info && local.attr != LocalAttr::Close {
                self.debug_local_descs.push(LocalDesc {
                    name: local.name.clone(),
                    attr: local.attr,
                    slot: local.slot,
                    start_pc: local.start_pc,
                    end_pc,
                    decl_location: local.decl_location.clone().map(Into::into),
                    is_implicit_self: local.is_implicit_self,
                });
            }
        }
    }

    /// Emit a warning if `local` was never read.
    fn check_unused_local(&mut self, local: &crate::scope::Local) {
        // Skip names starting with `_` (conventional "intentionally unused").
        if local.name.starts_with(b"_") {
            return;
        }
        // Skip `<close>` locals — their purpose is the __close side effect.
        if local.attr == LocalAttr::Close {
            return;
        }
        // Skip compiler-internal hidden variables (e.g. "(for index)").
        if local.name.starts_with(b"(") {
            return;
        }
        // Skip implicit `self` in method declarations — it is always
        // available but many methods legitimately never reference it.
        if local.name == &b"self"[..] && local.is_implicit_self {
            return;
        }

        if local.read_count == 0 {
            let name_str = String::from_utf8_lossy(&local.name);
            let (location, message) = if local.write_count > 0 {
                // Point to the last write site, not the declaration.
                let loc = local
                    .last_write_location
                    .clone()
                    .or_else(|| local.decl_location.clone())
                    .unwrap_or_else(|| CSourceLocation::unknown(&self.opts().source_name));
                (
                    loc,
                    format!("variable '{name_str}' is assigned to but never read"),
                )
            } else {
                let loc = local
                    .decl_location
                    .clone()
                    .unwrap_or_else(|| CSourceLocation::unknown(&self.opts().source_name));
                let kind = if local.is_function {
                    "function"
                } else {
                    "variable"
                };
                (loc, format!("unused {kind} '{name_str}'"))
            };
            self.diagnostics.push(Diagnostic {
                lint: LintId::UnusedVariable,
                severity: crate::error::Severity::Warning,
                location,
                message,
                help: Some(format!(
                    "prefix the name with '_' to suppress this warning: '_{name_str}'"
                )),
            });
        }
    }

    /// Returns `true` if the last emitted instruction is an unconditional exit
    /// (`Return` or `Jump`), meaning scope-exit `CloseVar` need not be emitted.
    fn already_unconditionally_exited(&self) -> bool {
        self.exited
    }

    /// Emit `CloseVar` for every `<close>` local in the **current** (innermost)
    /// scope, in reverse declaration order.  Call this just before `pop_scope`
    /// when the block exits without an unconditional jump.
    fn emit_close_for_scope(&mut self) {
        let slots: Vec<u8> = self
            .scope
            .close_vars_in_current_scope()
            .map(|l| l.slot)
            .collect();
        for slot in slots.into_iter().rev() {
            self.cg.emit(Instruction::CloseVar { slot });
        }
    }

    /// Emit `CloseVar` for all live `<close>` locals across every scope down
    /// to (but not including) scope depth `target_depth`.  Used by `return`
    /// (target_depth = 0) and `break` (target_depth = loop scope depth).
    fn emit_close_for_exit(&mut self, target_depth: usize) {
        let slots: Vec<u8> = self
            .scope
            .close_vars_for_exit(target_depth)
            .into_iter()
            .map(|l| l.slot)
            .collect();
        for slot in slots {
            self.cg.emit(Instruction::CloseVar { slot });
        }
    }

    /// Build an `UnsupportedFeature` error without a position (for cases where
    /// we don't have a token handy).
    fn unsupported_pos0(&self, feature: &'static str) -> CompileError {
        CompileError::UnsupportedFeature {
            location: CSourceLocation::unknown(&self.opts().source_name),
            feature: feature.to_string(),
            help: None,
        }
    }

    /// Build an `UnsupportedFeature` error for an AST node we recognized
    /// the parent enum of but whose specific variant the lowerer doesn't
    /// know about (e.g. a future full-moon AST variant we haven't taught
    /// the compiler to handle).  Includes the rendered source text and a
    /// help message pointing the user at filing an issue.
    fn unsupported_node<N>(&self, node: &N, kind: &'static str) -> CompileError
    where
        N: full_moon::node::Node + std::fmt::Display,
    {
        let pos = full_moon::node::Node::start_position(node)
            .unwrap_or_else(full_moon::tokenizer::Position::default);
        CompileError::UnsupportedFeature {
            location: self.loc(pos),
            feature: format!("{kind} {}", node.to_string().trim()),
            help: Some(
                "The syntax parses but is not supported by this version \
                 of shingetsu, please file an issue"
                    .to_string(),
            ),
        }
    }

    /// Apply a single suffix from a prefix-expression chain to the value in
    /// `src`, writing the result to `dst`.  Handles all four suffix forms
    /// (index `.name` / `[exp]`, anonymous call, method call) so that
    /// arbitrarily chained `f().x`, `f()[i]`, `f()()`, `f():m()` work.
    /// Middle-of-chain calls truncate to a single return value — per
    /// Lua semantics, only the last expression in a list expands.
    fn apply_index_suffix<'b>(
        &'b mut self,
        suffix: &'b ast::Suffix,
        src: u8,
        dst: u8,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), CompileError>> + Send + 'b>>
    {
        Box::pin(async move {
            match suffix {
                ast::Suffix::Index(ast::Index::Dot { name, .. }) => {
                    let key = tok_str(name);
                    let idx = self.cg.constant(key);
                    let k = self.alloc_temp()?;
                    self.cg.emit(Instruction::LoadK { dst: k, idx });
                    self.cg.emit(Instruction::GetTable {
                        dst,
                        table: src,
                        key: k,
                    });
                    self.free_temp();
                }
                ast::Suffix::Index(ast::Index::Brackets { expression, .. }) => {
                    let k = self.alloc_temp()?;
                    self.compile_expr(expression, k).await?;
                    self.cg.emit(Instruction::GetTable {
                        dst,
                        table: src,
                        key: k,
                    });
                    self.free_temp();
                }
                ast::Suffix::Call(ast::Call::AnonymousCall(args)) => {
                    // f(args) in the middle of a chain.  Put the function at
                    // `dst`, args at dst+1.., emit Call with nresults=1.
                    // `compile_args_and_call` bumps `temp_top` (which doubles as
                    // the temp-register allocator) to reserve arg slots and to
                    // guard sub-expression temps — save and restore around it
                    // so subsequent `alloc_temp` calls in the chain get the
                    // correct next slot.
                    if src != dst {
                        self.cg.emit(Instruction::Move { dst, src });
                    }
                    let saved = self.temp_top;
                    // Mid-chain call: the `.` token is on the previous suffix;
                    // not tracked here yet (end-of-chain calls cover the common case).
                    self.compile_args_and_call(args, dst, 1, 0, 1, None, None, None)
                        .await?;
                    self.temp_top = saved;
                }
                ast::Suffix::Call(ast::Call::MethodCall(mc)) => {
                    // obj:m(args) in the middle of a chain.  Place the
                    // receiver at `dst` (which doubles as the self slot for
                    // Invoke), with explicit args at dst+1.., and emit
                    // Invoke with the method-name constant.
                    let saved = self.temp_top;
                    if src != dst {
                        self.cg.emit(Instruction::Move { dst, src });
                    }
                    let method_name = tok_str(mc.name());
                    let kidx = self.cg.constant(method_name);
                    self.compile_args_and_call(
                        mc.args(),
                        dst,
                        1,
                        1,
                        1,
                        Some(kidx),
                        Some(mc.colon_token()),
                        None,
                    )
                    .await?;
                    self.temp_top = saved;
                }
                ast::Suffix::TypeInstantiation(_) => {
                    // Luau generic instantiation (e.g. func<<T>>(args)).
                    // The TypeInstantiation holds only the type parameters;
                    // these have no runtime representation so we ignore them.
                    // However, apply_index_suffix is sometimes called with
                    // src != dst (e.g. the last suffix in a chain copies the
                    // result into a different register), so we must still
                    // propagate the value.
                    if src != dst {
                        self.cg.emit(Instruction::Move { dst, src });
                    }
                }
                _ => return Err(self.unsupported_pos0("unknown suffix form")),
            }
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------------

    fn compile_block<'b>(
        &'b mut self,
        block: &'b ast::Block,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), CompileError>> + Send + 'b>>
    {
        Box::pin(async move {
            self.scope.push_scope();
            // Slot at scope entry: any local declared inside this block
            // lands at `entry_slot` or higher.  Used to emit
            // `CloseUpvalues` when the block falls off the end so that
            // closures captured during the block don't retain Open
            // cells pointing at registers that may be reused after the
            // scope exits.
            let entry_slot = self.scope.current_slot();
            for stmt in block.stmts() {
                self.compile_stmt(stmt).await?;
            }
            if let Some(last) = block.last_stmt() {
                self.compile_last_stmt(last).await?;
            }
            // Emit CloseVar for <close> vars unless the block already exited
            // unconditionally (in which case those exits already handled it).
            if !self.already_unconditionally_exited() {
                self.emit_close_for_scope();
                // Close any open upvalues for locals declared inside
                // this scope.  Required for correctness whenever a
                // closure created in this scope captured a body local
                // (Lua 5.4 §3.3.5).  Cheap when there are no captures.
                if self.scope.current_slot() > entry_slot {
                    self.cg
                        .emit(Instruction::CloseUpvalues { from: entry_slot });
                }
            }
            self.pop_scope_with_debug();
            Ok(())
        })
    }

    fn compile_stmt<'b>(
        &'b mut self,
        stmt: &'b ast::Stmt,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), CompileError>> + Send + 'b>>
    {
        Box::pin(async move {
            if self.already_unconditionally_exited() {
                if let Some(pos) = full_moon::node::Node::start_position(stmt) {
                    self.diagnostics.push(Diagnostic {
                        lint: LintId::UnreachableCode,
                        severity: crate::error::Severity::Warning,
                        location: CSourceLocation::from_pos(&self.opts().source_name, pos),
                        message: "unreachable code".to_string(),
                        help: None,
                    });
                }
            }
            self.set_node_loc(stmt);
            match stmt {
                ast::Stmt::LocalAssignment(la) => self.compile_local_assignment(la).await,
                ast::Stmt::Assignment(a) => self.compile_assignment(a).await,
                ast::Stmt::Do(d) => self.compile_do(d).await,
                ast::Stmt::While(w) => self.compile_while(w).await,
                ast::Stmt::Repeat(r) => self.compile_repeat(r).await,
                ast::Stmt::If(i) => self.compile_if(i).await,
                ast::Stmt::NumericFor(nf) => self.compile_numeric_for(nf).await,
                ast::Stmt::FunctionCall(fc) => self.compile_call_stmt(fc).await,
                ast::Stmt::LocalFunction(lf) => self.compile_local_function(lf).await,
                ast::Stmt::FunctionDeclaration(fd) => self.compile_function_decl(fd).await,
                ast::Stmt::Goto(g) => self.compile_goto(g).await,
                ast::Stmt::Label(l) => self.compile_label(l).await,
                ast::Stmt::GenericFor(gf) => self.compile_generic_for(gf).await,
                ast::Stmt::CompoundAssignment(ca) => self.compile_compound_assignment(ca).await,
                ast::Stmt::TypeDeclaration(td) => {
                    self.compile_type_declaration(td, false).await;
                    Ok(())
                }
                ast::Stmt::ExportedTypeDeclaration(etd) => {
                    self.compile_type_declaration(etd.type_declaration(), true)
                        .await;
                    Ok(())
                }
                ast::Stmt::ConstAssignment(ca) => self.compile_const_assignment(ca).await,
                ast::Stmt::ConstFunction(cf) => self.compile_const_function(cf).await,
                other => Err(self.unsupported_node(other, "statement")),
            }
        })
    }

    async fn compile_last_stmt(&mut self, stmt: &ast::LastStmt) -> Result<(), CompileError> {
        if self.already_unconditionally_exited() {
            if let Some(pos) = full_moon::node::Node::start_position(stmt) {
                self.diagnostics.push(Diagnostic {
                    lint: LintId::UnreachableCode,
                    severity: crate::error::Severity::Warning,
                    location: CSourceLocation::from_pos(&self.opts().source_name, pos),
                    message: "unreachable code".to_string(),
                    help: None,
                });
            }
        }
        self.set_node_loc(stmt);
        match stmt {
            ast::LastStmt::Return(r) => self.compile_return(r).await,
            ast::LastStmt::Break(b) => match self.break_stacks.last() {
                None => Err(CompileError::Semantic {
                    location: self.loc(b.start_position()),
                    message: "break outside loop".to_string(),
                    help: None,
                }),
                Some(info) => {
                    let loop_depth = info.scope_depth;
                    let close_from = info.close_upvalues_from;
                    self.emit_close_for_exit(loop_depth);
                    if let Some(slot) = close_from {
                        self.cg.emit(Instruction::CloseUpvalues { from: slot });
                    }
                    let jump_idx = self.cg.emit_jump();
                    self.break_stacks
                        .last_mut()
                        .expect("break_stacks non-empty")
                        .patch_list
                        .push(jump_idx);
                    self.exited = true;
                    Ok(())
                }
            },
            ast::LastStmt::Continue(c) => match self.break_stacks.last() {
                None => Err(CompileError::Semantic {
                    location: self.loc(c.start_position()),
                    message: "continue outside loop".to_string(),
                    help: None,
                }),
                Some(info) => {
                    let loop_depth = info.scope_depth;
                    let close_from = info.close_upvalues_from;
                    self.emit_close_for_exit(loop_depth);
                    if let Some(slot) = close_from {
                        self.cg.emit(Instruction::CloseUpvalues { from: slot });
                    }
                    let jump_idx = self.cg.emit_jump();
                    self.break_stacks
                        .last_mut()
                        .expect("break_stacks non-empty")
                        .continue_patch_list
                        .push(jump_idx);
                    self.exited = true;
                    Ok(())
                }
            },
            other => Err(self.unsupported_node(other, "last statement")),
        }
    }

    // -----------------------------------------------------------------------
    // Local assignment
    // -----------------------------------------------------------------------

    async fn compile_local_assignment(
        &mut self,
        la: &ast::LocalAssignment,
    ) -> Result<(), CompileError> {
        // Resolve per-name `<const>` / `<close>` attributes from the AST.
        let mut attrs: Vec<LocalAttr> = Vec::new();
        for attr in la.attributes() {
            attrs.push(match attr {
                Some(a) => {
                    let attr_name = tok_str(a.name());
                    match attr_name.as_ref() {
                        b"const" => LocalAttr::Const,
                        b"close" => LocalAttr::Close,
                        _ => {
                            return Err(CompileError::Semantic {
                                location: CSourceLocation::from_pos(
                                    &self.opts().source_name,
                                    a.name().start_position(),
                                ),
                                message: format!("unknown attribute '{attr_name}'"),
                                help: None,
                            });
                        }
                    }
                }
                None => LocalAttr::None,
            });
        }
        let names: Vec<_> = la.names().iter().collect();
        let exprs: Vec<_> = la.expressions().iter().collect();
        let type_specs: Vec<_> = la.type_specifiers().collect();
        self.compile_local_assignment_core(&names, &exprs, &type_specs, &attrs)
            .await
    }

    /// Lower a Luau `const x = 1` declaration.  Same shape as a
    /// `LocalAssignment` but every binding is implicitly `<const>`, and the
    /// AST node has no per-name attribute slots.
    async fn compile_const_assignment(
        &mut self,
        ca: &full_moon::ast::luau::ConstAssignment,
    ) -> Result<(), CompileError> {
        let names: Vec<_> = ca.names().iter().collect();
        let exprs: Vec<_> = ca.expressions().iter().collect();
        let type_specs: Vec<_> = ca.type_specifiers().collect();
        let attrs = vec![LocalAttr::Const; names.len()];
        self.compile_local_assignment_core(&names, &exprs, &type_specs, &attrs)
            .await
    }

    async fn compile_local_assignment_core(
        &mut self,
        names: &[&TokenReference],
        exprs: &[&ast::Expression],
        type_specs: &[Option<&full_moon::ast::luau::TypeSpecifier>],
        attrs: &[LocalAttr],
    ) -> Result<(), CompileError> {
        let n_names = names.len();

        // Evaluate RHS expressions into temporaries.
        //
        // Standard Lua adjustment rule: all expressions except the last are
        // adjusted to exactly 1 value.  The *last* expression may expand to
        // fill all remaining name slots if it is a function call.
        let mut rhs_regs: Vec<u8> = Vec::new();
        let mut n_temps: usize = 0;

        // Non-last expressions: always 1 result each.
        let non_last_count = exprs.len().saturating_sub(1);
        for expr in &exprs[..non_last_count] {
            let tmp = self.alloc_temp()?;
            self.compile_expr(expr, tmp).await?;
            rhs_regs.push(tmp);
            n_temps += 1;
        }

        // Last expression: may expand if it is a function call.
        if let Some(last_expr) = exprs.last() {
            let remaining = n_names.saturating_sub(rhs_regs.len());
            let nresults = remaining.max(1) as i32;
            let base = self.alloc_temp()?;
            n_temps += 1;

            if nresults > 1 {
                if let ast::Expression::FunctionCall(fc) = last_expr {
                    self.compile_function_call(fc, base, nresults).await?;
                    // The call wrote `nresults` values into base, base+1, …
                    for i in 0..nresults as u8 {
                        rhs_regs.push(base + i);
                    }
                } else if is_vararg_expr(last_expr) {
                    // Expand varargs to fill the remaining slots.
                    self.cg.emit(Instruction::Vararg {
                        dst: base,
                        nresults: nresults as i16,
                    });
                    for i in 0..nresults as u8 {
                        rhs_regs.push(base + i);
                    }
                } else {
                    // Non-call, non-vararg last expression: only 1 value.
                    self.compile_expr(last_expr, base).await?;
                    rhs_regs.push(base);
                }
            } else {
                self.compile_expr(last_expr, base).await?;
                rhs_regs.push(base);
            }
        }

        // Declare local variables and move values in.
        for (i, name_tok) in names.iter().enumerate() {
            let attr = attrs.get(i).copied().unwrap_or(LocalAttr::None);

            let name = tok_str(name_tok);

            // Warn if this shadows a variable already declared in the same scope.
            if !name.starts_with(b"_") {
                if let Some(_) = self.scope.same_scope_lookup(&name) {
                    self.diagnostics.push(Diagnostic {
                        lint: LintId::Shadowing,
                        severity: crate::error::Severity::Warning,
                        location: CSourceLocation::from_pos(
                            &self.opts().source_name,
                            name_tok.start_position(),
                        ),
                        message: format!(
                            "variable '{name}' shadows earlier declaration in same scope"
                        ),
                        help: None,
                    });
                }
            }

            let pc = self.cg.pc();
            let slot =
                self.scope
                    .declare(name, attr, pc)
                    .map_err(|msg| CompileError::Semantic {
                        location: CSourceLocation::from_pos(
                            &self.opts().source_name,
                            name_tok.start_position(),
                        ),
                        message: format!("{msg}; consider refactoring into smaller functions"),
                        help: None,
                    })?;
            self.scope.set_last_decl_location(CSourceLocation::from_pos(
                &self.opts().source_name,
                name_tok.start_position(),
            ));

            // Set inferred type from type annotation if present.
            if let Some(Some(ts)) = type_specs.get(i) {
                let lua_type = crate::type_convert::convert_type_specifier_ctx(
                    ts,
                    &crate::type_convert::TypeContext::with_aliases(&[], &self.type_aliases),
                );
                self.scope.set_last_decl_type(lua_type);
            } else if let Some(expr) = exprs.get(i) {
                // Infer type from the RHS when it's a simple global reference.
                if let ast::Expression::Var(ast::Var::Name(tok)) = expr {
                    let rhs_name = tok_str(tok);
                    if self.scope.resolve(&rhs_name).is_none() {
                        if let Some(ty) = self.compiler.global_types.get(&rhs_name) {
                            self.scope.set_last_decl_type(ty.clone());
                        }
                    }
                } else if let Some(mod_name) = Self::extract_require_literal(expr) {
                    // `local M = require("foo")` — import module type info.
                    if let Some(info) = self.resolve_require_type(&mod_name).await {
                        // Import exported types as type aliases.
                        for (type_name, alias) in &info.exported_types {
                            self.type_aliases.insert(type_name.clone(), alias.clone());
                        }
                        // Set the local's type from the module's return type.
                        if let Some(ret_ty) = &info.return_type {
                            self.scope.set_last_decl_type(ret_ty.clone());
                        }
                    }
                } else if matches!(expr, ast::Expression::TableConstructor(_)) {
                    // `local mod = {}` — seed an empty table type that
                    // `function mod.f()` declarations will accumulate into.
                    self.scope
                        .set_last_decl_type(shingetsu_vm::types::LuaType::Table(Box::new(
                            shingetsu_vm::types::TableLuaType {
                                fields: vec![],
                                indexer: None,
                            },
                        )));
                }
            }

            if let Some(&rhs) = rhs_regs.get(i) {
                if rhs != slot {
                    self.cg.emit(Instruction::Move {
                        dst: slot,
                        src: rhs,
                    });
                }
            } else {
                self.cg.emit(Instruction::LoadNil { dst: slot });
            }

            // Record a LocalDesc for <close> locals so the VM can find
            // them when unwinding errors through pcall.
            if attr == LocalAttr::Close {
                let name_bytes = tok_str(name_tok);
                self.close_local_descs.push(LocalDesc {
                    name: name_bytes,
                    attr: LocalAttr::Close,
                    slot,
                    // start_pc is the PC right after the init instruction.
                    start_pc: self.cg.pc(),
                    // end_pc is set conservatively to usize::MAX; the VM uses
                    // a nil-check to avoid double-closing.
                    end_pc: usize::MAX,
                    decl_location: None,
                    is_implicit_self: false,
                });
            }
        }

        // Release the temporaries we explicitly allocated.
        for _ in 0..n_temps {
            self.free_temp();
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Assignment to existing variables / table fields
    // -----------------------------------------------------------------------

    async fn compile_assignment(&mut self, a: &ast::Assignment) -> Result<(), CompileError> {
        let vars: Vec<_> = a.variables().iter().collect();
        let exprs: Vec<_> = a.expressions().iter().collect();
        let n_vars = vars.len();

        // Evaluate RHS into temporaries.
        //
        // Lua adjustment rule: all expressions except the last are adjusted
        // to exactly 1 value.  The *last* expression may expand to fill all
        // remaining target slots if it is a function call or `...`.  This
        // mirrors the logic in `compile_local_assignment`.
        let mut rhs_regs: Vec<u8> = Vec::new();
        let mut n_temps: usize = 0;

        let non_last_count = exprs.len().saturating_sub(1);
        for expr in &exprs[..non_last_count] {
            let tmp = self.alloc_temp()?;
            self.compile_expr(expr, tmp).await?;
            rhs_regs.push(tmp);
            n_temps += 1;
        }

        if let Some(last_expr) = exprs.last() {
            let remaining = n_vars.saturating_sub(rhs_regs.len());
            let nresults = remaining.max(1) as i32;
            let base = self.alloc_temp()?;
            n_temps += 1;

            if nresults > 1 {
                if let ast::Expression::FunctionCall(fc) = last_expr {
                    self.compile_function_call(fc, base, nresults).await?;
                    for i in 0..nresults as u8 {
                        rhs_regs.push(base + i);
                    }
                    // The call wrote values into base+1..base+nresults-1 in
                    // addition to `base`; reserve those as live temps so the
                    // LHS loop's `alloc_temp` (used by Var::Expression
                    // targets) doesn't clobber them before we consume them.
                    let extra = nresults as usize - 1;
                    self.temp_top += extra as u8;
                    n_temps += extra;
                } else if is_vararg_expr(last_expr) {
                    self.cg.emit(Instruction::Vararg {
                        dst: base,
                        nresults: nresults as i16,
                    });
                    for i in 0..nresults as u8 {
                        rhs_regs.push(base + i);
                    }
                    let extra = nresults as usize - 1;
                    self.temp_top += extra as u8;
                    n_temps += extra;
                } else {
                    // Non-call, non-vararg last expression: only 1 value.
                    self.compile_expr(last_expr, base).await?;
                    rhs_regs.push(base);
                }
            } else {
                self.compile_expr(last_expr, base).await?;
                rhs_regs.push(base);
            }
        }

        for (i, var) in vars.iter().enumerate() {
            let src = rhs_regs.get(i).copied();
            match var {
                ast::Var::Name(tok) => {
                    let name = tok_str(tok);
                    if let Some(local) = self.scope.resolve_mut(&name) {
                        if local.attr == LocalAttr::Const {
                            return Err(self.const_assign_error(&name, tok.start_position()));
                        }
                        local.write_count += 1;
                        local.last_write_location = Some(CSourceLocation::from_pos(
                            &self.compiler.opts.source_name,
                            tok.start_position(),
                        ));
                        let slot = local.slot;
                        if let Some(src_reg) = src {
                            self.cg.emit(Instruction::Move {
                                dst: slot,
                                src: src_reg,
                            });
                        } else {
                            self.cg.emit(Instruction::LoadNil { dst: slot });
                        }
                    } else if let Some(upval_idx) = self.resolve_upvalue(&name) {
                        if self.ancestor_local_attr(&name) == Some(LocalAttr::Const) {
                            return Err(self.const_assign_error(&name, tok.start_position()));
                        }
                        // Upvalue assignment.
                        let src_reg = if let Some(r) = src {
                            r
                        } else {
                            let tmp = self.alloc_temp()?;
                            self.cg.emit(Instruction::LoadNil { dst: tmp });
                            tmp
                        };
                        self.cg.emit(Instruction::SetUpval {
                            upval: upval_idx,
                            src: src_reg,
                        });
                        if src.is_none() {
                            self.free_temp();
                        }
                    } else {
                        // Global assignment.
                        let name_idx = self.cg.name(name);
                        let src_reg = if let Some(r) = src {
                            r
                        } else {
                            let tmp = self.alloc_temp()?;
                            self.cg.emit(Instruction::LoadNil { dst: tmp });
                            tmp
                        };
                        self.emit_global_write(name_idx, src_reg)?;
                        if src.is_none() {
                            self.free_temp();
                        }
                    }
                }
                ast::Var::Expression(ve) => {
                    let suffixes: Vec<_> = ve.suffixes().collect();
                    match suffixes.last() {
                        Some(ast::Suffix::Index(idx)) => {
                            let obj = self.alloc_temp()?;
                            self.compile_prefix_expr(ve.prefix(), obj).await?;
                            for s in &suffixes[..suffixes.len() - 1] {
                                self.apply_index_suffix(s, obj, obj).await?;
                            }
                            let key = self.alloc_temp()?;
                            match idx {
                                ast::Index::Dot { name, .. } => {
                                    let kb = tok_str(name);
                                    let kidx = self.cg.constant(kb);
                                    self.cg.emit(Instruction::LoadK {
                                        dst: key,
                                        idx: kidx,
                                    });
                                }
                                ast::Index::Brackets { expression, .. } => {
                                    self.compile_expr(expression, key).await?;
                                }
                                _ => return Err(self.unsupported_pos0("unknown index form")),
                            }
                            let val = self.alloc_temp()?;
                            if let Some(src_reg) = src {
                                self.cg.emit(Instruction::Move {
                                    dst: val,
                                    src: src_reg,
                                });
                            } else {
                                self.cg.emit(Instruction::LoadNil { dst: val });
                            }
                            self.cg.emit(Instruction::SetTable {
                                table: obj,
                                key,
                                src: val,
                            });
                            self.free_temp(); // val
                            self.free_temp(); // key
                            self.free_temp(); // obj
                        }
                        _ => return Err(self.unsupported_pos0("complex assignment target")),
                    }
                }
                other => return Err(self.unsupported_node(other, "assignment target")),
            }
        }

        // Track `package.path` mutations for compile-time require resolution.
        for (i, var) in vars.iter().enumerate() {
            if Self::is_package_path_target(var) {
                if let Some(rhs_expr) = exprs.get(i) {
                    if let Some(new_path) = self.try_eval_static_string(rhs_expr) {
                        self.effective_package_path = Some(new_path);
                    }
                }
            }
        }

        for _ in 0..n_temps {
            self.free_temp();
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Compound assignment  (LuaU:  x += y,  x -= y,  x ..= y, …)
    // -----------------------------------------------------------------------

    async fn compile_compound_assignment(
        &mut self,
        ca: &ast::CompoundAssignment,
    ) -> Result<(), CompileError> {
        use ast::CompoundOp;

        // Step 1 — read the current LHS value into `cur`.
        //
        // For table fields we also keep the object and key registers live so
        // we can write back without re-evaluating the table expression.
        let cur = self.alloc_temp()?; // holds the current LHS value

        enum WriteBack {
            Local(u8),
            Upvalue(u8),
            Global(u16),
            Table { obj: u8, key: u8 },
        }
        let writeback: WriteBack;

        #[allow(clippy::enum_variant_names)]
        match ca.lhs() {
            ast::Var::Name(tok) => {
                let name = tok_str(tok);
                if let Some(local) = self.scope.resolve_mut(&name) {
                    if local.attr == LocalAttr::Const {
                        return Err(self.const_assign_error(&name, tok.start_position()));
                    }
                    local.read_count += 1;
                    local.write_count += 1;
                    local.last_write_location = Some(CSourceLocation::from_pos(
                        &self.compiler.opts.source_name,
                        tok.start_position(),
                    ));
                    let slot = local.slot;
                    self.cg.emit(Instruction::Move {
                        dst: cur,
                        src: slot,
                    });
                    writeback = WriteBack::Local(slot);
                } else if let Some(upval_idx) = self.resolve_upvalue(&name) {
                    if self.ancestor_local_attr(&name) == Some(LocalAttr::Const) {
                        return Err(self.const_assign_error(&name, tok.start_position()));
                    }
                    self.cg.emit(Instruction::GetUpval {
                        dst: cur,
                        upval: upval_idx,
                    });
                    writeback = WriteBack::Upvalue(upval_idx);
                } else {
                    let name_idx = self.cg.name(name);
                    self.emit_global_read(name_idx, cur)?;
                    writeback = WriteBack::Global(name_idx);
                }
            }
            ast::Var::Expression(ve) => {
                let obj = self.alloc_temp()?;
                self.compile_prefix_expr(ve.prefix(), obj).await?;
                let suffixes: Vec<_> = ve.suffixes().collect();
                for s in &suffixes[..suffixes.len().saturating_sub(1)] {
                    self.apply_index_suffix(s, obj, obj).await?;
                }
                let key = self.alloc_temp()?;
                match suffixes.last() {
                    Some(ast::Suffix::Index(ast::Index::Dot { name, .. })) => {
                        let kb = tok_str(name);
                        let kidx = self.cg.constant(kb);
                        self.cg.emit(Instruction::LoadK {
                            dst: key,
                            idx: kidx,
                        });
                    }
                    Some(ast::Suffix::Index(ast::Index::Brackets { expression, .. })) => {
                        self.compile_expr(expression, key).await?;
                    }
                    _ => {
                        return Err(self.unsupported_pos0("compound assignment on non-index target"))
                    }
                }
                self.cg.emit(Instruction::GetTable {
                    dst: cur,
                    table: obj,
                    key,
                });
                writeback = WriteBack::Table { obj, key };
            }
            _ => return Err(self.unsupported_pos0("compound assignment: unknown lhs form")),
        }

        // Step 2 — evaluate RHS into `rhs`.
        let rhs = self.alloc_temp()?;
        self.compile_expr(ca.rhs(), rhs).await?;

        // Step 3 — apply the compound operator; result goes to `cur`.
        let instr = match ca.compound_operator() {
            CompoundOp::PlusEqual(_) => Instruction::Add {
                dst: cur,
                lhs: cur,
                rhs,
            },
            CompoundOp::MinusEqual(_) => Instruction::Sub {
                dst: cur,
                lhs: cur,
                rhs,
            },
            CompoundOp::StarEqual(_) => Instruction::Mul {
                dst: cur,
                lhs: cur,
                rhs,
            },
            CompoundOp::SlashEqual(_) => Instruction::Div {
                dst: cur,
                lhs: cur,
                rhs,
            },
            CompoundOp::CaretEqual(_) => Instruction::Pow {
                dst: cur,
                lhs: cur,
                rhs,
            },
            CompoundOp::DoubleSlashEqual(_) => Instruction::IDiv {
                dst: cur,
                lhs: cur,
                rhs,
            },
            CompoundOp::PercentEqual(_) => Instruction::Mod {
                dst: cur,
                lhs: cur,
                rhs,
            },
            CompoundOp::TwoDotsEqual(_) => {
                // Reuse the Concat instruction with count=2.
                self.free_temp(); // rhs
                self.free_temp(); // cur
                                  // Re-allocate contiguously: base=cur, base+1=rhs.
                let base = self.alloc_temp()?;
                // cur already holds the LHS value, but we freed it.
                // We need a second slot for rhs.
                let rhs2 = self.alloc_temp()?;
                // Move the LHS current value into base, then re-evaluate RHS.
                // (cur was at the same slot as base since we freed/alloc in order)
                // Actually, we can't move cur into base because we freed cur.
                // Better: read the LHS into base fresh, then eval RHS into rhs2.
                match &writeback {
                    WriteBack::Local(slot) => {
                        self.cg.emit(Instruction::Move {
                            dst: base,
                            src: *slot,
                        });
                    }
                    WriteBack::Upvalue(idx) => {
                        self.cg.emit(Instruction::GetUpval {
                            dst: base,
                            upval: *idx,
                        });
                    }
                    WriteBack::Global(idx) => {
                        self.emit_global_read(*idx, base)?;
                    }
                    WriteBack::Table { obj, key } => {
                        self.cg.emit(Instruction::GetTable {
                            dst: base,
                            table: *obj,
                            key: *key,
                        });
                    }
                }
                self.compile_expr(ca.rhs(), rhs2).await?;
                self.cg.emit(Instruction::Concat {
                    dst: base,
                    base,
                    count: 2,
                });
                // Write back base to writeback target.
                self.free_temp(); // rhs2
                match writeback {
                    WriteBack::Local(slot) => {
                        if base != slot {
                            self.cg.emit(Instruction::Move {
                                dst: slot,
                                src: base,
                            });
                        }
                    }
                    WriteBack::Upvalue(idx) => {
                        self.cg.emit(Instruction::SetUpval {
                            upval: idx,
                            src: base,
                        });
                    }
                    WriteBack::Global(idx) => {
                        self.emit_global_write(idx, base)?;
                    }
                    WriteBack::Table { obj, key } => {
                        self.cg.emit(Instruction::SetTable {
                            table: obj,
                            key,
                            src: base,
                        });
                        self.free_temp(); // key
                        self.free_temp(); // obj
                    }
                }
                self.free_temp(); // base

                // Track `package.path ..= "suffix"` for require resolution.
                if Self::is_package_path_target(ca.lhs()) {
                    if let Some(suffix) = self.try_eval_static_string(ca.rhs()) {
                        if let Some(ref mut path) = self.effective_package_path {
                            path.push_str(&suffix);
                        }
                    }
                }

                return Ok(());
            }
            _ => return Err(self.unsupported_pos0("unsupported compound operator")),
        };
        self.cg.emit(instr);
        self.free_temp(); // rhs

        // Step 4 — write `cur` back to the LHS.
        match writeback {
            WriteBack::Local(slot) => {
                if cur != slot {
                    self.cg.emit(Instruction::Move {
                        dst: slot,
                        src: cur,
                    });
                }
            }
            WriteBack::Upvalue(idx) => {
                self.cg.emit(Instruction::SetUpval {
                    upval: idx,
                    src: cur,
                });
            }
            WriteBack::Global(idx) => {
                self.emit_global_write(idx, cur)?;
            }
            WriteBack::Table { obj, key } => {
                self.cg.emit(Instruction::SetTable {
                    table: obj,
                    key,
                    src: cur,
                });
                self.free_temp(); // key
                self.free_temp(); // obj
            }
        }
        self.free_temp(); // cur
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Type declarations
    // -----------------------------------------------------------------------

    /// Process a `type Name<...> = ...` declaration.
    /// Stores the alias in `self.type_aliases` for later reference by
    /// type annotation conversion.  Produces no runtime code.
    async fn compile_type_declaration(
        &mut self,
        td: &full_moon::ast::luau::TypeDeclaration,
        exported: bool,
    ) {
        let name = Bytes::from(tok_str(td.type_name()));
        let generic_params = td
            .generics()
            .map(crate::type_convert::convert_generic_declaration)
            .unwrap_or_default();
        // Build a context from the alias's own generic params so that
        // `type Pair<A, B> = { first: A, second: B }` produces TypeParam
        // for A and B inside the body.
        let ctx =
            crate::type_convert::TypeContext::with_aliases(&generic_params, &self.type_aliases);
        let body = crate::type_convert::convert_type_info_ctx(td.type_definition(), &ctx);
        self.type_aliases.insert(
            name,
            TypeAlias {
                params: generic_params,
                body,
                exported,
            },
        );
    }

    // -----------------------------------------------------------------------
    // Control flow
    // -----------------------------------------------------------------------

    async fn compile_do(&mut self, d: &ast::Do) -> Result<(), CompileError> {
        self.compile_block(d.block()).await
    }

    async fn compile_while(&mut self, w: &ast::While) -> Result<(), CompileError> {
        if let Some(pos) = full_moon::node::Node::start_position(w) {
            self.warn_empty_loop_body(w.block(), pos);
        }

        let cond_pc = self.cg.pc();
        let tmp = self.alloc_temp()?;
        self.compile_expr(w.condition(), tmp).await?;
        let exit_jump = self.cg.emit_branch_false(tmp);
        self.free_temp();

        // Close upvalues for any local declared inside the body when
        // `break`/`continue` exits.  Without this, a closure that
        // captured a body local on the breaking iteration retains an
        // Open cell pointing at a register that may be reused after
        // the loop exits, causing stale reads.
        let close_from = self.scope.current_slot();
        self.break_stacks.push(BreakInfo {
            patch_list: Vec::new(),
            continue_patch_list: Vec::new(),
            scope_depth: self.scope.scope_depth(),
            close_upvalues_from: Some(close_from),
        });
        self.compile_block(w.block()).await?;
        let break_info = self.break_stacks.pop().expect("break stack non-empty");

        // Close upvalues captured during this iteration before looping
        // back so the next iteration's locals get fresh upvalue
        // identity (Lua 5.4 §3.3.5 also applies to `while` body locals
        // captured by closures — each iteration is a fresh scope).
        self.cg
            .emit(Instruction::CloseUpvalues { from: close_from });
        let back_jump = self.cg.emit_jump();
        self.cg.patch(back_jump, cond_pc);

        let exit_pc = self.cg.pc();
        self.cg.patch(exit_jump, exit_pc);
        for jump_idx in break_info.patch_list {
            self.cg.patch(jump_idx, exit_pc);
        }
        // `continue` in a while loop re-evaluates the condition.
        for jump_idx in break_info.continue_patch_list {
            self.cg.patch(jump_idx, cond_pc);
        }
        self.exited = false;
        Ok(())
    }

    async fn compile_repeat(&mut self, r: &ast::Repeat) -> Result<(), CompileError> {
        if let Some(pos) = full_moon::node::Node::start_position(r) {
            self.warn_empty_loop_body(r.block(), pos);
        }

        let body_pc = self.cg.pc();

        // See `compile_while` for the rationale on `close_upvalues_from`.
        let close_from = self.scope.current_slot();
        self.break_stacks.push(BreakInfo {
            patch_list: Vec::new(),
            continue_patch_list: Vec::new(),
            scope_depth: self.scope.scope_depth(),
            close_upvalues_from: Some(close_from),
        });

        // Per Lua 5.4 §3.3.4: the scope of a local variable declared
        // inside a `repeat` body extends through the `until` condition.
        // Inline the block compilation so we can compile `until` while
        // the body's scope is still open.
        self.scope.push_scope();
        for stmt in r.block().stmts() {
            self.compile_stmt(stmt).await?;
        }
        if let Some(last) = r.block().last_stmt() {
            self.compile_last_stmt(last).await?;
        }

        let break_info = self.break_stacks.pop().expect("break stack non-empty");

        // `continue` in a repeat…until loop jumps to the condition check.
        let cond_pc = self.cg.pc();
        for jump_idx in break_info.continue_patch_list {
            self.cg.patch(jump_idx, cond_pc);
        }

        // `repeat ... until cond` loops until cond is truthy.  The body's
        // scope is still open here, so locals declared inside the body
        // resolve correctly in `cond`.
        let tmp = self.alloc_temp()?;
        self.compile_expr(r.until(), tmp).await?;
        // If cond is false, jump back to body.  Close any open
        // upvalues for the body's locals first so each iteration's
        // closures get fresh upvalue identity (Lua 5.4 §3.3.5).
        if self.scope.current_slot() > close_from {
            self.cg
                .emit(Instruction::CloseUpvalues { from: close_from });
        }
        let back_jump = self.cg.emit_branch_false(tmp);
        self.cg.patch(back_jump, body_pc);
        self.free_temp();

        // Now close the body scope (mirrors `compile_block`'s tail).
        if !self.already_unconditionally_exited() {
            self.emit_close_for_scope();
        }
        self.pop_scope_with_debug();

        let exit_pc = self.cg.pc();
        for jump_idx in break_info.patch_list {
            self.cg.patch(jump_idx, exit_pc);
        }
        self.exited = false;
        Ok(())
    }

    /// Compile a LuaU `if … then … elseif … else …` *expression* (not statement).
    /// The resulting value is written to `dst`.
    async fn compile_if_expression(
        &mut self,
        ie: &ast::luau::IfExpression,
        dst: u8,
    ) -> Result<(), CompileError> {
        let mut end_jumps: Vec<usize> = Vec::new();

        // Evaluate the initial condition.
        let tmp = self.alloc_temp()?;
        self.compile_expr(ie.condition(), tmp).await?;
        let else_jump = self.cg.emit_branch_false(tmp);
        self.free_temp();

        // "then" branch value.
        self.compile_expr(ie.if_expression(), dst).await?;
        end_jumps.push(self.cg.emit_jump());

        // `elseif` chains.
        let mut next_else_jump = else_jump;
        if let Some(elseifs) = ie.else_if_expressions() {
            for elseif in elseifs {
                let elseif_pc = self.cg.pc();
                self.cg.patch(next_else_jump, elseif_pc);

                let tmp = self.alloc_temp()?;
                self.compile_expr(elseif.condition(), tmp).await?;
                next_else_jump = self.cg.emit_branch_false(tmp);
                self.free_temp();

                self.compile_expr(elseif.expression(), dst).await?;
                end_jumps.push(self.cg.emit_jump());
            }
        }

        // `else` branch value.
        let else_pc = self.cg.pc();
        self.cg.patch(next_else_jump, else_pc);
        self.compile_expr(ie.else_expression(), dst).await?;

        // Patch all jumps to the instruction after the expression.
        let end_pc = self.cg.pc();
        for j in end_jumps {
            self.cg.patch(j, end_pc);
        }
        Ok(())
    }

    async fn compile_interpolated_string(
        &mut self,
        is: &ast::luau::InterpolatedString,
        dst: u8,
    ) -> Result<(), CompileError> {
        use full_moon::tokenizer::TokenType;

        // Build a parts list: Literal(Bytes) or Expr, skipping empty
        // literals and merging adjacent literals.
        enum Part<'a> {
            Literal(Bytes),
            Expr(&'a ast::Expression),
        }

        let mut parts: Vec<Part<'_>> = Vec::new();

        let push_literal =
            |parts: &mut Vec<Part<'_>>, token: &full_moon::tokenizer::TokenReference| {
                if let TokenType::InterpolatedString { literal, .. } = token.token().token_type() {
                    let s = literal.as_str();
                    if !s.is_empty() {
                        let bytes = unescape_string(s);
                        if let Some(Part::Literal(prev)) = parts.last_mut() {
                            let mut merged = prev.to_vec();
                            merged.extend_from_slice(&bytes);
                            *prev = Bytes::from(merged);
                        } else {
                            parts.push(Part::Literal(bytes));
                        }
                    }
                }
            };

        for seg in is.segments() {
            push_literal(&mut parts, &seg.literal);
            parts.push(Part::Expr(&seg.expression));
        }

        push_literal(&mut parts, is.last_string());

        // Degenerate cases.
        if parts.is_empty() {
            let idx = self.cg.constant(Bytes::from(""));
            self.cg.emit(Instruction::LoadK { dst, idx });
            return Ok(());
        }
        if parts.len() == 1 {
            match parts.remove(0) {
                Part::Literal(b) => {
                    let idx = self.cg.constant(b);
                    self.cg.emit(Instruction::LoadK { dst, idx });
                }
                Part::Expr(e) => {
                    self.compile_expr(e, dst).await?;
                    self.cg.emit(Instruction::ToString { dst, src: dst });
                }
            }
            return Ok(());
        }

        // Multiple parts: allocate contiguous registers and batch.
        let current_top = self.scope.current_slot() + self.temp_top;
        // Reserve space for the batch; cap at what fits in registers.
        let max_batch = (255u16.saturating_sub(current_top as u16)) as usize;
        if max_batch == 0 || (max_batch < 2 && parts.len() > max_batch) {
            return Err(CompileError::Semantic {
                location: self.node_loc(is),
                message: "string interpolation requires at least 2 free registers, \
                          but too many locals are in scope; \
                          consider refactoring into smaller functions"
                    .into(),
                help: None,
            });
        }

        let total = parts.len();
        let mut part_idx = 0;

        // For multi-batch, the first register of each batch (after the
        // first) holds the accumulated result from previous batches.
        let mut carry: Option<u8> = None;

        while part_idx < total {
            let slots_available = if carry.is_some() {
                max_batch - 1
            } else {
                max_batch
            };
            let batch_count = std::cmp::min(total - part_idx, slots_available);

            // Allocate the register window.
            let base = self.scope.current_slot() + self.temp_top;
            let mut reg = base;

            // If we have a carry from a previous batch, place it first.
            if let Some(carry_reg) = carry {
                self.alloc_temp()?;
                self.cg.emit(Instruction::Move {
                    dst: reg,
                    src: carry_reg,
                });
                reg += 1;
            }

            for _ in 0..batch_count {
                let part = &parts[part_idx];
                self.alloc_temp()?;
                match part {
                    Part::Literal(b) => {
                        let idx = self.cg.constant(b.clone());
                        self.cg.emit(Instruction::LoadK { dst: reg, idx });
                    }
                    Part::Expr(e) => {
                        self.compile_expr(e, reg).await?;
                        self.cg.emit(Instruction::ToString { dst: reg, src: reg });
                    }
                }
                reg += 1;
                part_idx += 1;
            }

            let count = (reg - base) as u8;
            let is_last_batch = part_idx >= total;

            if count == 1 && is_last_batch && carry.is_none() {
                // Single remaining part, move to dst.
                self.cg.emit(Instruction::Move { dst, src: base });
            } else {
                let concat_dst = if is_last_batch { dst } else { base };
                self.cg.emit(Instruction::Concat {
                    dst: concat_dst,
                    base,
                    count,
                });
                if !is_last_batch {
                    carry = Some(base);
                }
            }

            // Free temps for this batch.
            for _ in 0..count {
                self.free_temp();
            }
        }

        Ok(())
    }

    async fn compile_if(&mut self, stmt: &ast::If) -> Result<(), CompileError> {
        let mut end_jumps: Vec<usize> = Vec::new();

        // Condition.
        let tmp = self.alloc_temp()?;
        self.compile_expr(stmt.condition(), tmp).await?;
        let else_jump = self.cg.emit_branch_false(tmp);
        self.free_temp();

        self.exited = false;
        self.compile_block(stmt.block()).await?;
        let mut all_branches_exit = self.exited;

        // Process `elseif` chains.
        let mut next_else_jump = else_jump;
        for elseif in stmt.else_if().iter().flat_map(|e| e.iter()) {
            let end_jump = self.cg.emit_jump();
            end_jumps.push(end_jump);

            let elseif_pc = self.cg.pc();
            self.cg.patch(next_else_jump, elseif_pc);

            let tmp = self.alloc_temp()?;
            self.compile_expr(elseif.condition(), tmp).await?;
            next_else_jump = self.cg.emit_branch_false(tmp);
            self.free_temp();

            self.exited = false;
            self.compile_block(elseif.block()).await?;
            all_branches_exit = all_branches_exit && self.exited;
        }

        // `else` branch.
        let end_jump = self.cg.emit_jump();
        end_jumps.push(end_jump);

        let else_pc = self.cg.pc();
        self.cg.patch(next_else_jump, else_pc);

        let has_else = stmt.else_block().is_some();
        if let Some(else_block) = stmt.else_block() {
            self.exited = false;
            self.compile_block(else_block).await?;
            all_branches_exit = all_branches_exit && self.exited;
        }

        let end_pc = self.cg.pc();
        for j in end_jumps {
            self.cg.patch(j, end_pc);
        }

        // Code after the if is unreachable only when every branch
        // (including an explicit else) unconditionally exits.
        self.exited = has_else && all_branches_exit;
        Ok(())
    }

    async fn compile_numeric_for(&mut self, nf: &ast::NumericFor) -> Result<(), CompileError> {
        if let Some(pos) = full_moon::node::Node::start_position(nf) {
            self.warn_empty_loop_body(nf.block(), pos);
        }

        let var_name = tok_str(nf.index_variable());
        let pc = self.cg.pc();
        let loc = CSourceLocation::unknown(&self.opts().source_name);

        // Open a hidden scope for the three control registers so that locals
        // declared inside the loop body don't clobber them.
        self.scope.push_scope();
        let counter = self
            .scope
            .declare(Bytes::from("(for index)"), LocalAttr::None, pc)
            .map_err(|msg| CompileError::Semantic {
                location: loc.clone(),
                message: msg,
                help: None,
            })?;
        let limit = self
            .scope
            .declare(Bytes::from("(for limit)"), LocalAttr::None, pc)
            .map_err(|msg| CompileError::Semantic {
                location: loc.clone(),
                message: msg,
                help: None,
            })?;
        let step = self
            .scope
            .declare(Bytes::from("(for step)"), LocalAttr::None, pc)
            .map_err(|msg| CompileError::Semantic {
                location: loc.clone(),
                message: msg,
                help: None,
            })?;

        // Evaluate start, limit, step into the control registers.
        self.compile_expr(nf.start(), counter).await?;
        self.compile_expr(nf.end(), limit).await?;
        if let Some(step_expr) = nf.step() {
            self.compile_expr(step_expr, step).await?;
        } else {
            let idx = self.cg.add_constant(Value::Integer(1));
            self.cg.emit(Instruction::LoadK { dst: step, idx });
        }

        // ForPrep: check if loop should execute.
        let for_prep_idx = self.cg.emit(Instruction::ForPrep {
            base: counter,
            exit_offset: 0, // patched below
        });

        // Declare the user-visible loop variable in an inner body scope.
        let body_pc = self.cg.pc();
        self.scope.push_scope();
        let slot = self
            .scope
            .declare(var_name, LocalAttr::None, body_pc)
            .map_err(|msg| CompileError::Semantic {
                location: loc.clone(),
                message: msg,
                help: None,
            })?;
        self.scope.set_last_decl_location(CSourceLocation::from_pos(
            &self.opts().source_name,
            nf.index_variable().start_position(),
        ));
        // Copy counter into the loop variable at the top of each iteration.
        self.cg.emit(Instruction::Move {
            dst: slot,
            src: counter,
        });

        // Use scope_depth()-1 so that break/continue close <close> vars
        // declared in the for-body scope (which is already open here).
        self.break_stacks.push(BreakInfo {
            patch_list: Vec::new(),
            continue_patch_list: Vec::new(),
            scope_depth: self.scope.scope_depth() - 1,
            close_upvalues_from: Some(slot),
        });
        self.compile_block_stmts(nf.block()).await?;
        let break_info = self.break_stacks.pop().expect("break stack non-empty");

        // Close open upvalues for the loop variable so each iteration
        // gets its own upvalue identity (Lua 5.4 §3.3.5).
        self.cg.emit(Instruction::CloseUpvalues { from: slot });
        self.pop_scope_with_debug(); // body scope (loop variable)

        // ForStep: increment counter and branch back to body.
        // This is also the `continue` target.
        let for_step_idx = self.cg.emit(Instruction::ForStep {
            base: counter,
            body_offset: 0, // patched below
        });
        self.cg.patch_for_step(for_step_idx, body_pc);

        let exit_pc = self.cg.pc();
        self.cg.patch_for_prep(for_prep_idx, exit_pc);
        for jump_idx in break_info.patch_list {
            self.cg.patch(jump_idx, exit_pc);
        }
        // `continue` in a numeric for jumps to ForStep.
        for jump_idx in break_info.continue_patch_list {
            self.cg.patch(jump_idx, for_step_idx);
        }

        self.pop_scope_with_debug(); // control scope (counter/limit/step)
        self.exited = false;
        Ok(())
    }

    async fn compile_generic_for(&mut self, gf: &ast::GenericFor) -> Result<(), CompileError> {
        if let Some(pos) = full_moon::node::Node::start_position(gf) {
            self.warn_empty_loop_body(gf.block(), pos);
        }

        let pc = self.cg.pc();
        let loc = CSourceLocation::unknown(&self.opts().source_name);

        let var_name_toks: Vec<_> = gf.names().iter().collect();
        let var_names: Vec<Bytes> = var_name_toks.iter().map(|t| tok_str(t)).collect();
        let n_vars = var_names.len();

        // Hidden control scope: (for iter), (for state), (for control),
        // (for closing).  Lua 5.4 §3.3.5: the 4th variable has the
        // <close> attribute and is auto-closed when the loop exits.
        self.scope.push_scope();
        let iter = self
            .scope
            .declare(Bytes::from("(for iter)"), LocalAttr::None, pc)
            .map_err(|msg| CompileError::Semantic {
                location: loc.clone(),
                message: msg,
                help: None,
            })?;
        let _state = self
            .scope
            .declare(Bytes::from("(for state)"), LocalAttr::None, pc)
            .map_err(|msg| CompileError::Semantic {
                location: loc.clone(),
                message: msg,
                help: None,
            })?;
        let _control = self
            .scope
            .declare(Bytes::from("(for control)"), LocalAttr::None, pc)
            .map_err(|msg| CompileError::Semantic {
                location: loc.clone(),
                message: msg,
                help: None,
            })?;
        let closing = self
            .scope
            .declare(Bytes::from("(for closing)"), LocalAttr::Close, pc)
            .map_err(|msg| CompileError::Semantic {
                location: loc.clone(),
                message: msg,
                help: None,
            })?;

        // Evaluate the expression list (iterator, state, initial_control,
        // closing).  Standard adjustment rule: non-last exprs produce
        // 1 result each; the last expr may expand to fill remaining slots.
        let exprs: Vec<_> = gf.expressions().iter().collect();
        let non_last = exprs.len().saturating_sub(1);
        for (i, expr) in exprs[..non_last].iter().enumerate() {
            let dst = iter + i as u8;
            if dst <= closing {
                self.compile_expr(expr, dst).await?;
            }
        }
        if let Some(last) = exprs.last() {
            let base = iter + non_last as u8;
            let remaining = 4u8.saturating_sub(non_last as u8);
            if remaining > 1 {
                if let ast::Expression::FunctionCall(fc) = last {
                    self.compile_function_call(fc, base, remaining as i32)
                        .await?;
                } else if is_vararg_expr(last) {
                    self.cg.emit(Instruction::Vararg {
                        dst: base,
                        nresults: remaining as i16,
                    });
                } else {
                    self.compile_expr(last, base).await?;
                    // remaining-1 slots left as nil (registers init to nil)
                }
            } else if remaining == 1 {
                self.compile_expr(last, base).await?;
            }
        }

        // Record <close> local desc so the VM can find the closing
        // variable during error-path unwinding through pcall.
        self.close_local_descs.push(LocalDesc {
            name: Bytes::from("(for closing)"),
            attr: LocalAttr::Close,
            slot: closing,
            start_pc: self.cg.pc(),
            end_pc: usize::MAX,
            decl_location: None,
            is_implicit_self: false,
        });

        // Inner scope for user-visible loop variables; these are the
        // registers that GenericForCall writes its results into.
        self.scope.push_scope();
        for (i, name) in var_names.iter().enumerate() {
            let slot = self
                .scope
                .declare(name.clone(), LocalAttr::None, pc)
                .map_err(|msg| CompileError::Semantic {
                    location: loc.clone(),
                    message: msg,
                    help: None,
                })?;
            self.scope.set_last_decl_location(CSourceLocation::from_pos(
                &self.opts().source_name,
                var_name_toks[i].start_position(),
            ));
            debug_assert!(
                i > 0 || slot == iter + 4,
                "first loop var must be at iter+4"
            );
            let _ = slot;
        }

        let loop_pc = self.cg.pc();
        // Use scope_depth()-1 so that break/continue close <close> vars
        // declared in the user vars scope (which is already open here).
        self.break_stacks.push(BreakInfo {
            patch_list: Vec::new(),
            continue_patch_list: Vec::new(),
            scope_depth: self.scope.scope_depth() - 1,
            close_upvalues_from: Some(iter + 4),
        });

        self.cg.emit(Instruction::GenericForCall {
            base: iter,
            nresults: n_vars as u8,
        });
        let check_idx = self.cg.emit(Instruction::GenericForCheck {
            base: iter,
            exit_offset: 0, // patched below
        });

        self.compile_block_stmts(gf.block()).await?;
        let break_info = self.break_stacks.pop().expect("break stack non-empty");

        // Close open upvalues for the loop variables so each iteration
        // gets its own upvalue identity (Lua 5.4 §3.3.5).
        self.cg.emit(Instruction::CloseUpvalues { from: iter + 4 });

        // Jump back to the iterator call.
        let back_jump = self.cg.emit_jump();
        self.cg.patch(back_jump, loop_pc);

        let exit_pc = self.cg.pc();
        self.cg.patch(check_idx, exit_pc);
        for jump_idx in break_info.patch_list {
            self.cg.patch(jump_idx, exit_pc);
        }
        // `continue` in a generic for re-invokes the iterator.
        for jump_idx in break_info.continue_patch_list {
            self.cg.patch(jump_idx, loop_pc);
        }

        self.pop_scope_with_debug(); // user vars scope
                                     // Close the 4th hidden variable (for closing) which has <close>.
                                     // This runs on both normal loop termination and break.
        self.emit_close_for_scope();
        self.pop_scope_with_debug(); // hidden control scope
        self.exited = false;
        Ok(())
    }

    /// Compile only the statements of a block (without opening a new scope).
    fn warn_empty_loop_body(
        &mut self,
        block: &ast::Block,
        keyword_pos: full_moon::tokenizer::Position,
    ) {
        if block.stmts().next().is_none() && block.last_stmt().is_none() {
            self.diagnostics.push(Diagnostic {
                lint: LintId::EmptyLoop,
                severity: crate::error::Severity::Warning,
                location: CSourceLocation::from_pos(&self.opts().source_name, keyword_pos),
                message: "empty loop body".to_string(),
                help: None,
            });
        }
    }

    async fn compile_block_stmts(&mut self, block: &ast::Block) -> Result<(), CompileError> {
        for stmt in block.stmts() {
            self.compile_stmt(stmt).await?;
        }
        if let Some(last) = block.last_stmt() {
            self.compile_last_stmt(last).await?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Return
    // -----------------------------------------------------------------------

    async fn compile_return(&mut self, r: &ast::Return) -> Result<(), CompileError> {
        let exprs: Vec<_> = r.returns().iter().collect();

        // Evaluate all return expressions into consecutive temporaries before
        // emitting any CloseVar — the expressions may reference the <close>
        // variables themselves.
        let base = self.scope.current_slot() + self.temp_top;
        let last_idx = exprs.len().wrapping_sub(1);
        let mut count = 0i32;
        for (i, expr) in exprs.iter().enumerate() {
            let reg = base + count as u8;
            self.temp_top += 1;
            let is_last = !exprs.is_empty() && i == last_idx;
            let is_last_call = is_last && matches!(expr, ast::Expression::FunctionCall(_));
            let is_last_vararg = is_last && is_vararg_expr(expr);
            if is_last_call {
                if let ast::Expression::FunctionCall(fc) = expr {
                    self.compile_function_call(fc, reg, -1).await?;
                }
                // Close all live <close> vars, then return everything from base.
                self.emit_close_for_exit(0);
                self.cg.emit(Instruction::Return { base, nresults: -1 });
                self.temp_top -= count as u8 + 1;
                self.exited = true;
                return Ok(());
            }
            if is_last_vararg {
                self.cg.emit(Instruction::Vararg {
                    dst: reg,
                    nresults: -1,
                });
                self.emit_close_for_exit(0);
                self.cg.emit(Instruction::Return { base, nresults: -1 });
                self.temp_top -= count as u8 + 1;
                self.exited = true;
                return Ok(());
            }
            self.compile_expr(expr, reg).await?;
            count += 1;
        }
        // Close all live <close> vars before the Return instruction.
        self.emit_close_for_exit(0);
        if count == 0 {
            self.cg.emit(Instruction::Return {
                base: 0,
                nresults: 0,
            });
        } else {
            self.cg.emit(Instruction::Return {
                base,
                nresults: count as i16,
            });
            self.temp_top -= count as u8;
        }
        self.exited = true;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Goto / label
    // -----------------------------------------------------------------------

    async fn compile_goto(&mut self, g: &ast52::Goto) -> Result<(), CompileError> {
        let label_name = tok_str(g.label_name());

        // Check if the label is already defined (backward goto).
        if let Some((_, target_pc)) = self.labels.iter().find(|(n, _)| n == &label_name) {
            let target_pc = *target_pc;
            let jump_idx = self.cg.emit_jump();
            self.cg.patch(jump_idx, target_pc);
        } else {
            // Forward goto — record for patching when label is encountered.
            let jump_idx = self.cg.emit_jump();
            let depth = self.scope.scope_depth();
            self.pending_gotos.push((label_name, jump_idx, depth));
        }
        self.exited = true;
        Ok(())
    }

    async fn compile_label(&mut self, l: &ast52::Label) -> Result<(), CompileError> {
        let label_name = tok_str(l.name());
        let target_pc = self.cg.pc();
        // A label is a jump target, so code here is reachable.
        self.exited = false;

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

    async fn compile_local_function(
        &mut self,
        lf: &ast::LocalFunction,
    ) -> Result<(), CompileError> {
        self.compile_local_function_core(lf.name(), lf.body(), LocalAttr::None)
            .await
    }

    /// Lower a Luau `const function f() end` declaration.  Same shape as a
    /// `LocalFunction` but the binding is implicitly `<const>`.  Function
    /// attributes (e.g. `@native`) are accepted but ignored — they're hints
    /// for an optimizer we don't have.
    async fn compile_const_function(
        &mut self,
        cf: &full_moon::ast::luau::ConstFunction,
    ) -> Result<(), CompileError> {
        self.compile_local_function_core(cf.name(), cf.body(), LocalAttr::Const)
            .await
    }

    async fn compile_local_function_core(
        &mut self,
        name_tok: &TokenReference,
        body: &ast::FunctionBody,
        attr: LocalAttr,
    ) -> Result<(), CompileError> {
        let name = tok_str(name_tok);

        // Warn if this shadows a variable already declared in the same scope.
        if !name.starts_with(b"_") {
            if let Some(_) = self.scope.same_scope_lookup(&name) {
                self.diagnostics.push(Diagnostic {
                    lint: LintId::Shadowing,
                    severity: crate::error::Severity::Warning,
                    location: CSourceLocation::from_pos(
                        &self.opts().source_name,
                        name_tok.start_position(),
                    ),
                    message: format!("variable '{name}' shadows earlier declaration in same scope"),
                    help: None,
                });
            }
        }

        // Declare the local first (allows recursion).
        let pc = self.cg.pc();
        let slot =
            self.scope
                .declare(name.clone(), attr, pc)
                .map_err(|msg| CompileError::Semantic {
                    location: CSourceLocation::unknown(&self.opts().source_name),
                    message: msg,
                    help: None,
                })?;
        self.scope.set_last_decl_location(CSourceLocation::from_pos(
            &self.opts().source_name,
            name_tok.start_position(),
        ));
        self.scope.set_last_decl_is_function();

        let proto_idx = self.compile_function_body(name, body, false).await?;
        self.cg.emit(Instruction::NewClosure {
            dst: slot,
            proto_idx: proto_idx as u16,
        });

        // Infer the function's LuaType from its parameter and return
        // type annotations so that `return f` propagates the type as
        // the module's return_type.  Only set the type when the
        // function has at least one annotation — fully untyped
        // functions should not trigger arg-count checks.
        let type_specs: Vec<_> = body.type_specifiers().collect();
        let has_any_annotation =
            type_specs.iter().any(|ts| ts.is_some()) || body.return_type().is_some();
        if has_any_annotation {
            let type_ctx = crate::type_convert::TypeContext::with_aliases(&[], &self.type_aliases);
            let params: Vec<(Option<Bytes>, shingetsu_vm::types::LuaType)> = body
                .parameters()
                .iter()
                .enumerate()
                .filter_map(|(i, p)| match p {
                    ast::Parameter::Name(tok) => {
                        let pname = tok_str(tok);
                        let lua_type = type_specs
                            .get(i)
                            .and_then(|opt| opt.as_ref())
                            .map(|ts| {
                                crate::type_convert::convert_type_specifier_ctx(ts, &type_ctx)
                            })
                            .unwrap_or(shingetsu_vm::types::LuaType::Any);
                        Some((Some(pname), lua_type))
                    }
                    _ => None,
                })
                .collect();
            let is_method = params
                .first()
                .and_then(|(name, _)| name.as_ref())
                .map_or(false, |n| n == &b"self"[..]);
            let variadic = body
                .parameters()
                .iter()
                .any(|p| matches!(p, ast::Parameter::Ellipsis(_)));
            let returns = body
                .return_type()
                .map(|ts| crate::type_convert::convert_return_type_ctx(ts, &type_ctx))
                .unwrap_or_default();
            let func_type = shingetsu_vm::types::LuaType::Function(Box::new(
                shingetsu_vm::types::FunctionLuaType {
                    type_params: vec![],
                    params,
                    variadic: if variadic {
                        Some(Box::new(shingetsu_vm::types::LuaType::Any))
                    } else {
                        None
                    },
                    returns,
                    is_method,
                    inferred_unannotated: false,
                },
            ));
            self.scope.set_last_decl_type(func_type);
        }

        Ok(())
    }

    async fn compile_function_decl(
        &mut self,
        fd: &ast::FunctionDeclaration,
    ) -> Result<(), CompileError> {
        let func_name = fd.name();
        let names: Vec<_> = func_name.names().iter().collect();

        if names.len() == 1 && func_name.method_name().is_none() {
            // Simple: `function name(...)`
            let name = tok_str(names[0]);
            let tmp = self.alloc_temp()?;
            let proto_idx = self
                .compile_function_body(name.clone(), fd.body(), false)
                .await?;
            self.cg.emit(Instruction::NewClosure {
                dst: tmp,
                proto_idx: proto_idx as u16,
            });

            if let Some(local) = self.scope.resolve_mut(&name) {
                if local.attr == LocalAttr::Const {
                    return Err(self.const_assign_error(&name, names[0].start_position()));
                }
                local.write_count += 1;
                local.last_write_location = Some(CSourceLocation::from_pos(
                    &self.compiler.opts.source_name,
                    names[0].start_position(),
                ));
                let slot = local.slot;
                self.cg.emit(Instruction::Move {
                    dst: slot,
                    src: tmp,
                });
            } else {
                let name_idx = self.cg.name(name);
                self.emit_global_write(name_idx, tmp)?;
            }
            self.free_temp();
        } else {
            // Dotted / method: `function a.b.c(...)` or `function a:m(...)`.
            // Build a full name for the proto, then resolve the table chain and
            // assign via SetTable.
            let mut full_name_buf = tok_str(names[0]).to_vec();
            for n in &names[1..] {
                full_name_buf.push(b'.');
                full_name_buf.extend_from_slice(&tok_str(n));
            }
            if let Some(mname) = func_name.method_name() {
                full_name_buf.push(b':');
                full_name_buf.extend_from_slice(&tok_str(mname));
            }
            let full_name = Bytes::from(full_name_buf);

            let tmp = self.alloc_temp()?;
            let proto_idx = self
                .compile_function_body(full_name, fd.body(), func_name.method_name().is_some())
                .await?;
            self.cg.emit(Instruction::NewClosure {
                dst: tmp,
                proto_idx: proto_idx as u16,
            });

            // Load the root table.
            let obj = self.alloc_temp()?;
            let root = tok_str(names[0]);
            if let Some(local) = self.scope.resolve_mut(&root) {
                local.read_count += 1;
                let slot = local.slot;
                self.cg.emit(Instruction::Move {
                    dst: obj,
                    src: slot,
                });
            } else {
                let ni = self.cg.name(root.clone());
                self.emit_global_read(ni, obj)?;
            }

            // Navigate dotted chain (all names except first and last key).
            let key_names = if func_name.method_name().is_some() {
                &names[1..] // all names are table traversal; method name is the final key
            } else {
                &names[1..names.len() - 1] // traverse to parent table
            };
            for n in key_names {
                let kb = tok_str(n);
                let kidx = self.cg.constant(kb);
                let k = self.alloc_temp()?;
                self.cg.emit(Instruction::LoadK { dst: k, idx: kidx });
                self.cg.emit(Instruction::GetTable {
                    dst: obj,
                    table: obj,
                    key: k,
                });
                self.free_temp();
            }

            // Assign function to the final key.
            let final_key_bytes = if let Some(mname) = func_name.method_name() {
                tok_str(mname)
            } else {
                tok_str(names.last().expect("at least one name"))
            };
            let fidx = self.cg.constant(final_key_bytes);
            let fk = self.alloc_temp()?;
            self.cg.emit(Instruction::LoadK { dst: fk, idx: fidx });
            self.cg.emit(Instruction::SetTable {
                table: obj,
                key: fk,
                src: tmp,
            });
            self.free_temp(); // fk
            self.free_temp(); // obj
            self.free_temp(); // tmp

            // Track field definition syntax on the root local so that
            // call-site checks can detect dot-vs-colon mismatches.
            let is_method = func_name.method_name().is_some();
            let is_single_level = if is_method {
                names.len() == 1
            } else {
                names.len() == 2
            };
            if is_single_level {
                let field_name = if let Some(mname) = func_name.method_name() {
                    tok_str(mname)
                } else {
                    tok_str(names.last().expect("at least two names"))
                };
                if let Some(local) = self.scope.resolve_mut(&root) {
                    local.field_defs.insert(field_name.clone(), is_method);

                    // Accumulate function type into the local's table type.
                    let proto = &self.child_protos[proto_idx];
                    let func_type = Self::function_type_from_proto(&proto.signature, is_method);
                    match &mut local.inferred_type {
                        Some(shingetsu_vm::types::LuaType::Table(table_type)) => {
                            if let Some(existing) =
                                table_type.fields.iter_mut().find(|(n, _)| n == &field_name)
                            {
                                existing.1 = func_type;
                            } else {
                                table_type.fields.push((field_name, func_type));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }

    /// Compile a function body into a child `Proto`.  Returns the index in
    /// `self.child_protos`.
    async fn compile_function_body(
        &mut self,
        name: Bytes,
        body: &ast::FunctionBody,
        is_method: bool,
    ) -> Result<usize, CompileError> {
        // Snapshot this function's live locals for upvalue resolution in the child.
        let parent_locals: Vec<(Bytes, u8, LocalAttr)> = self
            .scope
            .all_live()
            .map(|l| (l.name.clone(), l.slot, l.attr))
            .collect();
        // Build the ancestor chain: parent's locals first, then grandparent's, …
        let mut ancestor_locals = vec![parent_locals];
        ancestor_locals.extend_from_slice(&self.ancestor_locals);

        // Share this function's upvalue descriptor list with the child so
        // that multi-level upvalue resolution can insert entries into
        // intermediate ancestor lists.
        let mut ancestor_upvalue_descs = vec![self.upvalue_descs.clone()];
        ancestor_upvalue_descs.extend(self.ancestor_upvalue_descs.iter().cloned());

        let mut child =
            FnCompiler::new_with_ancestors(self.compiler, ancestor_locals, ancestor_upvalue_descs);

        // Declare parameters as locals in the child's scope.
        let params: Vec<_> = body.parameters().iter().collect();
        let mut param_specs: Vec<ParamSpec> = Vec::new();
        let mut variadic = false;

        // Method declarations (`function t:m()`) have an implicit `self` first parameter.
        if is_method {
            child
                .scope
                .declare(Bytes::from("self"), LocalAttr::None, 0)
                .map_err(|msg| CompileError::Semantic {
                    location: CSourceLocation::unknown(&self.opts().source_name),
                    message: msg,
                    help: None,
                })?;
            child.scope.set_last_decl_implicit_self();
            child
                .scope
                .set_last_decl_location(CSourceLocation::from_pos(
                    &self.opts().source_name,
                    body.parameters_parentheses().tokens().0.start_position(),
                ));
            param_specs.push(ParamSpec {
                name: Some(Bytes::from("self")),
                runtime_type: None,
                lua_type: None,
                doc: None,
            });
        }
        // Parse generic type parameter declarations (e.g. `<T, U>`).
        let generic_type_params: Vec<shingetsu_vm::types::GenericTypeParam> = body
            .generics()
            .map(crate::type_convert::convert_generic_declaration)
            .unwrap_or_default();
        let type_ctx = crate::type_convert::TypeContext::with_aliases(
            &generic_type_params,
            &self.type_aliases,
        );

        // Collect type specifiers (LuaU annotations on parameters).
        let type_specs: Vec<_> = body.type_specifiers().collect();

        for (i, param) in params.iter().enumerate() {
            match param {
                ast::Parameter::Name(tok) => {
                    let pname = tok_str(tok);
                    let slot = child
                        .scope
                        .declare(pname.clone(), LocalAttr::None, 0)
                        .map_err(|msg| CompileError::Semantic {
                            location: CSourceLocation::unknown(&self.opts().source_name),
                            message: msg,
                            help: None,
                        })?;
                    child
                        .scope
                        .set_last_decl_location(CSourceLocation::from_pos(
                            &child.opts().source_name,
                            tok.start_position(),
                        ));
                    let lua_type = type_specs
                        .get(i)
                        .and_then(|opt| opt.as_ref())
                        .map(|ts| crate::type_convert::convert_type_specifier_ctx(ts, &type_ctx));
                    let runtime_type = lua_type
                        .as_ref()
                        .and_then(shingetsu_vm::types::derive_runtime_type);
                    param_specs.push(ParamSpec {
                        name: Some(pname),
                        runtime_type,
                        lua_type,
                        doc: None,
                    });
                    let _ = slot;
                }
                ast::Parameter::Ellipsis(_) => {
                    variadic = true;
                    child.is_variadic = true;
                }
                other => return Err(child.unsupported_node(other, "parameter")),
            }
        }

        // Compile the body block.
        child.compile_block_stmts(body.block()).await?;

        // Ensure there is always a Return at the end.  Checking the
        // literal last instruction is not enough: when an `if` ends
        // with all branches returning except for a fall-through `then`
        // branch that jumps to "after the if", the last emitted
        // instruction is a `Return` (from the else branch) but the
        // jump target is one past it.  Use the `exited` flag, which is
        // false whenever some path falls off the end of the body.
        if !child.exited {
            child.cg.emit(Instruction::Return {
                base: 0,
                nresults: 0,
            });
        }

        // Convert return type annotation if present.
        let lua_returns = body
            .return_type()
            .map(|ts| crate::type_convert::convert_return_type_ctx(ts, &type_ctx));

        // Line bounds: for a nested function, `line_defined` is the
        // line of the opening `(` of the parameter list (which in all
        // normal formatting sits on the same line as the `function`
        // keyword) and `last_line_defined` is the line of the matching
        // `end` token.  Populated unconditionally — two u32s regardless
        // of `debug_info`.
        let (line_defined, last_line_defined) = {
            let open_paren = body.parameters_parentheses().tokens().0;
            let line_defined = open_paren.start_position().line() as u32;
            let last_line_defined = body.end_token().start_position().line() as u32;
            (line_defined, last_line_defined)
        };

        // Flush any remaining scopes (including the root scope that
        // holds parameters) into debug_local_descs before building
        // the proto — mirrors what `finish()` does for the top-level chunk.
        {
            let end_pc = child.cg.instructions.len();
            while child.scope.scope_depth() > 0 {
                let locals = child.scope.pop_scope();
                for local in &locals {
                    child.check_unused_local(local);

                    if child.opts().debug_info && local.attr != LocalAttr::Close {
                        child.debug_local_descs.push(LocalDesc {
                            name: local.name.clone(),
                            attr: local.attr,
                            slot: local.slot,
                            start_pc: local.start_pc,
                            end_pc,
                            decl_location: local.decl_location.clone().map(Into::into),
                            is_implicit_self: local.is_implicit_self,
                        });
                    }
                }
            }
        }

        let num_upvalues = child.upvalue_descs.lock().len() as u8;

        let has_runtime_types = param_specs.iter().any(|p| p.runtime_type.is_some());
        let sig = Arc::new(FunctionSignature {
            name,
            source: Bytes::from(self.opts().source_name.as_bytes()),
            type_params: generic_type_params,
            params: param_specs,
            variadic,
            arg_offset: 0,
            returns: None,
            lua_returns,
            line_defined,
            last_line_defined,
            num_upvalues,
            has_runtime_types,
        });

        // Mark parent locals as read when captured as upvalues by the child.
        for desc in child.upvalue_descs.lock().iter() {
            if desc.in_stack {
                if let Some(local) = self.scope.resolve_mut(&desc.name) {
                    local.read_count += 1;
                }
            }
        }

        // Collect diagnostics from the child compiler into the parent.
        self.diagnostics.extend(child.diagnostics);

        let child_upvalues = child.upvalue_descs.lock().clone();
        let child_env_upvalue_idx = child_upvalues
            .iter()
            .position(|d| d.name == "_ENV")
            .map(|i| i as u8);
        let proto = Arc::new(encode_proto(
            sig,
            child.cg,
            {
                let mut all = child.close_local_descs;
                all.extend(child.debug_local_descs);
                all
            },
            child_upvalues,
            child_env_upvalue_idx,
            child.child_protos,
            child.type_aliases,
            child.max_stack_size.max(child.scope.max_slot as u16),
        ));

        let idx = self.child_protos.len();
        self.child_protos.push(proto);
        Ok(idx)
    }

    // -----------------------------------------------------------------------
    // Function calls (as statements)
    // -----------------------------------------------------------------------

    async fn compile_call_stmt(&mut self, fc: &ast::FunctionCall) -> Result<(), CompileError> {
        let tmp = self.alloc_temp()?;
        self.compile_function_call(fc, tmp, 0).await?;
        self.free_temp();
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    /// Compile an expression and place its result in `dst`.
    fn compile_expr<'b>(
        &'b mut self,
        expr: &'b ast::Expression,
        dst: u8,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), CompileError>> + Send + 'b>>
    {
        Box::pin(async move {
            self.note_reg_use(dst);
            match expr {
                ast::Expression::Number(tok) => {
                    self.compile_number(tok, dst).await?;
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
                        "..." => {
                            if !self.is_variadic {
                                return Err(self.unsupported(
                                    tok.start_position(),
                                    "cannot use '...' outside a variadic function",
                                ));
                            }
                            // Single-value context: take only the first vararg.
                            self.cg.emit(Instruction::Vararg { dst, nresults: 1 });
                        }
                        _ => {
                            return Err(
                                self.unsupported(tok.start_position(), "unknown symbol expression")
                            );
                        }
                    }
                }
                ast::Expression::Var(var) => {
                    self.compile_var_expr(var, dst).await?;
                }
                ast::Expression::BinaryOperator { lhs, binop, rhs } => {
                    self.compile_binop(lhs, binop, rhs, dst).await?;
                }
                ast::Expression::UnaryOperator { unop, expression } => {
                    self.compile_unop(unop, expression, dst).await?;
                }
                ast::Expression::FunctionCall(fc) => {
                    self.compile_function_call(fc, dst, 1).await?;
                }
                ast::Expression::Function(anon) => {
                    let name = Bytes::from("<anonymous>");
                    let proto_idx = self.compile_function_body(name, anon.body(), false).await?;
                    self.cg.emit(Instruction::NewClosure {
                        dst,
                        proto_idx: proto_idx as u16,
                    });
                }
                ast::Expression::Parentheses { expression, .. } => {
                    self.compile_expr(expression, dst).await?;
                }
                ast::Expression::TableConstructor(tc) => {
                    self.compile_table_constructor(tc, dst).await?;
                }
                ast::Expression::IfExpression(ie) => {
                    self.compile_if_expression(ie, dst).await?;
                }
                ast::Expression::InterpolatedString(is) => {
                    self.compile_interpolated_string(is, dst).await?;
                }
                ast::Expression::TypeAssertion { expression, .. } => {
                    self.compile_expr(expression, dst).await?;
                }
                _ => {
                    return Err(CompileError::UnsupportedFeature {
                        location: CSourceLocation::unknown(&self.opts().source_name),
                        feature: "unsupported expression".to_string(),
                        help: None,
                    });
                }
            }
            Ok(())
        })
    }

    async fn compile_var_expr(&mut self, var: &ast::Var, dst: u8) -> Result<(), CompileError> {
        match var {
            ast::Var::Name(tok) => {
                let name = tok_str(tok);
                if let Some(local) = self.scope.resolve_mut(&name) {
                    local.read_count += 1;
                    let slot = local.slot;
                    if slot != dst {
                        self.cg.emit(Instruction::Move { dst, src: slot });
                    }
                } else if let Some(upval_idx) = self.resolve_upvalue(&name) {
                    self.cg.emit(Instruction::GetUpval {
                        dst,
                        upval: upval_idx,
                    });
                } else {
                    let name_idx = self.cg.name(name);
                    self.emit_global_read(name_idx, dst)?;
                }
            }
            ast::Var::Expression(ve) => {
                let suffixes: Vec<_> = ve.suffixes().collect();
                match suffixes.last() {
                    Some(ast::Suffix::Index(idx)) => {
                        let obj = self.alloc_temp()?;
                        self.compile_prefix_expr(ve.prefix(), obj).await?;
                        for s in &suffixes[..suffixes.len() - 1] {
                            self.apply_index_suffix(s, obj, obj).await?;
                        }
                        match idx {
                            ast::Index::Dot { name, .. } => {
                                let kb = tok_str(name);
                                let kidx = self.cg.constant(kb);
                                let k = self.alloc_temp()?;
                                self.cg.emit(Instruction::LoadK { dst: k, idx: kidx });
                                self.cg.emit(Instruction::GetTable {
                                    dst,
                                    table: obj,
                                    key: k,
                                });
                                self.free_temp();
                            }
                            ast::Index::Brackets { expression, .. } => {
                                let k = self.alloc_temp()?;
                                self.compile_expr(expression, k).await?;
                                self.cg.emit(Instruction::GetTable {
                                    dst,
                                    table: obj,
                                    key: k,
                                });
                                self.free_temp();
                            }
                            _ => return Err(self.unsupported_pos0("unknown index form")),
                        }
                        self.free_temp(); // obj
                    }
                    _ => return Err(self.unsupported_pos0("complex variable expression")),
                }
            }
            other => return Err(self.unsupported_node(other, "variable")),
        }
        Ok(())
    }

    async fn compile_number(
        &mut self,
        tok: &full_moon::tokenizer::TokenReference,
        dst: u8,
    ) -> Result<(), CompileError> {
        let s_bytes = tok_str(tok);
        let s = std::str::from_utf8(s_bytes.as_ref()).unwrap_or("");
        // Try integer first.
        if let Ok(i) = parse_integer(s) {
            let idx = self.cg.add_constant(Value::Integer(i));
            self.cg.emit(Instruction::LoadK { dst, idx });
            return Ok(());
        }
        // Fall back to float (decimal), then hex float.
        if let Ok(f) = s.parse::<f64>() {
            let idx = self.cg.add_constant(Value::Float(f));
            self.cg.emit(Instruction::LoadK { dst, idx });
            return Ok(());
        }
        if let Some(f) = shingetsu_vm::Number::parse_hex_float(s) {
            let idx = self.cg.add_constant(Value::Float(f));
            self.cg.emit(Instruction::LoadK { dst, idx });
            return Ok(());
        }
        Err(CompileError::Semantic {
            location: CSourceLocation::from_pos(&self.opts().source_name, tok.start_position()),
            message: format!("cannot parse number literal: {s}"),
            help: None,
        })
    }

    async fn compile_binop(
        &mut self,
        lhs: &ast::Expression,
        binop: &ast::BinOp,
        rhs: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        use ast::BinOp;

        // Short-circuit `and` / `or`.
        match binop {
            BinOp::And(_) => return self.compile_and(lhs, rhs, dst).await,
            BinOp::Or(_) => return self.compile_or(lhs, rhs, dst).await,
            _ => {}
        }

        let l = self.alloc_temp()?;
        self.compile_expr(lhs, l).await?;
        let r = self.alloc_temp()?;
        self.compile_expr(rhs, r).await?;

        let instr = match binop {
            BinOp::Plus(_) => Instruction::Add {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::Minus(_) => Instruction::Sub {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::Star(_) => Instruction::Mul {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::Slash(_) => Instruction::Div {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::DoubleSlash(_) => Instruction::IDiv {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::Percent(_) => Instruction::Mod {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::Caret(_) => Instruction::Pow {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::Ampersand(_) => Instruction::BAnd {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::Pipe(_) => Instruction::BOr {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::Tilde(_) => Instruction::BXor {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::DoubleLessThan(_) => Instruction::Shl {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::DoubleGreaterThan(_) => Instruction::Shr {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::TwoEqual(_) => Instruction::Eq {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::TildeEqual(_) => {
                // `a ~= b` is always `not (a == b)`; compiling as Eq+Not
                // ensures __eq metamethods are respected.
                self.set_span_loc(lhs, rhs);
                self.cg.emit(Instruction::Eq {
                    dst,
                    lhs: l,
                    rhs: r,
                });
                self.free_temp(); // r
                self.free_temp(); // l
                self.cg.emit(Instruction::Not { dst, src: dst });
                return Ok(());
            }
            BinOp::LessThan(_) => Instruction::Lt {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::LessThanEqual(_) => Instruction::Le {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::GreaterThan(_) => Instruction::Gt {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::GreaterThanEqual(_) => Instruction::Ge {
                dst,
                lhs: l,
                rhs: r,
            },
            BinOp::TwoDots(_) => {
                // l and r are already contiguous (allocated sequentially).
                self.set_span_loc(lhs, rhs);
                self.cg.emit(Instruction::Concat {
                    dst,
                    base: l,
                    count: 2,
                });
                self.free_temp(); // r
                self.free_temp(); // l
                return Ok(());
            }
            _ => {
                self.free_temp();
                self.free_temp();
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation::unknown(&self.opts().source_name),
                    feature: "unsupported binary operator".to_string(),
                    help: None,
                });
            }
        };

        self.set_span_loc(lhs, rhs);
        self.cg.emit(instr);
        self.free_temp(); // r
        self.free_temp(); // l
        Ok(())
    }

    async fn compile_and(
        &mut self,
        lhs: &ast::Expression,
        rhs: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        // `a and b` → if a is falsy, result = a; else result = b.
        self.compile_expr(lhs, dst).await?;
        let skip_rhs = self.cg.emit_branch_false(dst);
        self.compile_expr(rhs, dst).await?;
        let end_pc = self.cg.pc();
        self.cg.patch(skip_rhs, end_pc);
        Ok(())
    }

    async fn compile_or(
        &mut self,
        lhs: &ast::Expression,
        rhs: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        // `a or b` → if a is truthy, result = a; else result = b.
        self.compile_expr(lhs, dst).await?;
        let skip_rhs = self.cg.emit_branch_true(dst);
        self.compile_expr(rhs, dst).await?;
        let end_pc = self.cg.pc();
        self.cg.patch(skip_rhs, end_pc);
        Ok(())
    }

    async fn compile_unop(
        &mut self,
        unop: &ast::UnOp,
        expr: &ast::Expression,
        dst: u8,
    ) -> Result<(), CompileError> {
        let tmp = self.alloc_temp()?;
        self.compile_expr(expr, tmp).await?;
        let instr = match unop {
            ast::UnOp::Minus(_) => Instruction::Neg { dst, src: tmp },
            ast::UnOp::Not(_) => Instruction::Not { dst, src: tmp },
            ast::UnOp::Hash(_) => Instruction::Len { dst, src: tmp },
            ast::UnOp::Tilde(_) => Instruction::BNot { dst, src: tmp },
            _ => {
                self.free_temp();
                return Err(CompileError::UnsupportedFeature {
                    location: CSourceLocation::unknown(&self.opts().source_name),
                    feature: "unsupported unary operator".to_string(),
                    help: None,
                });
            }
        };
        self.set_span_loc(unop, expr);
        self.cg.emit(instr);
        self.free_temp();
        Ok(())
    }

    /// Compile a function call.  `nresults` is how many return values to keep
    /// (1 = single value into `dst`; 0 = called as statement; -1 = all).
    ///
    /// Register layout after this returns:
    ///   `dst`         = function value
    ///   `dst + 1`     = first argument (or `self` for method calls)
    ///   `dst + 2, …` = remaining arguments
    async fn compile_function_call(
        &mut self,
        fc: &ast::FunctionCall,
        dst: u8,
        nresults: i32,
    ) -> Result<(), CompileError> {
        self.note_reg_use(dst);
        let saved_temp_top = self.temp_top;

        let suffixes: Vec<_> = fc.suffixes().collect();
        let call_suffix = match suffixes.last() {
            Some(ast::Suffix::Call(c)) => c,
            _ => return Err(self.unsupported_pos0("call without call suffix")),
        };
        let index_suffixes = &suffixes[..suffixes.len() - 1];

        // --- For anonymous calls, load the function value into `dst`.
        //     For method calls, load the receiver into `dst` (it doubles
        //     as the self/arg-1 slot for the Invoke opcode).  Also
        //     compute the method-name constant index for method calls.
        let (first_arg_offset, nself, method_const): (u8, i32, Option<shingetsu_vm::ir::ConstIdx>) =
            match call_suffix {
                ast::Call::AnonymousCall(_) => {
                    if index_suffixes.is_empty() {
                        // Simple case: f(args).
                        self.compile_prefix_expr(fc.prefix(), dst).await?;
                    } else {
                        // Chain: a.b.c(args). Load prefix into T, chain through
                        // index suffixes, put function into `dst`.
                        let t = self.alloc_temp()?;
                        self.compile_prefix_expr(fc.prefix(), t).await?;
                        let (non_last, last) = index_suffixes.split_at(index_suffixes.len() - 1);
                        for s in non_last {
                            self.apply_index_suffix(s, t, t).await?;
                        }
                        self.apply_index_suffix(last[0], t, dst).await?;
                        self.free_temp(); // t
                    }
                    (1, 0, None)
                }
                ast::Call::MethodCall(mc) => {
                    // Load receiver chain directly into `dst`.  The Invoke
                    // opcode treats `R(dst)` as both the receiver and the
                    // self/arg-1 slot, so no separate Move is needed.
                    if index_suffixes.is_empty() {
                        self.compile_prefix_expr(fc.prefix(), dst).await?;
                    } else {
                        let t = self.alloc_temp()?;
                        self.compile_prefix_expr(fc.prefix(), t).await?;
                        let (non_last, last) = index_suffixes.split_at(index_suffixes.len() - 1);
                        for s in non_last {
                            self.apply_index_suffix(s, t, t).await?;
                        }
                        self.apply_index_suffix(last[0], t, dst).await?;
                        self.free_temp(); // t
                    }
                    let method_name = tok_str(mc.name());
                    let kidx = self.cg.constant(method_name);
                    (1, 1, Some(kidx))
                }
                _ => return Err(self.unsupported_pos0("unknown call form")),
            };

        // --- Check for dot-vs-colon call syntax mismatches against
        //     same-scope field definitions (e.g. `function t:m() end; t.m()`).
        self.check_call_syntax(fc.prefix(), index_suffixes, call_suffix);

        // --- Capture the `.` or `:` token position for call-site debug info.
        let dot_colon_token: Option<&full_moon::tokenizer::TokenReference> = match call_suffix {
            ast::Call::MethodCall(mc) => Some(mc.colon_token()),
            ast::Call::AnonymousCall(_) => {
                // For `a.b(args)`, the `.` is on the last index suffix.
                index_suffixes.last().and_then(|s| match s {
                    ast::Suffix::Index(ast::Index::Dot { dot, .. }) => Some(dot),
                    _ => None,
                })
            }
            _ => None,
        };
        // Start of the receiver expression (prefix of the function call).
        let receiver_start: Option<u32> =
            full_moon::node::Node::start_position(fc.prefix()).map(|p| p.bytes() as u32);

        // --- Evaluate explicit arguments and emit the Call instruction.
        let explicit_args: &ast::FunctionArgs = match call_suffix {
            ast::Call::AnonymousCall(a) => a,
            ast::Call::MethodCall(mc) => mc.args(),
            _ => unreachable!(),
        };
        // Set location to the call expression so that runtime errors
        // point at `require('name')` rather than the enclosing statement.
        self.set_node_loc(fc);
        self.compile_args_and_call(
            explicit_args,
            dst,
            first_arg_offset,
            nself,
            nresults,
            method_const,
            dot_colon_token,
            receiver_start,
        )
        .await?;
        // Restore temp_top: the Call instruction "consumes" all registers
        // dst + 1 .. dst + nargs, so they're no longer live.
        self.temp_top = saved_temp_top;
        Ok(())
    }

    /// Emit argument evaluation and the call instruction.
    ///
    /// For anonymous calls (`method_const = None`, `nself = 0`,
    /// `first_arg_offset = 1`): the function value must already be at
    /// `R(dst)`; explicit args land at `R(dst+1..)`.  Emits `Call`.
    ///
    /// For method calls (`method_const = Some(idx)`, `nself = 1`,
    /// `first_arg_offset = 1`): the receiver must already be at `R(dst)`
    /// where it doubles as the self/arg-1 slot; explicit args land at
    /// `R(dst+1..)`.  Emits `Invoke` with `idx` as the method-name
    /// constant.
    ///
    /// Caller is responsible for saving and restoring `temp_top` around
    /// this helper.
    fn compile_args_and_call<'b>(
        &'b mut self,
        explicit_args: &'b ast::FunctionArgs,
        dst: u8,
        first_arg_offset: u8,
        nself: i32,
        nresults: i32,
        method_const: Option<shingetsu_vm::ir::ConstIdx>,
        dot_colon_token: Option<&'b full_moon::tokenizer::TokenReference>,
        receiver_start: Option<u32>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), CompileError>> + Send + 'b>>
    {
        Box::pin(async move {
            let base = self.scope.current_slot() as usize;
            let mut nargs = nself;
            match explicit_args {
                ast::FunctionArgs::Parentheses { arguments, .. } => {
                    let arg_list: Vec<_> = arguments.iter().collect();
                    let last_arg_idx = arg_list.len().wrapping_sub(1);
                    for (i, arg) in arg_list.iter().enumerate() {
                        let arg_reg = dst + first_arg_offset + (nargs - nself) as u8;
                        // Guard: sub-expression temps start above this arg.
                        let needed = (arg_reg as usize + 1).saturating_sub(base);
                        if (self.temp_top as usize) < needed {
                            self.temp_top = needed as u8;
                        }
                        self.note_reg_use(arg_reg);
                        // If the last argument is `...`, expand it and signal
                        // variable arg count to the Call instruction.
                        if i == last_arg_idx && is_vararg_expr(arg) {
                            self.cg.emit(Instruction::Vararg {
                                dst: arg_reg,
                                nresults: -1,
                            });
                            nargs = -1; // sentinel: nargs = -1 means "all on stack"
                            break;
                        }
                        // If the last argument is a function call, expand it.
                        if i == last_arg_idx {
                            if let ast::Expression::FunctionCall(last_fc) = arg {
                                self.compile_function_call(last_fc, arg_reg, -1).await?;
                                nargs = -1;
                                break;
                            }
                        }
                        self.compile_expr(arg, arg_reg).await?;
                        nargs += 1;
                    }
                }
                ast::FunctionArgs::String(s) => {
                    let arg_reg = dst + first_arg_offset;
                    self.note_reg_use(arg_reg);
                    let bytes = parse_string_literal(s);
                    let idx = self.cg.constant(bytes);
                    self.cg.emit(Instruction::LoadK { dst: arg_reg, idx });
                    nargs += 1;
                }
                ast::FunctionArgs::TableConstructor(tc) => {
                    let arg_reg = dst + first_arg_offset;
                    let needed = (arg_reg as usize + 1).saturating_sub(base);
                    if (self.temp_top as usize) < needed {
                        self.temp_top = needed as u8;
                    }
                    self.note_reg_use(arg_reg);
                    self.compile_table_constructor(tc, arg_reg).await?;
                    nargs += 1;
                }
                _ => return Err(self.unsupported_pos0("unknown function arg form")),
            }

            let pc = match method_const {
                Some(method_const) => self.cg.emit(Instruction::Invoke {
                    dst,
                    nargs: nargs as i16,
                    nresults: nresults as i16,
                    method_const,
                }),
                None => self.cg.emit(Instruction::Call {
                    func: dst,
                    nargs: nargs as i16,
                    nresults: nresults as i16,
                }),
            };
            if let Some(tok) = dot_colon_token {
                if let Some(pos) = full_moon::node::Node::start_position(tok) {
                    self.cg.set_call_site_info(
                        pc,
                        shingetsu_vm::proto::CallSiteInfo {
                            dot_colon_offset: pos.bytes() as u32,
                            dot_colon_len: 1,
                            receiver_offset: receiver_start.unwrap_or(0),
                        },
                    );
                }
            }
            Ok(())
        })
    }

    /// Compile a table constructor `{...}` into register `dst`.
    async fn compile_table_constructor(
        &mut self,
        tc: &ast::TableConstructor,
        dst: u8,
    ) -> Result<(), CompileError> {
        // Hint: count positional fields for array hint.
        let fields: Vec<_> = tc.fields().iter().collect();
        let array_hint = fields
            .iter()
            .filter(|f| matches!(f, ast::Field::NoKey(_)))
            .count() as u32;
        let hash_count = (fields.len() as u32).saturating_sub(array_hint);
        self.cg.emit(Instruction::NewTable {
            dst,
            array_hint: shingetsu_vm::ir::encode_size_hint(array_hint),
            hash_hint: shingetsu_vm::ir::encode_size_hint(hash_count),
        });

        let mut batch = SetListBatch::new(dst);
        let last_field_idx = fields.len().wrapping_sub(1);

        for (field_idx, field) in fields.iter().enumerate() {
            match field {
                ast::Field::NoKey(expr) => {
                    // Final field as `...` or a function call expands
                    // its multi-results to fill the remaining array
                    // slots (Lua 5.4 §3.4.9).  Emit a tail SetList
                    // with count=-1 to absorb everything from the
                    // source base to the register-file top.
                    if field_idx == last_field_idx
                        && (is_vararg_expr(expr)
                            || matches!(expr, ast::Expression::FunctionCall(_)))
                    {
                        batch.flush(self).await?;
                        let base = self.alloc_temp()?;
                        if is_vararg_expr(expr) {
                            self.cg.emit(Instruction::Vararg {
                                dst: base,
                                nresults: -1,
                            });
                        } else if let ast::Expression::FunctionCall(fc) = expr {
                            self.compile_function_call(fc, base, -1).await?;
                        }
                        let as_idx = self.cg.add_constant(Value::Integer(batch.next_index()));
                        self.cg.emit(Instruction::SetList {
                            table: dst,
                            src_base: base,
                            count: -1,
                            array_start: as_idx,
                        });
                        self.free_temp(); // base
                        continue;
                    }
                    batch.push(self, expr).await?;
                }
                ast::Field::NameKey { key, value, .. } => {
                    // A hash-key field reuses the next free register;
                    // any pending positional batch must be drained
                    // first so its register window doesn't overlap.
                    batch.flush(self).await?;
                    // Named: t["key"] = value
                    let v = self.alloc_temp()?;
                    self.compile_expr(value, v).await?;
                    let k = self.alloc_temp()?;
                    let kb = tok_str(key);
                    let kidx = self.cg.constant(kb);
                    self.cg.emit(Instruction::LoadK { dst: k, idx: kidx });
                    self.cg.emit(Instruction::SetTable {
                        table: dst,
                        key: k,
                        src: v,
                    });
                    self.free_temp(); // k
                    self.free_temp(); // v
                }
                ast::Field::ExpressionKey { key, value, .. } => {
                    batch.flush(self).await?;
                    // Computed: t[key_expr] = value
                    let v = self.alloc_temp()?;
                    self.compile_expr(value, v).await?;
                    let k = self.alloc_temp()?;
                    self.compile_expr(key, k).await?;
                    self.cg.emit(Instruction::SetTable {
                        table: dst,
                        key: k,
                        src: v,
                    });
                    self.free_temp(); // k
                    self.free_temp(); // v
                }
                other => return Err(self.unsupported_node(other, "table field")),
            }
        }
        batch.flush(self).await?;
        Ok(())
    }

    /// Check whether a call like `t.foo()` or `t:foo()` uses the same
    /// syntax (dot vs colon) as the definition `function t.foo()` or
    /// `function t:foo()` in the same scope.  Emits a warning diagnostic
    /// on mismatch.
    fn check_call_syntax(
        &mut self,
        prefix: &ast::Prefix,
        index_suffixes: &[&ast::Suffix],
        call_suffix: &ast::Call,
    ) {
        // Extract the receiver local name from the prefix.
        let receiver_name = match prefix {
            ast::Prefix::Name(tok) => tok_str(tok),
            _ => return,
        };

        // Determine the field name, whether this is a method call, and the
        // position of the `.` or `:` token (for pointing the diagnostic caret).
        let (field_name, is_method_call, dot_colon_pos) = match call_suffix {
            ast::Call::MethodCall(mc) => {
                // `t:foo()` — only valid when there are no intermediate
                // index suffixes (i.e. `t:foo()`, not `t.x:foo()`).
                if !index_suffixes.is_empty() {
                    return;
                }
                let pos = mc.colon_token().start_position();
                (tok_str(mc.name()), true, Some(pos))
            }
            ast::Call::AnonymousCall(_) => {
                // `t.foo()` — exactly one index suffix that's a dot.
                if index_suffixes.len() != 1 {
                    return;
                }
                match index_suffixes[0] {
                    ast::Suffix::Index(ast::Index::Dot { dot, name, .. }) => {
                        let pos = dot.start_position();
                        (tok_str(name), false, Some(pos))
                    }
                    _ => return,
                }
            }
            _ => return,
        };

        // Look up the receiver's field definition to determine whether the
        // field was defined with method (`:`) or function (`.`) syntax.
        //
        // First check same-scope locals, then fall back to the global
        // type map for globals with inferred type info.
        let defined_as_method = if let Some(local) = self.scope.resolve(&receiver_name) {
            match local.field_defs.get(&field_name) {
                Some(m) => *m,
                // No same-scope field def — check the local's inferred type.
                None => match &local.inferred_type {
                    Some(ty) => match Self::lookup_field_is_method(ty, &field_name) {
                        Some(m) => m,
                        None => return,
                    },
                    None => return,
                },
            }
        } else if let Some(is_method) =
            self.lookup_global_field_is_method(&receiver_name, &field_name)
        {
            is_method
        } else {
            return;
        };

        if is_method_call == defined_as_method {
            return;
        }

        // Suppress the warning when a `:` method is called with `.` and the
        // first explicit argument is the receiver itself (e.g. `t.method(t)`).
        // This is the manual equivalent of `t:method()` and is intentional.
        if defined_as_method && !is_method_call {
            if let ast::Call::AnonymousCall(ast::FunctionArgs::Parentheses { arguments, .. }) =
                call_suffix
            {
                if let Some(first_arg) = arguments.iter().next() {
                    if let ast::Expression::Var(ast::Var::Name(tok)) = first_arg {
                        if tok_str(tok) == receiver_name {
                            return;
                        }
                    }
                }
            }
        }

        let loc = dot_colon_pos
            .map(|p| CSourceLocation::from_pos(&self.opts().source_name, p))
            .unwrap_or_else(|| CSourceLocation::unknown(&self.opts().source_name));
        let field_str = String::from_utf8_lossy(&field_name);
        let receiver_str = String::from_utf8_lossy(&receiver_name);
        let (used, expected) = if is_method_call {
            (":", ".")
        } else {
            (".", ":")
        };
        self.diagnostics.push(Diagnostic {
            lint: LintId::CallConvention,
            severity: crate::error::Severity::Warning,
            location: loc,
            message: format!(
                "'{field_str}' was defined with '{expected}' syntax \
                 but called as '{receiver_str}{used}{field_str}()'; \
                 did you mean '{receiver_str}{expected}{field_str}()'?"
            ),
            help: Some(format!(
                "use '{expected}' syntax: '{receiver_str}{expected}{field_str}()'"
            )),
        });
    }

    /// Look up a field on a global's inferred type and return whether it
    /// is a method (`is_method`).  Returns `None` if the global is not in
    /// the type map, is not a table type, or the field is not found.
    /// If `expr` is `require("literal")`, return the string literal.
    /// Returns `None` for any other expression shape.
    /// Check if an assignment target is `package.path` (global `package`
    /// with a single `.path` dot-index suffix).
    fn is_package_path_target(var: &ast::Var) -> bool {
        let ve = match var {
            ast::Var::Expression(ve) => ve,
            _ => return false,
        };
        // Prefix must be the bare name `package`.
        match ve.prefix() {
            ast::Prefix::Name(tok) if tok_str(tok) == &b"package"[..] => {}
            _ => return false,
        }
        // Exactly one suffix: `.path`.
        let suffixes: Vec<_> = ve.suffixes().collect();
        if suffixes.len() != 1 {
            return false;
        }
        matches!(
            &suffixes[0],
            ast::Suffix::Index(ast::Index::Dot { name, .. })
                if tok_str(name) == &b"path"[..]
        )
    }

    /// Try to evaluate an expression as a compile-time string constant.
    /// Handles string literals, `package.path` references (resolved from
    /// the current `effective_package_path`), and binary `..` concatenation
    /// of sub-expressions that are themselves statically evaluable.
    fn try_eval_static_string(&self, expr: &ast::Expression) -> Option<String> {
        match expr {
            ast::Expression::String(s) => {
                let bytes = parse_string_literal(s);
                String::from_utf8(bytes.to_vec()).ok()
            }
            ast::Expression::Var(ast::Var::Expression(ve)) => {
                // Recognize `package.path`.
                match ve.prefix() {
                    ast::Prefix::Name(tok) if tok_str(tok) == &b"package"[..] => {}
                    _ => return None,
                }
                let suffixes: Vec<_> = ve.suffixes().collect();
                if suffixes.len() != 1 {
                    return None;
                }
                match &suffixes[0] {
                    ast::Suffix::Index(ast::Index::Dot { name, .. })
                        if tok_str(name) == &b"path"[..] =>
                    {
                        self.effective_package_path.clone()
                    }
                    _ => None,
                }
            }
            ast::Expression::BinaryOperator { lhs, binop, rhs } => {
                // Only handle `..` (concatenation).
                if !matches!(binop, ast::BinOp::TwoDots(_)) {
                    return None;
                }
                let l = self.try_eval_static_string(lhs)?;
                let r = self.try_eval_static_string(rhs)?;
                Some(l + &r)
            }
            // Parenthesized expression: unwrap.
            ast::Expression::Parentheses { expression, .. } => {
                self.try_eval_static_string(expression)
            }
            _ => None,
        }
    }

    fn extract_require_literal(expr: &ast::Expression) -> Option<String> {
        extract_require_literal(expr)
    }

    /// Resolve type information for a `require("name")` call.
    ///
    /// First checks the module type registry cache. On a miss, if a
    /// module loader and package path are configured, compiles the
    /// dependency on demand and caches the result.
    ///
    /// Returns `None` if the module cannot be found or is currently
    /// being compiled (circular require).
    async fn resolve_require_type(
        &self,
        mod_name: &str,
    ) -> Option<shingetsu_vm::types::ModuleTypeInfo> {
        let registry = &self.compiler.module_types;
        let name_bytes = mod_name.as_bytes();

        // Fast path: already in the cache.
        if let Some(info) = registry.get(name_bytes) {
            return Some(info);
        }

        // No loader or no package path — can't resolve on demand.
        let loader = self.compiler.module_loader.as_ref()?;
        let package_path = self.effective_package_path.as_ref()?;

        // Circular require guard.
        if !registry.begin_compile(name_bytes) {
            return None;
        }

        let candidates = shingetsu_vm::candidate_paths(mod_name, package_path);
        let mut result = None;
        for path in &candidates {
            match loader.load(mod_name, path).await {
                Ok(loaded) => {
                    registry.insert(Bytes::from(mod_name.to_owned()), loaded.type_info.clone());
                    result = Some(loaded.type_info);
                    break;
                }
                Err(_) => continue,
            }
        }

        registry.end_compile(name_bytes);
        result
    }

    /// Look up a field on a type and return whether it is a method.
    /// Returns `None` if the type is not a table or the field is not found.
    fn lookup_field_is_method(
        ty: &shingetsu_vm::types::LuaType,
        field_name: &Bytes,
    ) -> Option<bool> {
        use shingetsu_vm::types::LuaType;
        let table = match ty {
            LuaType::Table(t) => t,
            _ => return None,
        };
        for (name, field_ty) in &table.fields {
            if name == field_name {
                if let LuaType::Function(f) = field_ty {
                    return Some(f.is_method);
                }
                return None;
            }
        }
        None
    }

    /// Look up a field on a global's inferred type and return whether it
    /// is a method (`is_method`).  Returns `None` if the global is not in
    /// the type map, is not a table type, or the field is not found.
    /// Build a `LuaType::Function` from a compiled proto's signature.
    /// Used to accumulate function types into a local's table type.
    fn function_type_from_proto(
        sig: &std::sync::Arc<shingetsu_vm::types::FunctionSignature>,
        is_method: bool,
    ) -> shingetsu_vm::types::LuaType {
        let params: Vec<(Option<Bytes>, shingetsu_vm::types::LuaType)> = sig
            .params
            .iter()
            .map(|p| {
                let ty = p
                    .lua_type
                    .clone()
                    .unwrap_or(shingetsu_vm::types::LuaType::Any);
                (p.name.clone(), ty)
            })
            .collect();
        let has_any_annotation =
            sig.params.iter().any(|p| p.lua_type.is_some()) || sig.lua_returns.is_some();
        let variadic = if sig.variadic {
            Some(Box::new(shingetsu_vm::types::LuaType::Any))
        } else {
            None
        };
        let returns = sig.lua_returns.clone().unwrap_or_default();
        shingetsu_vm::types::LuaType::Function(Box::new(shingetsu_vm::types::FunctionLuaType {
            type_params: sig.type_params.clone(),
            params,
            variadic,
            returns,
            is_method,
            inferred_unannotated: !has_any_annotation,
        }))
    }

    fn lookup_global_field_is_method(
        &self,
        global_name: &Bytes,
        field_name: &Bytes,
    ) -> Option<bool> {
        let global_type = self.compiler.global_types.get(global_name)?;
        Self::lookup_field_is_method(global_type, field_name)
    }

    async fn compile_prefix_expr(
        &mut self,
        prefix: &ast::Prefix,
        dst: u8,
    ) -> Result<(), CompileError> {
        match prefix {
            ast::Prefix::Name(tok) => {
                let name = tok_str(tok);
                if let Some(local) = self.scope.resolve_mut(&name) {
                    local.read_count += 1;
                    let slot = local.slot;
                    if slot != dst {
                        self.cg.emit(Instruction::Move { dst, src: slot });
                    }
                } else if let Some(upval_idx) = self.resolve_upvalue(&name) {
                    self.cg.emit(Instruction::GetUpval {
                        dst,
                        upval: upval_idx,
                    });
                } else {
                    let name_idx = self.cg.name(name);
                    self.emit_global_read(name_idx, dst)?;
                }
            }
            ast::Prefix::Expression(e) => {
                self.compile_expr(e, dst).await?;
            }
            other => return Err(self.unsupported_node(other, "prefix expression")),
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Finish
    // -----------------------------------------------------------------------

    /// Finalise this compiler into a [`Proto`].
    ///
    /// `line_defined` and `last_line_defined` are the source line bounds
    /// that debug tooling (and `debug.getinfo`) surface on the function.
    /// For the main chunk these should be `(0, last_source_line)`, per
    /// Lua 5.4 convention.
    fn finish(
        mut self,
        name: Bytes,
        params: Vec<ParamSpec>,
        variadic: bool,
        line_defined: u32,
        last_line_defined: u32,
    ) -> (Proto, Vec<Diagnostic>) {
        // Flush any remaining scopes into debug_local_descs before building
        // the proto.  The top-level chunk and function bodies may leave the
        // root scope un-popped.
        let end_pc = self.cg.instructions.len();
        while self.scope.scope_depth() > 0 {
            let locals = self.scope.pop_scope();
            for local in &locals {
                self.check_unused_local(local);

                if self.opts().debug_info && local.attr != LocalAttr::Close {
                    self.debug_local_descs.push(LocalDesc {
                        name: local.name.clone(),
                        attr: local.attr,
                        slot: local.slot,
                        start_pc: local.start_pc,
                        end_pc,
                        decl_location: local.decl_location.clone().map(Into::into),
                        is_implicit_self: local.is_implicit_self,
                    });
                }
            }
        }

        // Ensure every path ends with a Return.
        if !matches!(
            self.cg.instructions.last(),
            Some(Instruction::Return { .. })
        ) {
            self.cg.emit(Instruction::Return {
                base: 0,
                nresults: 0,
            });
        }

        let num_upvalues = self.upvalue_descs.lock().len() as u8;

        let has_runtime_types = params.iter().any(|p| p.runtime_type.is_some());
        let sig = Arc::new(FunctionSignature {
            name,
            source: Bytes::from(self.opts().source_name.as_bytes()),
            type_params: vec![],
            params,
            variadic,
            arg_offset: 0,
            returns: None,
            lua_returns: None,
            line_defined,
            last_line_defined,
            num_upvalues,
            has_runtime_types,
        });

        let upvalues = self.upvalue_descs.lock().clone();
        let env_upvalue_idx = upvalues
            .iter()
            .position(|d| d.name == "_ENV")
            .map(|i| i as u8);
        let proto = encode_proto(
            sig,
            self.cg,
            {
                let mut all = self.close_local_descs;
                all.extend(self.debug_local_descs);
                all
            },
            upvalues,
            env_upvalue_idx,
            self.child_protos,
            self.type_aliases,
            self.max_stack_size.max(self.scope.max_slot as u16),
        );
        (proto, self.diagnostics)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a `Proto` from a completed `CodeGen`, encoding instructions to
/// compact u32 bytecode and remapping source locations and call-site info.
fn encode_proto(
    signature: Arc<FunctionSignature>,
    cg: CodeGen,
    locals: Vec<LocalDesc>,
    upvalues: Vec<UpvalueDesc>,
    env_upvalue_idx: Option<u8>,
    protos: Vec<Arc<Proto>>,
    type_aliases: std::collections::HashMap<Bytes, TypeAlias>,
    max_stack_size: u16,
) -> Proto {
    let (code, index_map) = bytecode::encode_all(&cg.instructions);

    // Remap source_locations: expand to match the u32 code array.  For
    // multi-word instructions (SetList, Invoke), every emitted word
    // shares the source location of the originating IR instruction —
    // otherwise traceback resolution lands on the trailing ExtraArg
    // word and reports `?:` instead of the real line.
    let source_locations = if cg.source_locations.is_empty() {
        Vec::new()
    } else {
        let mut locs = vec![None; code.len()];
        for (old_idx, loc) in cg.source_locations.into_iter().enumerate() {
            let Some(&new_idx) = index_map.get(old_idx) else {
                continue;
            };
            let width = index_map.get(old_idx + 1).copied().unwrap_or(code.len()) - new_idx;
            for w in 0..width {
                locs[new_idx + w] = loc.clone();
            }
        }
        locs
    };

    // Remap call_site_info keys from old PC to new PC.
    let call_site_info = cg
        .call_site_info
        .into_iter()
        .filter_map(|(old_pc, info)| index_map.get(old_pc).map(|&new_pc| (new_pc, info)))
        .collect();

    Proto {
        signature,
        code,
        constants: cg.constants,
        locals,
        upvalues,
        env_upvalue_idx,
        protos,
        source_locations,
        call_site_info,
        source_name: Arc::new(String::new()),
        source_text: Bytes::default(),
        type_aliases,
        max_stack_size,
    }
}

/// Look up the synthetic `_ENV` upvalue in `descs`, registering a new
/// entry pointing at `parent_idx` if not already present.  Returns its
/// index in `descs`.
fn ensure_env_in_ancestor(descs: &Mutex<Vec<UpvalueDesc>>, parent_idx: u8) -> u8 {
    let mut descs = descs.lock();
    if let Some(idx) = descs.iter().position(|d| d.name == "_ENV") {
        return idx as u8;
    }
    let idx = descs.len() as u8;
    descs.push(UpvalueDesc {
        name: "_ENV".into(),
        in_stack: false,
        index: parent_idx,
    });
    idx
}

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
pub(crate) fn tok_str(tok: &TokenReference) -> Bytes {
    match tok.token().token_type() {
        TokenType::Identifier { .. } => Bytes::from(ident(tok.token()).as_bytes()),
        _ => Bytes::from(tok.token().to_string().as_bytes()),
    }
}

// ---------------------------------------------------------------------------
// SetListBatch — helper for `compile_table_constructor`.
// ---------------------------------------------------------------------------

/// Buffered emission of consecutive positional fields in a table
/// constructor.
///
/// Plain `{a, b, c, ...}` fields can be written with a single
/// `SetList` covering a register window, instead of N separate
/// `LoadK + SetTable` pairs.  This struct accumulates expressions
/// and emits one `SetList` per chunk of `MAX_BATCH` fields.
///
/// `MAX_BATCH` keeps the live register window small (the batch's
/// values are temporaries held above the current scope's slot top
/// until flush).  50 is well below the per-instruction operand limits
/// and matches the order-of-magnitude reference Lua uses.
struct SetListBatch<'a> {
    /// Destination table register — same value for every flush of
    /// this constructor.
    table_reg: u8,
    /// Buffered expressions awaiting flush.  Empty when nothing is
    /// pending.
    pending: Vec<&'a ast::Expression>,
    /// Next array index that the head of `pending` will land at when
    /// flushed (1-based, like Lua array indexing).
    next_array_idx: i64,
}

impl<'a> SetListBatch<'a> {
    const MAX_BATCH: usize = 50;

    fn new(table_reg: u8) -> Self {
        Self {
            table_reg,
            pending: Vec::new(),
            next_array_idx: 1,
        }
    }

    /// Index where the next-to-be-pushed field would land in the
    /// final table.  Used for the tail-SetList path that emits
    /// `count = -1` after a vararg/call.
    fn next_index(&self) -> i64 {
        self.next_array_idx + self.pending.len() as i64
    }

    /// Buffer a positional field; auto-flush if the batch is full.
    async fn push(
        &mut self,
        cx: &mut FnCompiler<'_>,
        expr: &'a ast::Expression,
    ) -> Result<(), CompileError> {
        self.pending.push(expr);
        if self.pending.len() >= Self::MAX_BATCH {
            self.flush(cx).await?;
        }
        Ok(())
    }

    /// Emit a `SetList` for the buffered fields, then clear the
    /// buffer.  No-op when nothing is pending.
    async fn flush(&mut self, cx: &mut FnCompiler<'_>) -> Result<(), CompileError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        // Save the entry location so we can restore it after
        // compiling fields.  Each field-level `set_node_loc` below
        // narrows the location for any error raised during that
        // field's emit (notably constant-pool overflow); restoring
        // the entry loc afterwards keeps the constructor's outer
        // location attached to the `SetList` and to subsequent
        // instructions emitted by the caller.
        let entry_loc = cx.cg.current_loc().cloned();
        let base = cx.alloc_temp()?;
        for (i, e) in self.pending.iter().enumerate() {
            if i > 0 {
                cx.alloc_temp()?;
            }
            cx.set_node_loc(*e);
            cx.compile_expr(e, base + i as u8).await?;
        }
        cx.cg.set_loc(entry_loc);
        let as_idx = cx.cg.add_constant(Value::Integer(self.next_array_idx));
        cx.cg.emit(Instruction::SetList {
            table: self.table_reg,
            src_base: base,
            count: self.pending.len() as i16,
            array_start: as_idx,
        });
        for _ in 0..self.pending.len() {
            cx.free_temp();
        }
        self.next_array_idx += self.pending.len() as i64;
        self.pending.clear();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn lower_chunk(
    ast: &Ast,
    compiler_ctx: &Compiler,
) -> Result<(Proto, Vec<Diagnostic>, Option<shingetsu_vm::types::LuaType>), CompileError> {
    let mut compiler = FnCompiler::new(compiler_ctx);
    // The top-level chunk is implicitly variadic (receives command-line args
    // / host-provided args as `...`).
    compiler.is_variadic = true;

    // The top-level chunk is an implicit function with no parameters.
    for stmt in ast.nodes().stmts() {
        compiler.compile_stmt(stmt).await?;
    }
    if let Some(last) = ast.nodes().last_stmt() {
        compiler.compile_last_stmt(last).await?;
    }

    // Determine the module's return type for cross-module type propagation.
    // Handles two patterns:
    //   1. `return <local>` where the local has a known type
    //   2. `return { key = value, ... }` — structural inference from
    //      the table constructor's named fields
    let module_return_type = ast.nodes().last_stmt().and_then(|stmt| match stmt {
        ast::LastStmt::Return(r) => {
            let returns: Vec<_> = r.returns().iter().collect();
            if returns.len() == 1 {
                if let ast::Expression::Var(ast::Var::Name(tok)) = &returns[0] {
                    let name = tok_str(tok);
                    compiler
                        .scope
                        .resolve(&name)
                        .and_then(|local| local.inferred_type.clone())
                } else if let ast::Expression::TableConstructor(tc) = &returns[0] {
                    infer_table_constructor_type(tc, &compiler)
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    });

    // Main-chunk line bounds: Lua 5.4 convention is `linedefined = 0`
    // and `lastlinedefined = <last line of source>`.  We derive the
    // last line from the EOF token's start position — after the
    // tokenizer consumed all content, that is the line the file ends
    // on (accounting for trailing whitespace/comments).  If the source
    // contains no content at all, we fall back to `0` so both bounds
    // are `0` — matching how Lua treats an empty chunk.
    let last_line_defined = ast.eof().start_position().line() as u32;

    if let Some(loc) = compiler.cg.constant_overflow.take() {
        // Fill in the source name when CodeGen had no location set
        // (e.g. an emit path that didn't call `set_loc`).  Without
        // this the diagnostic would render with an empty file name.
        let source_name = if loc.source_name.is_empty() {
            Arc::clone(&compiler_ctx.opts.source_name)
        } else {
            loc.source_name
        };
        let location = CSourceLocation {
            source_name,
            line: loc.line,
            column: loc.column,
            byte_offset: loc.byte_offset,
            byte_len: loc.byte_len,
        };
        return Err(CompileError::Semantic {
            location,
            message: format!("too many constants in chunk (limit: {})", u16::MAX),
            help: Some(
                "split large literal table constructors into smaller pieces, \
                 or load data from an external source at runtime"
                    .to_string(),
            ),
        });
    }

    let (proto, diagnostics) = compiler.finish(
        Bytes::from(compiler_ctx.opts.source_name.as_bytes()),
        vec![],
        true, // top-level chunk is variadic
        0,
        last_line_defined,
    );
    Ok((proto, diagnostics, module_return_type))
}

// ---------------------------------------------------------------------------
// Vararg helper
// ---------------------------------------------------------------------------

/// Return `true` if `expr` is the bare `...` vararg expression.
/// Extract the module name from a `require("literal")` call expression.
/// Returns `None` if the expression is not a require call with a single
/// string literal argument.
pub(crate) fn extract_require_literal(expr: &ast::Expression) -> Option<String> {
    let fc = match expr {
        ast::Expression::FunctionCall(fc) => fc,
        _ => return None,
    };
    // Must be a plain `require` call (no chained suffixes).
    let suffixes: Vec<_> = fc.suffixes().collect();
    if suffixes.len() != 1 {
        return None;
    }
    // Prefix must be the name `require`.
    match fc.prefix() {
        ast::Prefix::Name(tok) if tok_str(tok) == &b"require"[..] => {}
        _ => return None,
    }
    // Single string argument: require("foo") or require 'foo'.
    match &suffixes[0] {
        ast::Suffix::Call(ast::Call::AnonymousCall(ast::FunctionArgs::Parentheses {
            arguments,
            ..
        })) => {
            let args: Vec<_> = arguments.iter().collect();
            if args.len() != 1 {
                return None;
            }
            match &args[0] {
                ast::Expression::String(s) => {
                    let bytes = parse_string_literal(s);
                    String::from_utf8(bytes.to_vec()).ok()
                }
                _ => None,
            }
        }
        ast::Suffix::Call(ast::Call::AnonymousCall(ast::FunctionArgs::String(s))) => {
            let bytes = parse_string_literal(s);
            String::from_utf8(bytes.to_vec()).ok()
        }
        _ => None,
    }
}

fn is_vararg_expr(expr: &ast::Expression) -> bool {
    matches!(
        expr,
        ast::Expression::Symbol(tok)
            if tok.token().to_string() == "..."
    )
}

// ---------------------------------------------------------------------------
// Number parsing helpers
// ---------------------------------------------------------------------------

fn parse_integer(s: &str) -> Result<i64, ()> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        // Hex integer literals wrap modularly to i64 per Lua 5.4
        // §3.1; this also accepts 17+ digit literals that
        // `i64::from_str_radix` would reject as overflow.
        shingetsu_vm::Number::parse_hex_integer_wrapping(hex).ok_or(())
    } else if s.contains('.') || s.contains('e') || s.contains('E') {
        Err(())
    } else {
        s.parse::<i64>().map_err(|_| ())
    }
}

pub(crate) fn parse_string_literal(tok: &TokenReference) -> Bytes {
    match tok.token().token_type() {
        TokenType::StringLiteral {
            literal,
            multi_line_depth,
            ..
        } => {
            if *multi_line_depth == 0 {
                // Short string — process Lua escape sequences.
                unescape_string(literal.as_str())
            } else {
                // Long string `[[…]]` / `[=[…]=]` — no escape processing.
                // The first newline (if any) immediately after the opening
                // bracket is stripped per the Lua reference.
                let s = literal.as_str();
                let s = if s.starts_with('\n') {
                    &s[1..]
                } else if s.starts_with("\r\n") {
                    &s[2..]
                } else {
                    s
                };
                Bytes::from(s.as_bytes())
            }
        }
        _ => {
            // Fallback: should not happen for String tokens.
            Bytes::from(tok.token().to_string().as_bytes())
        }
    }
}

/// Process Lua string escape sequences in the raw literal (contents between
/// quotes, not including the quote characters themselves).
fn unescape_string(s: &str) -> Bytes {
    let bytes = s.as_bytes();
    let mut buf = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            buf.extend_from_slice(&bytes[i..i + 1]);
            i += 1;
            continue;
        }
        i += 1; // skip backslash
        if i >= bytes.len() {
            break;
        }
        match bytes[i] {
            b'a' => {
                buf.extend_from_slice(&[0x07]);
                i += 1;
            }
            b'b' => {
                buf.extend_from_slice(&[0x08]);
                i += 1;
            }
            b'f' => {
                buf.extend_from_slice(&[0x0C]);
                i += 1;
            }
            b'n' => {
                buf.extend_from_slice(&[0x0A]);
                i += 1;
            }
            b'r' => {
                buf.extend_from_slice(&[0x0D]);
                i += 1;
            }
            b't' => {
                buf.extend_from_slice(&[0x09]);
                i += 1;
            }
            b'v' => {
                buf.extend_from_slice(&[0x0B]);
                i += 1;
            }
            b'\\' => {
                buf.extend_from_slice(&[0x5C]);
                i += 1;
            }
            b'\'' => {
                buf.extend_from_slice(&[0x27]);
                i += 1;
            }
            b'"' => {
                buf.extend_from_slice(&[0x22]);
                i += 1;
            }
            b'`' => {
                buf.extend_from_slice(&[0x60]);
                i += 1;
            }
            b'{' => {
                buf.extend_from_slice(&[0x7B]);
                i += 1;
            }
            b'\n' => {
                buf.extend_from_slice(&[0x0A]);
                i += 1;
            }
            b'\r' => {
                buf.extend_from_slice(&[0x0A]);
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1; // \r\n → single newline
                }
            }
            b'x' => {
                // \xNN — exactly two hex digits.
                i += 1;
                if i + 2 <= bytes.len() {
                    if let Ok(s2) = std::str::from_utf8(&bytes[i..i + 2]) {
                        if let Ok(v) = u8::from_str_radix(s2, 16) {
                            buf.extend_from_slice(&[v]);
                            i += 2;
                            continue;
                        }
                    }
                }
                buf.extend_from_slice(b"x"); // malformed, pass through
            }
            b'u' => {
                // \u{NNNN} — Unicode code point (Lua 5.4).
                i += 1;
                if i < bytes.len() && bytes[i] == b'{' {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i] != b'}' {
                        i += 1;
                    }
                    if let Ok(hex) = std::str::from_utf8(&bytes[start..i]) {
                        if let Ok(n) = u32::from_str_radix(hex, 16) {
                            if let Some(c) = char::from_u32(n) {
                                let mut tmp = [0u8; 4];
                                buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                            }
                        }
                    }
                    if i < bytes.len() {
                        i += 1; // skip closing '}'
                    }
                } else {
                    buf.extend_from_slice(b"u");
                }
            }
            b'z' => {
                // \z — skip following whitespace (space, \t, \n, \r, \v, \f).
                i += 1;
                while i < bytes.len()
                    && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C)
                {
                    i += 1;
                }
            }
            d @ b'0'..=b'9' => {
                // \ddd — decimal escape, 1-3 digits, value 0-255.
                let mut n = (d - b'0') as u32;
                i += 1;
                let mut count = 1;
                while count < 3 && i < bytes.len() && bytes[i].is_ascii_digit() {
                    n = n * 10 + (bytes[i] - b'0') as u32;
                    i += 1;
                    count += 1;
                }
                buf.extend_from_slice(&[n as u8]);
            }
            other => {
                // Unknown escape — pass through.
                buf.extend_from_slice(&[other]);
                i += 1;
            }
        }
    }
    Bytes::from(buf)
}

/// Infer a structural `LuaType::Table` from a table constructor expression.
///
/// Walks `NameKey` fields and infers each value's type from:
/// - Local variable references (uses `inferred_type`)
/// - Global variable references (uses `GlobalTypeMap`)
///
/// Returns `None` if the constructor has no named fields with
/// inferrable types.
fn infer_table_constructor_type(
    tc: &ast::TableConstructor,
    compiler: &FnCompiler<'_>,
) -> Option<shingetsu_vm::types::LuaType> {
    let mut fields: Vec<(Bytes, shingetsu_vm::types::LuaType)> = Vec::new();
    for field in tc.fields().iter() {
        if let ast::Field::NameKey { key, value, .. } = field {
            let field_name = tok_str(key);
            if let Some(ty) = infer_expr_type(value, compiler) {
                fields.push((field_name, ty));
            }
        }
    }
    if fields.is_empty() {
        return None;
    }
    Some(shingetsu_vm::types::LuaType::Table(Box::new(
        shingetsu_vm::types::TableLuaType {
            fields,
            indexer: None,
        },
    )))
}

/// Infer the type of an expression from compile-time information.
///
/// Handles variable references (locals and globals) and table field
/// access (`t.field`).  Returns `None` when the type cannot be
/// determined.
fn infer_expr_type(
    expr: &ast::Expression,
    compiler: &FnCompiler<'_>,
) -> Option<shingetsu_vm::types::LuaType> {
    match expr {
        ast::Expression::Var(ast::Var::Name(tok)) => {
            let name = tok_str(tok);
            if let Some(local) = compiler.scope.resolve(&name) {
                local.inferred_type.clone()
            } else {
                compiler.compiler.global_types.get(&name).cloned()
            }
        }
        ast::Expression::Var(ast::Var::Expression(ve)) => {
            // Handle `t.field` — resolve the receiver then look up the field.
            let receiver_name = match ve.prefix() {
                ast::Prefix::Name(tok) => tok_str(tok),
                _ => return None,
            };
            let suffixes: Vec<_> = ve.suffixes().collect();
            if suffixes.len() != 1 {
                return None;
            }
            let field_name = match &suffixes[0] {
                ast::Suffix::Index(ast::Index::Dot { name, .. }) => tok_str(name),
                _ => return None,
            };
            let receiver_type = if let Some(local) = compiler.scope.resolve(&receiver_name) {
                local.inferred_type.as_ref()
            } else {
                compiler.compiler.global_types.get(&receiver_name)
            }?;
            lookup_table_field(receiver_type, &field_name)
        }
        _ => None,
    }
}

/// Look up a named field on a table type.
fn lookup_table_field(
    ty: &shingetsu_vm::types::LuaType,
    field_name: &[u8],
) -> Option<shingetsu_vm::types::LuaType> {
    match ty {
        shingetsu_vm::types::LuaType::Table(t) => t
            .fields
            .iter()
            .find(|(n, _)| n.as_ref() == field_name)
            .map(|(_, ty)| ty.clone()),
        _ => None,
    }
}
