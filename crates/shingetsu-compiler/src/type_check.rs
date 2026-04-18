use bytes::Bytes;
use full_moon::ast;
use shingetsu_vm::types::{FunctionLuaType, LuaType};

use crate::error::{Diagnostic, Severity, SourceLocation};
use crate::lower::tok_str;
use crate::Compiler;

/// Run the type checker over a parsed AST and return any diagnostics.
///
/// Currently checks argument counts for function calls where the callee
/// has a known type (globals with inferred types from `GlobalTypeMap`).
pub fn check(ast: &ast::Ast, compiler: &Compiler) -> Vec<Diagnostic> {
    let mut checker = TypeChecker {
        compiler,
        diagnostics: Vec::new(),
    };
    checker.check_block(ast.nodes());
    checker.diagnostics
}

struct TypeChecker<'a> {
    compiler: &'a Compiler,
    diagnostics: Vec<Diagnostic>,
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
            }
            ast::Stmt::Assignment(a) => {
                for expr in a.expressions().iter() {
                    self.check_expr(expr);
                }
            }
            ast::Stmt::Do(d) => self.check_block(d.block()),
            ast::Stmt::While(w) => {
                self.check_expr(w.condition());
                self.check_block(w.block());
            }
            ast::Stmt::Repeat(r) => {
                self.check_block(r.block());
                self.check_expr(r.until());
            }
            ast::Stmt::If(i) => {
                self.check_expr(i.condition());
                self.check_block(i.block());
                if let Some(else_ifs) = i.else_if() {
                    for else_if in else_ifs {
                        self.check_expr(else_if.condition());
                        self.check_block(else_if.block());
                    }
                }
                if let Some(else_block) = i.else_block() {
                    self.check_block(else_block);
                }
            }
            ast::Stmt::NumericFor(nf) => {
                self.check_expr(nf.start());
                self.check_expr(nf.end());
                if let Some(step) = nf.step() {
                    self.check_expr(step);
                }
                self.check_block(nf.block());
            }
            ast::Stmt::GenericFor(gf) => {
                for expr in gf.expressions().iter() {
                    self.check_expr(expr);
                }
                self.check_block(gf.block());
            }
            ast::Stmt::LocalFunction(lf) => {
                self.check_block(lf.body().block());
            }
            ast::Stmt::FunctionDeclaration(fd) => {
                self.check_block(fd.body().block());
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
        let min_params = expected_params.len();
        let is_variadic = func_type.variadic.is_some();

        if is_variadic {
            // Variadic: at least `min_params` required.
            if explicit_count < min_params {
                self.emit_arg_count_diagnostic(fc, call_suffix, min_params, explicit_count, true);
            }
        } else if explicit_count != min_params {
            self.emit_arg_count_diagnostic(fc, call_suffix, min_params, explicit_count, false);
        }
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
                // `receiver:method(args)` — look up receiver in globals,
                // then find the method field.
                let receiver_name = match prefix {
                    ast::Prefix::Name(tok) => tok_str(tok),
                    _ => return None,
                };
                // Only handle simple `t:method()`, not chained `t.x:method()`.
                if !index_suffixes.is_empty() {
                    return None;
                }
                let receiver_type = self.compiler.global_types.get(&receiver_name)?;
                let method_name = tok_str(mc.name());
                self.lookup_function_field(&receiver_type, &method_name)
            }
            ast::Call::AnonymousCall(_) => {
                if index_suffixes.is_empty() {
                    // Simple call: `f(args)` — look up `f` in globals.
                    let name = match prefix {
                        ast::Prefix::Name(tok) => tok_str(tok),
                        _ => return None,
                    };
                    let global_type = self.compiler.global_types.get(&name)?;
                    match global_type {
                        LuaType::Function(f) => Some(f.as_ref().clone()),
                        _ => None,
                    }
                } else if index_suffixes.len() == 1 {
                    // `t.field(args)` — look up `t` in globals, then find field.
                    let receiver_name = match prefix {
                        ast::Prefix::Name(tok) => tok_str(tok),
                        _ => return None,
                    };
                    let field_name = match index_suffixes[0] {
                        ast::Suffix::Index(ast::Index::Dot { name, .. }) => tok_str(name),
                        _ => return None,
                    };
                    let receiver_type = self.compiler.global_types.get(&receiver_name)?;
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
        expected: usize,
        got: usize,
        is_variadic: bool,
    ) {
        // Point the diagnostic at the arguments (parentheses).
        let loc = self
            .call_args_location(call_suffix)
            .unwrap_or_else(|| self.function_call_location(fc));

        let expected_str = if is_variadic {
            format!("at least {expected}")
        } else {
            expected.to_string()
        };

        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            location: loc,
            message: format!(
                "expected {expected_str} argument{} but got {got}",
                if expected == 1 { "" } else { "s" },
            ),
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
