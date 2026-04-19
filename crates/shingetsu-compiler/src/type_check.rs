use bytes::Bytes;
use full_moon::ast;
use shingetsu_vm::types::{FunctionLuaType, LuaType, TypeAlias};

use crate::error::{Diagnostic, LintId, Severity, SourceLocation};
use crate::lower::tok_str;
use crate::util::plural;
use crate::Compiler;

/// Run the type checker over a parsed AST and return any diagnostics.
///
/// Checks argument counts for function calls where the callee has a
/// known type — either from `GlobalTypeMap` or from a local variable's
/// type annotation / inferred type.
pub fn check(ast: &ast::Ast, compiler: &Compiler) -> Vec<Diagnostic> {
    let mut checker = TypeChecker {
        compiler,
        diagnostics: Vec::new(),
        scopes: vec![std::collections::HashMap::new()],
        type_aliases: std::collections::HashMap::new(),
    };
    checker.check_block(ast.nodes());
    checker.diagnostics
}

struct TypeChecker<'a> {
    compiler: &'a Compiler,
    diagnostics: Vec<Diagnostic>,
    /// Stack of scopes mapping local variable names to their types.
    scopes: Vec<std::collections::HashMap<Bytes, LuaType>>,
    /// Type aliases from `type Name = ...` declarations.
    type_aliases: std::collections::HashMap<Bytes, TypeAlias>,
}

impl TypeChecker<'_> {
    fn push_scope(&mut self) {
        self.scopes.push(std::collections::HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Declare a local with an optional type.
    fn declare_local(&mut self, name: Bytes, ty: Option<LuaType>) {
        if let Some(ty) = ty {
            if let Some(scope) = self.scopes.last_mut() {
                scope.insert(name, ty);
            }
        }
    }

    /// Look up a local's type by name, searching from innermost scope.
    fn resolve_local(&self, name: &[u8]) -> Option<&LuaType> {
        for scope in self.scopes.iter().rev() {
            if let Some(ty) = scope.get(name) {
                return Some(ty);
            }
        }
        None
    }

    /// Look up a local's type mutably, for in-place updates.
    fn resolve_local_mut(&mut self, name: &[u8]) -> Option<&mut LuaType> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(ty) = scope.get_mut(name) {
                return Some(ty);
            }
        }
        None
    }
}

impl<'a> TypeChecker<'a> {
    fn check_block(&mut self, block: &ast::Block) {
        for stmt in block.stmts() {
            self.check_stmt(stmt);
        }
        if let Some(last) = block.last_stmt() {
            self.check_last_stmt(last);
        }
    }

    fn check_last_stmt(&mut self, stmt: &ast::LastStmt) {
        match stmt {
            ast::LastStmt::Return(r) => {
                for expr in r.returns().iter() {
                    self.check_expr(expr);
                }
            }
            ast::LastStmt::Break(_) | ast::LastStmt::Continue(_) => {}
            _ => {}
        }
    }

    fn check_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::FunctionCall(fc) => self.check_function_call(fc),
            ast::Stmt::LocalAssignment(la) => {
                // Check function calls in RHS expressions.
                for expr in la.expressions().iter() {
                    self.check_expr(expr);
                }
                // Track local variable types from annotations.
                self.track_local_assignment(la);
            }
            ast::Stmt::Assignment(a) => {
                for expr in a.expressions().iter() {
                    self.check_expr(expr);
                }
            }
            ast::Stmt::Do(d) => {
                self.push_scope();
                self.check_block(d.block());
                self.pop_scope();
            }
            ast::Stmt::While(w) => {
                self.check_expr(w.condition());
                self.push_scope();
                self.check_block(w.block());
                self.pop_scope();
            }
            ast::Stmt::Repeat(r) => {
                self.push_scope();
                self.check_block(r.block());
                self.check_expr(r.until());
                self.pop_scope();
            }
            ast::Stmt::If(i) => {
                self.check_expr(i.condition());
                self.push_scope();
                self.check_block(i.block());
                self.pop_scope();
                if let Some(else_ifs) = i.else_if() {
                    for else_if in else_ifs {
                        self.check_expr(else_if.condition());
                        self.push_scope();
                        self.check_block(else_if.block());
                        self.pop_scope();
                    }
                }
                if let Some(else_block) = i.else_block() {
                    self.push_scope();
                    self.check_block(else_block);
                    self.pop_scope();
                }
            }
            ast::Stmt::NumericFor(nf) => {
                self.check_expr(nf.start());
                self.check_expr(nf.end());
                if let Some(step) = nf.step() {
                    self.check_expr(step);
                }
                self.push_scope();
                self.check_block(nf.block());
                self.pop_scope();
            }
            ast::Stmt::GenericFor(gf) => {
                for expr in gf.expressions().iter() {
                    self.check_expr(expr);
                }
                self.push_scope();
                self.check_block(gf.block());
                self.pop_scope();
            }
            ast::Stmt::LocalFunction(lf) => {
                self.track_local_function(lf);
                self.push_scope();
                self.check_block(lf.body().block());
                self.pop_scope();
            }
            ast::Stmt::FunctionDeclaration(fd) => {
                self.track_function_decl(fd);
                self.push_scope();
                self.check_block(fd.body().block());
                self.pop_scope();
            }
            ast::Stmt::TypeDeclaration(td) => {
                self.track_type_declaration(td, false);
            }
            ast::Stmt::ExportedTypeDeclaration(etd) => {
                self.track_type_declaration(etd.type_declaration(), true);
            }
            ast::Stmt::CompoundAssignment(ca) => {
                self.check_expr(ca.rhs());
            }
            _ => {}
        }
    }

    fn check_expr(&mut self, expr: &ast::Expression) {
        match expr {
            ast::Expression::FunctionCall(fc) => self.check_function_call(fc),
            ast::Expression::Parentheses { expression, .. } => self.check_expr(expression),
            ast::Expression::UnaryOperator { expression, .. } => self.check_expr(expression),
            ast::Expression::BinaryOperator { lhs, rhs, .. } => {
                self.check_expr(lhs);
                self.check_expr(rhs);
            }
            ast::Expression::IfExpression(ie) => {
                self.check_expr(ie.condition());
                self.check_expr(ie.if_expression());
                self.check_expr(ie.else_expression());
            }
            ast::Expression::Function(f) => {
                self.check_block(f.body().block());
            }
            ast::Expression::TableConstructor(tc) => {
                for field in tc.fields().iter() {
                    match field {
                        ast::Field::NoKey(expr) => self.check_expr(expr),
                        ast::Field::NameKey { value, .. } => self.check_expr(value),
                        ast::Field::ExpressionKey { key, value, .. } => {
                            self.check_expr(key);
                            self.check_expr(value);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// Track type aliases from `type Name = ...` declarations.
    fn track_type_declaration(
        &mut self,
        td: &full_moon::ast::luau::TypeDeclaration,
        exported: bool,
    ) {
        let name = Bytes::from(tok_str(td.type_name()));
        let generic_params = td
            .generics()
            .map(crate::type_convert::convert_generic_declaration)
            .unwrap_or_default();
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

    /// Track local variable declarations and their types.
    fn track_local_assignment(&mut self, la: &ast::LocalAssignment) {
        let names: Vec<_> = la.names().iter().collect();
        let type_specs: Vec<_> = la.type_specifiers().collect();

        for (i, name_tok) in names.iter().enumerate() {
            let name = tok_str(name_tok);
            // Prefer explicit type annotation.
            if let Some(Some(ts)) = type_specs.get(i) {
                let lua_type = crate::type_convert::convert_type_specifier_ctx(
                    ts,
                    &crate::type_convert::TypeContext::with_aliases(&[], &self.type_aliases),
                );
                self.declare_local(name, Some(lua_type));
            } else if let Some(expr) = la.expressions().iter().nth(i) {
                // Check for `require("module")` — look up cached type info
                // from the module type registry (populated by the lowerer).
                if let Some(mod_name) = crate::lower::extract_require_literal(expr) {
                    if let Some(info) = self.compiler.module_types().get(mod_name.as_bytes()) {
                        // Import exported types as type aliases.
                        for (type_name, alias) in &info.exported_types {
                            self.type_aliases.insert(type_name.clone(), alias.clone());
                        }
                        // Set the local's type from the module's return type.
                        self.declare_local(name, info.return_type.clone());
                    }
                } else if let ast::Expression::Var(ast::Var::Name(tok)) = expr {
                    // Infer from RHS when it's a global reference.
                    let rhs_name = tok_str(tok);
                    if self.resolve_local(&rhs_name).is_none() {
                        if let Some(ty) = self.compiler.global_types.get(&rhs_name) {
                            self.declare_local(name, Some(ty.clone()));
                        }
                    }
                } else if matches!(expr, ast::Expression::TableConstructor(_)) {
                    self.declare_local(
                        name,
                        Some(LuaType::Table(Box::new(
                            shingetsu_vm::types::TableLuaType {
                                fields: vec![],
                                indexer: None,
                            },
                        ))),
                    );
                }
            }
        }
    }

    /// Track `local function f(...)` declarations by inferring a
    /// `LuaType::Function` from the parameter and return annotations.
    /// Only sets a type when at least one annotation is present —
    /// fully untyped functions should not trigger arg-count checks.
    fn track_local_function(&mut self, lf: &ast::LocalFunction) {
        let name = tok_str(lf.name());
        let body = lf.body();
        let type_specs: Vec<_> = body.type_specifiers().collect();
        let has_any_annotation =
            type_specs.iter().any(|ts| ts.is_some()) || body.return_type().is_some();
        if !has_any_annotation {
            return;
        }
        let type_ctx = crate::type_convert::TypeContext::with_aliases(&[], &self.type_aliases);
        let params: Vec<(Option<Bytes>, LuaType)> = body
            .parameters()
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p {
                ast::Parameter::Name(tok) => {
                    let pname = tok_str(tok);
                    let lua_type = type_specs
                        .get(i)
                        .and_then(|opt| opt.as_ref())
                        .map(|ts| crate::type_convert::convert_type_specifier_ctx(ts, &type_ctx))
                        .unwrap_or(LuaType::Any);
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
        let func_type = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params,
            variadic: if variadic {
                Some(Box::new(LuaType::Any))
            } else {
                None
            },
            returns,
            is_method,
            inferred_unannotated: false,
        }));
        self.declare_local(name, Some(func_type));
    }

    /// Track `function t.f()` / `function t:m()` declarations by
    /// accumulating function types into the local's table type.
    fn track_function_decl(&mut self, fd: &ast::FunctionDeclaration) {
        let func_name = fd.name();
        let names: Vec<_> = func_name.names().iter().collect();
        let is_method = func_name.method_name().is_some();
        let is_single_level = if is_method {
            names.len() == 1
        } else {
            names.len() == 2
        };
        if !is_single_level {
            return;
        }
        let root = tok_str(names[0]);
        let field_name = if let Some(mname) = func_name.method_name() {
            tok_str(mname)
        } else {
            tok_str(names.last().expect("at least two names"))
        };

        // Build the function type from the declaration's signature.
        let body = fd.body();
        let type_specs: Vec<_> = body.type_specifiers().collect();
        let has_any_annotation =
            type_specs.iter().any(|ts| ts.is_some()) || body.return_type().is_some();
        let type_ctx = crate::type_convert::TypeContext::with_aliases(&[], &self.type_aliases);
        let mut params: Vec<(Option<Bytes>, LuaType)> = Vec::new();
        let mut variadic = false;
        if is_method {
            params.push((Some(Bytes::from_static(b"self")), LuaType::Any));
        }
        for (i, param) in body.parameters().iter().enumerate() {
            match param {
                ast::Parameter::Name(tok) => {
                    let pname = tok_str(tok);
                    let lua_type = type_specs
                        .get(i)
                        .and_then(|opt| opt.as_ref())
                        .map(|ts| crate::type_convert::convert_type_specifier_ctx(ts, &type_ctx))
                        .unwrap_or(LuaType::Any);
                    params.push((Some(pname), lua_type));
                }
                ast::Parameter::Ellipsis(_) => {
                    variadic = true;
                }
                _ => {}
            }
        }
        let returns = body
            .return_type()
            .map(|ts| crate::type_convert::convert_return_type_ctx(ts, &type_ctx))
            .unwrap_or_default();
        let func_type = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params,
            variadic: if variadic {
                Some(Box::new(LuaType::Any))
            } else {
                None
            },
            returns,
            is_method,
            inferred_unannotated: !has_any_annotation,
        }));

        // Find the local and accumulate the field.
        let local_type = self.resolve_local_mut(&root);
        if let Some(LuaType::Table(table_type)) = local_type {
            if let Some(existing) = table_type.fields.iter_mut().find(|(n, _)| n == &field_name) {
                existing.1 = func_type;
            } else {
                table_type.fields.push((field_name, func_type));
            }
        }
    }

    fn check_function_call(&mut self, fc: &ast::FunctionCall) {
        // Walk into any nested function calls in the arguments first.
        let suffixes: Vec<_> = fc.suffixes().collect();
        let call_suffix = match suffixes.last() {
            Some(ast::Suffix::Call(c)) => c,
            _ => return,
        };
        let explicit_args = match call_suffix {
            ast::Call::AnonymousCall(a) => a,
            ast::Call::MethodCall(mc) => mc.args(),
            _ => return,
        };
        // Recurse into argument expressions.
        if let ast::FunctionArgs::Parentheses { arguments, .. } = explicit_args {
            for arg in arguments.iter() {
                self.check_expr(arg);
            }
        }

        // Now check the call itself.
        let index_suffixes = &suffixes[..suffixes.len() - 1];

        // Resolve the callee's function type.
        let func_type = self.resolve_callee_type(fc.prefix(), index_suffixes, call_suffix);
        let func_type = match func_type {
            Some(ft) => ft,
            None => return,
        };

        // Skip untyped functions (generic `(...any) -> ()` signatures).
        if func_type.is_untyped() {
            return;
        }

        // Count explicit arguments.
        let explicit_count = self.count_explicit_args(explicit_args);

        // If argument count is unknown (vararg or call expansion as last arg),
        // we can't check.
        let explicit_count = match explicit_count {
            Some(n) => n,
            None => return,
        };

        // For method calls (`:` syntax), the receiver is passed implicitly
        // as `self`, so we skip the self param when counting.  For dot
        // calls on methods, the caller must pass self explicitly — count
        // all params.
        let is_colon_call = matches!(call_suffix, ast::Call::MethodCall(_));
        let expected_params: Vec<_> = if func_type.is_method && is_colon_call {
            func_type.params.iter().skip(1).collect()
        } else {
            func_type.params.iter().collect()
        };
        let max_params = expected_params.len();
        let min_params = expected_params
            .iter()
            .take_while(|(_, ty)| !matches!(ty, shingetsu_vm::types::LuaType::Optional(_)))
            .count();
        let is_variadic = func_type.variadic.is_some();

        let unannotated = func_type.inferred_unannotated;
        if is_variadic {
            // Variadic: at least `min_params` required, no upper bound.
            if explicit_count < min_params {
                self.emit_arg_count_diagnostic(
                    fc,
                    call_suffix,
                    min_params,
                    min_params,
                    explicit_count,
                    true,
                    unannotated,
                );
            }
        } else if explicit_count < min_params || explicit_count > max_params {
            self.emit_arg_count_diagnostic(
                fc,
                call_suffix,
                min_params,
                max_params,
                explicit_count,
                false,
                unannotated,
            );
        }
    }

    /// Look up a name's type, checking locals first, then globals.
    fn resolve_name_type(&self, name: &[u8]) -> Option<LuaType> {
        if let Some(ty) = self.resolve_local(name) {
            return Some(ty.clone());
        }
        self.compiler.global_types.get(name).cloned()
    }

    /// Resolve the function type of the callee for a function call.
    /// Returns `None` if the callee's type cannot be determined.
    fn resolve_callee_type(
        &self,
        prefix: &ast::Prefix,
        index_suffixes: &[&ast::Suffix],
        call_suffix: &ast::Call,
    ) -> Option<FunctionLuaType> {
        match call_suffix {
            ast::Call::MethodCall(mc) => {
                // `receiver:method(args)` — look up receiver in locals
                // or globals, then find the method field.
                let receiver_name = match prefix {
                    ast::Prefix::Name(tok) => tok_str(tok),
                    _ => return None,
                };
                // Only handle simple `t:method()`, not chained `t.x:method()`.
                if !index_suffixes.is_empty() {
                    return None;
                }
                let receiver_type = self.resolve_name_type(&receiver_name)?;
                let method_name = tok_str(mc.name());
                self.lookup_function_field(&receiver_type, &method_name)
            }
            ast::Call::AnonymousCall(_) => {
                if index_suffixes.is_empty() {
                    // Simple call: `f(args)` — look up `f` in locals or globals.
                    let name = match prefix {
                        ast::Prefix::Name(tok) => tok_str(tok),
                        _ => return None,
                    };
                    let callee_type = self.resolve_name_type(&name)?;
                    match callee_type {
                        LuaType::Function(f) => Some(f.as_ref().clone()),
                        _ => None,
                    }
                } else if index_suffixes.len() == 1 {
                    // `t.field(args)` — look up `t` in locals or globals,
                    // then find field.
                    let receiver_name = match prefix {
                        ast::Prefix::Name(tok) => tok_str(tok),
                        _ => return None,
                    };
                    let field_name = match index_suffixes[0] {
                        ast::Suffix::Index(ast::Index::Dot { name, .. }) => tok_str(name),
                        _ => return None,
                    };
                    let receiver_type = self.resolve_name_type(&receiver_name)?;
                    self.lookup_function_field(&receiver_type, &field_name)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Look up a field on a table type and return the function type if found.
    fn lookup_function_field(&self, ty: &LuaType, field_name: &Bytes) -> Option<FunctionLuaType> {
        let table = match ty {
            LuaType::Table(t) => t,
            _ => return None,
        };
        for (name, field_ty) in &table.fields {
            if name == field_name {
                if let LuaType::Function(f) = field_ty {
                    return Some(f.as_ref().clone());
                }
                return None;
            }
        }
        None
    }

    /// Count the explicit arguments in a function call.
    /// Returns `None` if the count is indeterminate (vararg or multi-return
    /// call expansion as the last argument).
    fn count_explicit_args(&self, args: &ast::FunctionArgs) -> Option<usize> {
        match args {
            ast::FunctionArgs::Parentheses { arguments, .. } => {
                let arg_list: Vec<_> = arguments.iter().collect();
                // If the last argument is `...` or a function call,
                // the argument count is indeterminate.
                if let Some(last) = arg_list.last() {
                    if is_vararg_expr(last) || matches!(last, ast::Expression::FunctionCall(_)) {
                        return None;
                    }
                }
                Some(arg_list.len())
            }
            ast::FunctionArgs::String(_) => Some(1),
            ast::FunctionArgs::TableConstructor(_) => Some(1),
            _ => None,
        }
    }

    fn emit_arg_count_diagnostic(
        &mut self,
        fc: &ast::FunctionCall,
        call_suffix: &ast::Call,
        min_params: usize,
        max_params: usize,
        got: usize,
        is_variadic: bool,
        inferred_unannotated: bool,
    ) {
        // Point the diagnostic at the arguments (parentheses).
        let loc = self
            .call_args_location(call_suffix)
            .unwrap_or_else(|| self.function_call_location(fc));

        let expected_str = if is_variadic {
            format!("at least {min_params}")
        } else if min_params == max_params {
            min_params.to_string()
        } else if got < min_params {
            format!("at least {min_params}")
        } else {
            format!("at most {max_params}")
        };

        let reference_count = if got < min_params {
            min_params
        } else {
            max_params
        };

        let severity = if inferred_unannotated {
            Severity::Warning
        } else {
            Severity::Error
        };

        self.diagnostics.push(Diagnostic {
            lint: LintId::ArgCount,
            severity,
            location: loc,
            message: format!(
                "expected {expected_str} {} but got {got}",
                plural(reference_count, "argument")
            ),
            help: None,
        });
    }

    /// Get the source location of the arguments portion of a call.
    fn call_args_location(&self, call_suffix: &ast::Call) -> Option<SourceLocation> {
        let args = match call_suffix {
            ast::Call::AnonymousCall(a) => a,
            ast::Call::MethodCall(mc) => mc.args(),
            _ => return None,
        };
        match args {
            ast::FunctionArgs::Parentheses {
                parentheses: parens,
                ..
            } => {
                use full_moon::node::Node;
                let start = Node::start_position(parens.tokens().0)?;
                let end = Node::end_position(parens.tokens().1)?;
                Some(SourceLocation::from_span(
                    &self.compiler.opts.source_name,
                    start,
                    end,
                ))
            }
            _ => None,
        }
    }

    /// Get the source location of a function call expression.
    fn function_call_location(&self, fc: &ast::FunctionCall) -> SourceLocation {
        use full_moon::node::Node;
        match Node::start_position(fc) {
            Some(pos) => SourceLocation::from_pos(&self.compiler.opts.source_name, pos),
            None => SourceLocation::unknown(&self.compiler.opts.source_name),
        }
    }
}

/// Returns `true` if the expression is a `...` vararg.
fn is_vararg_expr(expr: &ast::Expression) -> bool {
    matches!(expr, ast::Expression::Symbol(tok) if tok.token().to_string() == "...")
}
