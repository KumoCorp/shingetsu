use bytes::Bytes;
use full_moon::ast;
use shingetsu_vm::types::{FunctionLuaType, LuaType, TypeAlias};

use crate::error::{Diagnostic, LintId, Severity, SourceLocation};
use crate::lower::{parse_string_literal, tok_str};
use crate::util::plural;
use crate::Compiler;

/// Extract the display name from a type annotation, if it is a simple
/// named reference (e.g. `Point`). Returns `None` for complex types
/// like inline table types or unions.
fn annotation_display_name(ts: &full_moon::ast::luau::TypeSpecifier) -> Option<Bytes> {
    match ts.type_info() {
        full_moon::ast::luau::TypeInfo::Basic(tok) => {
            let name = tok_str(tok);
            // Don't treat built-in primitive names as display names.
            match name.as_ref() {
                b"number" | b"integer" | b"float" | b"string" | b"boolean" | b"nil" | b"any"
                | b"unknown" | b"never" | b"Table" => None,
                _ => Some(name),
            }
        }
        _ => None,
    }
}

/// Format the display label for a type in diagnostics.
/// Uses the alias/variable name if available, otherwise falls back
/// to the `DisplayLuaType` representation.
fn type_display_label(display_name: &Option<Bytes>, ty: &LuaType) -> String {
    match display_name {
        Some(name) => format!("{}", bstr::BStr::new(name)),
        None => format!("{}", DisplayLuaType(ty)),
    }
}

/// Format a qualified field name for diagnostics using Lua dot notation.
/// Uses the type alias name if available, otherwise the receiver variable name.
fn qualified_field_name(
    type_display: &Option<Bytes>,
    receiver_name: &[u8],
    field_name: &[u8],
) -> String {
    let prefix = match type_display {
        Some(name) => bstr::BStr::new(name).to_string(),
        None => bstr::BStr::new(receiver_name).to_string(),
    };
    format!("{prefix}.{}", bstr::BStr::new(field_name))
}

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
        expected_returns: Vec::new(),
    };
    checker.check_block(ast.nodes());
    checker.diagnostics
}

/// A local variable's type info, including an optional display name
/// for use in diagnostics (e.g. the type alias name "Point" rather
/// than the resolved "table").
#[derive(Clone)]
struct LocalTypeInfo {
    ty: LuaType,
    display_name: Option<Bytes>,
}

struct TypeChecker<'a> {
    compiler: &'a Compiler,
    diagnostics: Vec<Diagnostic>,
    /// Stack of scopes mapping local variable names to their types.
    scopes: Vec<std::collections::HashMap<Bytes, LocalTypeInfo>>,
    /// Type aliases from `type Name = ...` declarations.
    type_aliases: std::collections::HashMap<Bytes, TypeAlias>,
    /// Stack of expected return types for the enclosing function.
    /// Empty vec means no return type declared (don't check).
    expected_returns: Vec<Vec<LuaType>>,
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
        self.declare_local_named(name, ty, None);
    }

    /// Declare a local with an optional type and an explicit display name.
    fn declare_local_named(
        &mut self,
        name: Bytes,
        ty: Option<LuaType>,
        display_name: Option<Bytes>,
    ) {
        if let Some(ty) = ty {
            if let Some(scope) = self.scopes.last_mut() {
                scope.insert(name, LocalTypeInfo { ty, display_name });
            }
        }
    }

    /// Look up a local's type by name, searching from innermost scope.
    fn resolve_local(&self, name: &[u8]) -> Option<&LuaType> {
        self.resolve_local_info(name).map(|info| &info.ty)
    }

    /// Look up a local's full type info by name.
    fn resolve_local_info(&self, name: &[u8]) -> Option<&LocalTypeInfo> {
        for scope in self.scopes.iter().rev() {
            if let Some(info) = scope.get(name) {
                return Some(info);
            }
        }
        None
    }

    /// Look up a local's type mutably, for in-place updates.
    fn resolve_local_mut(&mut self, name: &[u8]) -> Option<&mut LuaType> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(info) = scope.get_mut(name) {
                return Some(&mut info.ty);
            }
        }
        None
    }
}

impl<'a> TypeChecker<'a> {
    fn check_block(&mut self, block: &ast::Block) {
        let mut terminated_by_never = false;
        for stmt in block.stmts() {
            if terminated_by_never {
                self.emit_unreachable(stmt);
            }
            self.check_stmt(stmt);
            if !terminated_by_never {
                // Only track never-returning calls here; the lowerer
                // already detects unreachable code after return/break
                // and fully-terminating if/else.
                if let ast::Stmt::FunctionCall(fc) = stmt {
                    if self.call_returns_never(fc) {
                        terminated_by_never = true;
                    }
                }
            }
        }
        if let Some(last) = block.last_stmt() {
            if terminated_by_never {
                self.emit_unreachable(last);
            }
            self.check_last_stmt(last);
        }
    }

    fn check_last_stmt(&mut self, stmt: &ast::LastStmt) {
        match stmt {
            ast::LastStmt::Return(r) => {
                for expr in r.returns().iter() {
                    self.check_expr(expr);
                }
                self.check_return_types(r);
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
                self.check_function_body(lf.body());
                self.pop_scope();
            }
            ast::Stmt::FunctionDeclaration(fd) => {
                self.track_function_decl(fd);
                self.push_scope();
                self.check_function_body(fd.body());
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
            ast::Expression::Var(ast::Var::Expression(ve)) => self.check_var_expression(ve),
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
                self.check_function_body(f.body());
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
                let display_name = annotation_display_name(ts);
                // Check assignment compatibility when both annotation
                // and RHS expression type are known.
                if !matches!(lua_type, LuaType::Any | LuaType::Unknown) {
                    if let Some(expr) = la.expressions().iter().nth(i) {
                        if let Some(actual) = self.infer_expr_type(expr) {
                            if !types_compatible(&lua_type, &actual) {
                                self.diagnostics.push(Diagnostic {
                                    lint: LintId::AssignType,
                                    severity: Severity::Error,
                                    location: self.expr_location(expr),
                                    message: format!(
                                        "expected '{}' but got '{}'",
                                        DisplayLuaType(&lua_type),
                                        DisplayLuaType(&actual),
                                    ),
                                    help: None,
                                });
                            }
                        }
                    }
                }
                self.declare_local_named(name, Some(lua_type), display_name);
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
                } else if let Some(ty) = self.infer_expr_type(expr) {
                    self.declare_local(name, Some(ty));
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
            None => {
                self.check_not_callable(fc.prefix(), index_suffixes, call_suffix);
                return;
            }
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

        // Check argument types against parameter types.
        if let ast::FunctionArgs::Parentheses { arguments, .. } = explicit_args {
            let args: Vec<_> = arguments.iter().collect();
            for (i, param) in expected_params.iter().enumerate() {
                let arg_expr = match args.get(i) {
                    Some(expr) => expr,
                    None => break,
                };
                let param_type = &param.1;
                if matches!(param_type, LuaType::Any | LuaType::Unknown) {
                    continue;
                }
                let arg_type = match self.infer_expr_type(arg_expr) {
                    Some(ty) => ty,
                    None => continue,
                };
                if !types_compatible(param_type, &arg_type) {
                    let param_label = param
                        .0
                        .as_ref()
                        .map(|n| format!(" '{}'", bstr::BStr::new(n)))
                        .unwrap_or_default();
                    let severity = if unannotated {
                        Severity::Warning
                    } else {
                        Severity::Error
                    };
                    let loc = self.expr_location(arg_expr);
                    self.diagnostics.push(Diagnostic {
                        lint: LintId::ArgType,
                        severity,
                        location: loc,
                        message: format!(
                            "expected '{}' for parameter{param_label} but got '{}'",
                            DisplayLuaType(param_type),
                            DisplayLuaType(&arg_type),
                        ),
                        help: None,
                    });
                }
            }
        }
    }

    /// Look up a name's type, checking locals first, then globals.
    fn resolve_name_type(&self, name: &[u8]) -> Option<LuaType> {
        if let Some(ty) = self.resolve_local(name) {
            return Some(ty.clone());
        }
        self.compiler.global_types.get(name).cloned()
    }

    /// Resolve a name's type and its display name for diagnostics.
    /// The display name is the type alias name for locals (e.g. "Point")
    /// or the variable name for globals (e.g. "math").
    fn resolve_name_type_display(&self, name: &[u8]) -> Option<(LuaType, Option<Bytes>)> {
        if let Some(info) = self.resolve_local_info(name) {
            return Some((info.ty.clone(), info.display_name.clone()));
        }
        self.compiler
            .global_types
            .get(name)
            .map(|ty| (ty.clone(), Some(Bytes::copy_from_slice(name))))
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

    /// Look up a field on a table type by name.
    ///
    /// Returns:
    /// - `Some(Some(ty))` if the field exists
    /// - `Some(None)` if the table has known fields but this field is not among them
    /// - `None` if the type is not a table or has no known fields (skip checking)
    fn lookup_field<'b>(&self, ty: &'b LuaType, field_name: &Bytes) -> Option<Option<&'b LuaType>> {
        let table = match ty {
            LuaType::Table(t) => t,
            _ => return None,
        };
        // Tables with no fields are generic — we don't know what fields they have.
        if table.fields.is_empty() {
            return None;
        }
        // Tables with an indexer allow any key matching the indexer type.
        if table.indexer.is_some() {
            return None;
        }
        for (name, field_ty) in &table.fields {
            if name == field_name {
                return Some(Some(field_ty));
            }
        }
        Some(None)
    }

    /// Look up a field on a table type and return the function type if found.
    fn lookup_function_field(&self, ty: &LuaType, field_name: &Bytes) -> Option<FunctionLuaType> {
        match self.lookup_field(ty, field_name) {
            Some(Some(LuaType::Function(f))) => Some(f.as_ref().clone()),
            _ => None,
        }
    }

    /// Extract the field name from an index suffix, if it is a dot access
    /// or a bracket access with a literal string key.
    fn index_field_name(&self, index: &ast::Index) -> Option<Bytes> {
        match index {
            ast::Index::Dot { name, .. } => Some(tok_str(name)),
            ast::Index::Brackets { expression, .. } => {
                if let ast::Expression::String(tok) = expression {
                    Some(parse_string_literal(tok))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Check field access on a variable expression (e.g., `t.foo` or `t["foo"]`).
    fn check_var_expression(&mut self, ve: &ast::VarExpression) {
        let receiver_name = match ve.prefix() {
            ast::Prefix::Name(tok) => tok_str(tok),
            _ => return,
        };
        let (receiver_type, type_display) = match self.resolve_name_type_display(&receiver_name) {
            Some(pair) => pair,
            None => return,
        };
        let suffixes: Vec<_> = ve.suffixes().collect();
        // Only check simple single-level access: `t.foo` or `t["foo"]`.
        if suffixes.len() != 1 {
            return;
        }
        let field_name = match &suffixes[0] {
            ast::Suffix::Index(index) => match self.index_field_name(index) {
                Some(name) => name,
                None => return,
            },
            _ => return,
        };
        match self.lookup_field(&receiver_type, &field_name) {
            Some(None) => {
                use full_moon::node::Node;
                let loc = match (Node::start_position(ve), Node::end_position(ve)) {
                    (Some(start), Some(end)) => {
                        SourceLocation::from_span(&self.compiler.opts.source_name, start, end)
                    }
                    (Some(pos), None) => {
                        SourceLocation::from_pos(&self.compiler.opts.source_name, pos)
                    }
                    _ => SourceLocation::unknown(&self.compiler.opts.source_name),
                };
                let type_label = type_display_label(&type_display, &receiver_type);
                self.diagnostics.push(Diagnostic {
                    lint: LintId::FieldAccess,
                    severity: Severity::Error,
                    location: loc,
                    message: format!(
                        "unknown field '{}' on type '{type_label}'",
                        bstr::BStr::new(&field_name),
                    ),
                    help: None,
                });
            }
            _ => {}
        }
    }

    /// Check that a field being called is actually a function.
    /// Called from `check_function_call` when `resolve_callee_type` returns None.
    fn check_not_callable(
        &mut self,
        prefix: &ast::Prefix,
        index_suffixes: &[&ast::Suffix],
        call_suffix: &ast::Call,
    ) {
        let receiver_name = match prefix {
            ast::Prefix::Name(tok) => tok_str(tok),
            _ => return,
        };
        // Determine the field name from either `t.field()` / `t["field"]()`
        // or `t:method()` patterns.
        let field_name = match call_suffix {
            ast::Call::MethodCall(mc) if index_suffixes.is_empty() => tok_str(mc.name()),
            ast::Call::AnonymousCall(_) if index_suffixes.len() == 1 => match index_suffixes[0] {
                ast::Suffix::Index(index) => match self.index_field_name(index) {
                    Some(name) => name,
                    None => return,
                },
                _ => return,
            },
            _ => return,
        };
        let (receiver_type, type_display) = match self.resolve_name_type_display(&receiver_name) {
            Some(pair) => pair,
            None => return,
        };
        let message = match self.lookup_field(&receiver_type, &field_name) {
            Some(Some(field_ty)) if !matches!(field_ty, LuaType::Function(_)) => {
                let qualified = qualified_field_name(&type_display, &receiver_name, &field_name);
                format!(
                    "field '{qualified}' is not callable (type is '{}')",
                    DisplayLuaType(field_ty),
                )
            }
            Some(None) => {
                let type_label = type_display_label(&type_display, &receiver_type);
                format!(
                    "unknown field '{}' on type '{type_label}'",
                    bstr::BStr::new(&field_name),
                )
            }
            _ => return,
        };
        use full_moon::node::Node;
        let fc_start = Node::start_position(prefix);
        let fc_end = Node::end_position(call_suffix);
        let loc = match (fc_start, fc_end) {
            (Some(start), Some(end)) => {
                SourceLocation::from_span(&self.compiler.opts.source_name, start, end)
            }
            (Some(pos), None) => SourceLocation::from_pos(&self.compiler.opts.source_name, pos),
            _ => SourceLocation::unknown(&self.compiler.opts.source_name),
        };
        self.diagnostics.push(Diagnostic {
            lint: LintId::FieldAccess,
            severity: Severity::Error,
            location: loc,
            message,
            help: None,
        });
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

    /// Push expected return types from a function body's return annotation.
    /// Check a function body: push expected returns, check the block,
    /// verify that all paths return if a return type is declared, then pop.
    fn check_function_body(&mut self, body: &ast::FunctionBody) {
        self.push_expected_returns(body);
        self.check_block(body.block());
        self.check_missing_return(body);
        self.expected_returns.pop();
    }

    /// Emit a diagnostic if the function has a declared return type
    /// but its body can fall off the end without returning.
    fn check_missing_return(&mut self, body: &ast::FunctionBody) {
        let expected = match self.expected_returns.last() {
            Some(e) if !e.is_empty() => e,
            _ => return,
        };
        // Skip if every return type is any/unknown/never.
        if expected
            .iter()
            .all(|t| matches!(t, LuaType::Any | LuaType::Unknown | LuaType::Never))
        {
            return;
        }
        if self.block_always_terminates(body.block()) {
            return;
        }
        // Point the diagnostic at the `end` keyword of the function.
        use full_moon::node::Node;
        let end_tok = body.end_token();
        let loc = match (Node::start_position(end_tok), Node::end_position(end_tok)) {
            (Some(start), Some(end)) => {
                SourceLocation::from_span(&self.compiler.opts.source_name, start, end)
            }
            (Some(pos), None) => SourceLocation::from_pos(&self.compiler.opts.source_name, pos),
            _ => SourceLocation::unknown(&self.compiler.opts.source_name),
        };
        let ret_label = if expected.len() == 1 {
            format!("'{}'", DisplayLuaType(&expected[0]))
        } else {
            let parts: Vec<_> = expected
                .iter()
                .map(|t| format!("'{}'", DisplayLuaType(t)))
                .collect();
            format!("({})", parts.join(", "))
        };
        self.diagnostics.push(Diagnostic {
            lint: LintId::MissingReturn,
            severity: Severity::Error,
            location: loc,
            message: format!("function may fall off the end without returning {ret_label}"),
            help: None,
        });
    }

    /// Check whether a block always terminates (returns or diverges).
    /// A block always terminates if:
    /// - It ends with a `return` statement, OR
    /// - Its last statement is a function call whose return type is `never`, OR
    /// - Its last statement is an `if/elseif/else` where every branch
    ///   (including the else) always terminates, OR
    /// - Its last statement is a `do ... end` whose inner block always terminates.
    fn block_always_terminates(&self, block: &ast::Block) -> bool {
        // A block with an explicit `return` or `break`/`continue` as
        // its last statement always terminates (return means it returns;
        // break/continue exit the enclosing loop).
        if let Some(last) = block.last_stmt() {
            return matches!(last, ast::LastStmt::Return(_));
        }
        // Check every statement, not just the last: a never-returning
        // call or a fully-terminating if/do anywhere in the block means
        // the block cannot fall through (the code after it is unreachable).
        for stmt in block.stmts() {
            let terminates = match &stmt {
                ast::Stmt::If(i) => self.if_always_terminates(i),
                ast::Stmt::Do(d) => self.block_always_terminates(d.block()),
                ast::Stmt::FunctionCall(fc) => self.call_returns_never(fc),
                _ => false,
            };
            if terminates {
                return true;
            }
        }
        false
    }

    /// Check whether an if statement always terminates: every branch
    /// (if, all elseif, and else) must always terminate.
    fn if_always_terminates(&self, i: &ast::If) -> bool {
        // Must have an else branch.
        let else_block = match i.else_block() {
            Some(b) => b,
            None => return false,
        };
        if !self.block_always_terminates(i.block()) {
            return false;
        }
        if let Some(else_ifs) = i.else_if() {
            for else_if in else_ifs {
                if !self.block_always_terminates(else_if.block()) {
                    return false;
                }
            }
        }
        self.block_always_terminates(else_block)
    }

    /// Check whether a function call's return type is `never`.
    fn call_returns_never(&self, fc: &ast::FunctionCall) -> bool {
        let suffixes: Vec<_> = fc.suffixes().collect();
        let call_suffix = match suffixes.last() {
            Some(ast::Suffix::Call(c)) => c,
            _ => return false,
        };
        let index_suffixes = &suffixes[..suffixes.len() - 1];
        let func_type = match self.resolve_callee_type(fc.prefix(), index_suffixes, call_suffix) {
            Some(ft) => ft,
            None => return false,
        };
        matches!(func_type.returns.first(), Some(LuaType::Never))
    }

    fn stmt_always_terminates(&self, stmt: &ast::Stmt) -> bool {
        match stmt {
            ast::Stmt::If(i) => self.if_always_terminates(i),
            ast::Stmt::Do(d) => self.block_always_terminates(d.block()),
            ast::Stmt::FunctionCall(fc) => self.call_returns_never(fc),
            _ => false,
        }
    }

    fn emit_unreachable<N: full_moon::node::Node>(&mut self, node: &N) {
        use full_moon::node::Node;
        let loc = match Node::start_position(node) {
            Some(pos) => SourceLocation::from_pos(&self.compiler.opts.source_name, pos),
            None => return,
        };
        self.diagnostics.push(Diagnostic {
            lint: LintId::UnreachableCode,
            severity: Severity::Warning,
            location: loc,
            message: "unreachable code".to_string(),
            help: None,
        });
    }

    fn push_expected_returns(&mut self, body: &ast::FunctionBody) {
        let returns = match body.return_type() {
            Some(ts) => {
                let ctx = crate::type_convert::TypeContext::with_aliases(&[], &self.type_aliases);
                crate::type_convert::convert_return_type_ctx(ts, &ctx)
            }
            None => vec![],
        };
        self.expected_returns.push(returns);
    }

    /// Check return expressions against the enclosing function's
    /// declared return type.
    fn check_return_types(&mut self, ret: &ast::Return) {
        let expected = match self.expected_returns.last() {
            Some(e) if !e.is_empty() => e.clone(),
            _ => return,
        };
        let return_exprs: Vec<_> = ret.returns().iter().collect();
        for (i, expected_ty) in expected.iter().enumerate() {
            if matches!(expected_ty, LuaType::Any | LuaType::Unknown) {
                continue;
            }
            let actual_ty = match return_exprs.get(i) {
                Some(expr) => match self.infer_expr_type(expr) {
                    Some(ty) => ty,
                    None => continue,
                },
                None => LuaType::Nil,
            };
            if !types_compatible(expected_ty, &actual_ty) {
                let loc = return_exprs
                    .get(i)
                    .map(|e| self.expr_location(e))
                    .unwrap_or_else(|| self.return_keyword_location(ret));
                self.diagnostics.push(Diagnostic {
                    lint: LintId::ReturnType,
                    severity: Severity::Error,
                    location: loc,
                    message: if expected.len() == 1 {
                        format!(
                            "expected return type '{}' but got '{}'",
                            DisplayLuaType(expected_ty),
                            DisplayLuaType(&actual_ty),
                        )
                    } else {
                        format!(
                            "expected return type '{}' at position {} but got '{}'",
                            DisplayLuaType(expected_ty),
                            i + 1,
                            DisplayLuaType(&actual_ty),
                        )
                    },
                    help: None,
                });
            }
        }
    }

    /// Get the source location of the `return` statement.
    fn return_keyword_location(&self, ret: &ast::Return) -> SourceLocation {
        use full_moon::node::Node;
        match (Node::start_position(ret), Node::end_position(ret)) {
            (Some(start), Some(end)) => {
                SourceLocation::from_span(&self.compiler.opts.source_name, start, end)
            }
            (Some(pos), None) => SourceLocation::from_pos(&self.compiler.opts.source_name, pos),
            _ => SourceLocation::unknown(&self.compiler.opts.source_name),
        }
    }

    /// Get the source location of an expression (full span).
    fn expr_location(&self, expr: &ast::Expression) -> SourceLocation {
        use full_moon::node::Node;
        match (Node::start_position(expr), Node::end_position(expr)) {
            (Some(start), Some(end)) => {
                SourceLocation::from_span(&self.compiler.opts.source_name, start, end)
            }
            (Some(pos), None) => SourceLocation::from_pos(&self.compiler.opts.source_name, pos),
            _ => SourceLocation::unknown(&self.compiler.opts.source_name),
        }
    }

    /// Infer the type of an expression, returning `None` when it cannot
    /// be determined.
    fn infer_expr_type(&self, expr: &ast::Expression) -> Option<LuaType> {
        match expr {
            ast::Expression::Number(tok) => {
                let s = tok.token().to_string();
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    Some(LuaType::Float)
                } else {
                    Some(LuaType::Integer)
                }
            }
            ast::Expression::String(_) => Some(LuaType::String),
            ast::Expression::Symbol(tok) => match tok.token().to_string().as_str() {
                "nil" => Some(LuaType::Nil),
                "true" | "false" => Some(LuaType::Boolean),
                _ => None,
            },
            ast::Expression::Var(var) => match var {
                ast::Var::Name(tok) => {
                    let name = tok_str(tok);
                    self.resolve_name_type(&name)
                }
                ast::Var::Expression(ve) => {
                    let receiver_name = match ve.prefix() {
                        ast::Prefix::Name(tok) => tok_str(tok),
                        _ => return None,
                    };
                    let suffixes: Vec<_> = ve.suffixes().collect();
                    if suffixes.len() != 1 {
                        return None;
                    }
                    let field_name = match &suffixes[0] {
                        ast::Suffix::Index(index) => self.index_field_name(index)?,
                        _ => return None,
                    };
                    let receiver_type = self.resolve_name_type(&receiver_name)?;
                    match self.lookup_field(&receiver_type, &field_name) {
                        Some(Some(ty)) => Some(ty.clone()),
                        _ => None,
                    }
                }
                _ => None,
            },
            ast::Expression::Parentheses { expression, .. } => self.infer_expr_type(expression),
            ast::Expression::UnaryOperator { unop, .. } => match unop {
                ast::UnOp::Minus(_) | ast::UnOp::Tilde(_) => Some(LuaType::Number),
                ast::UnOp::Not(_) => Some(LuaType::Boolean),
                ast::UnOp::Hash(_) => Some(LuaType::Integer),
                _ => None,
            },
            ast::Expression::BinaryOperator { binop, lhs, .. } => match binop {
                ast::BinOp::TwoDots(_) => Some(LuaType::String),
                ast::BinOp::Plus(_)
                | ast::BinOp::Minus(_)
                | ast::BinOp::Star(_)
                | ast::BinOp::Slash(_)
                | ast::BinOp::DoubleSlash(_)
                | ast::BinOp::Percent(_)
                | ast::BinOp::Caret(_) => Some(LuaType::Number),
                ast::BinOp::Ampersand(_)
                | ast::BinOp::Pipe(_)
                | ast::BinOp::Tilde(_)
                | ast::BinOp::DoubleLessThan(_)
                | ast::BinOp::DoubleGreaterThan(_) => Some(LuaType::Integer),
                ast::BinOp::TwoEqual(_)
                | ast::BinOp::TildeEqual(_)
                | ast::BinOp::LessThan(_)
                | ast::BinOp::LessThanEqual(_)
                | ast::BinOp::GreaterThan(_)
                | ast::BinOp::GreaterThanEqual(_) => Some(LuaType::Boolean),
                ast::BinOp::And(_) | ast::BinOp::Or(_) => self.infer_expr_type(lhs),
                _ => None,
            },
            ast::Expression::FunctionCall(fc) => {
                let suffixes: Vec<_> = fc.suffixes().collect();
                let call_suffix = match suffixes.last() {
                    Some(ast::Suffix::Call(c)) => c,
                    _ => return None,
                };
                let index_suffixes = &suffixes[..suffixes.len() - 1];
                let func_type =
                    self.resolve_callee_type(fc.prefix(), index_suffixes, call_suffix)?;
                func_type.returns.first().cloned()
            }
            ast::Expression::TableConstructor(_) => Some(LuaType::Table(Box::new(
                shingetsu_vm::types::TableLuaType {
                    fields: vec![],
                    indexer: None,
                },
            ))),
            ast::Expression::Function(_) => Some(LuaType::Function(Box::new(FunctionLuaType {
                type_params: vec![],
                params: vec![],
                variadic: Some(Box::new(LuaType::Any)),
                returns: vec![],
                is_method: false,
                inferred_unannotated: true,
            }))),
            _ => None,
        }
    }
}

/// Returns `true` if the expression is a `...` vararg.
fn is_vararg_expr(expr: &ast::Expression) -> bool {
    matches!(expr, ast::Expression::Symbol(tok) if tok.token().to_string() == "...")
}

/// Check whether `actual` is compatible with `expected`.
fn types_compatible(expected: &LuaType, actual: &LuaType) -> bool {
    if matches!(expected, LuaType::Any | LuaType::Unknown)
        || matches!(actual, LuaType::Any | LuaType::Unknown)
    {
        return true;
    }

    match (expected, actual) {
        // Exact match.
        (a, b) if a == b => true,

        // Number accepts Integer and Float.
        (LuaType::Number, LuaType::Integer | LuaType::Float) => true,
        // Integer and Float also accept Number (Lua coercion).
        (LuaType::Integer | LuaType::Float, LuaType::Number) => true,
        // Float accepts Integer (Lua coerces integers to floats).
        (LuaType::Float, LuaType::Integer) => true,

        // Optional(T) accepts T or Nil.
        (LuaType::Optional(inner), _) => {
            matches!(actual, LuaType::Nil) || types_compatible(inner, actual)
        }

        // Union: actual is compatible if it matches any variant.
        (LuaType::Union(variants), _) => variants.iter().any(|v| types_compatible(v, actual)),

        // Actual is a union: compatible if every variant is compatible.
        (_, LuaType::Union(variants)) => variants.iter().all(|v| types_compatible(expected, v)),

        // Actual is Optional: check inner against expected (nil won't match
        // unless expected also allows it).
        (_, LuaType::Optional(inner)) => {
            types_compatible(expected, &LuaType::Nil) && types_compatible(expected, inner)
        }

        // Named types: nominal equality.
        (LuaType::Named(a), LuaType::Named(b)) => a == b,

        // String literal is a String.
        (LuaType::String, LuaType::StringLiteral(_)) => true,
        (LuaType::StringLiteral(_), LuaType::String) => true,

        // Boolean literal is a Boolean.
        (LuaType::Boolean, LuaType::BoolLiteral(_)) => true,
        (LuaType::BoolLiteral(_), LuaType::Boolean) => true,

        // Table accepts any table.
        (LuaType::Table(_), LuaType::Table(_)) => true,

        // Function accepts any function.
        (LuaType::Function(_), LuaType::Function(_)) => true,

        _ => false,
    }
}

/// Display wrapper for `LuaType` that produces human-readable type names.
struct DisplayLuaType<'a>(&'a LuaType);

impl std::fmt::Display for DisplayLuaType<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            LuaType::Nil => f.write_str("nil"),
            LuaType::Boolean => f.write_str("boolean"),
            LuaType::Number => f.write_str("number"),
            LuaType::Integer => f.write_str("integer"),
            LuaType::Float => f.write_str("float"),
            LuaType::String => f.write_str("string"),
            LuaType::Any => f.write_str("any"),
            LuaType::Unknown => f.write_str("unknown"),
            LuaType::Never => f.write_str("never"),
            LuaType::Named(name) => write!(f, "{}", bstr::BStr::new(name)),
            LuaType::Optional(inner) => write!(f, "{}?", DisplayLuaType(inner)),
            LuaType::Union(variants) => {
                for (i, v) in variants.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{}", DisplayLuaType(v))?;
                }
                Ok(())
            }
            LuaType::Table(_) => f.write_str("table"),
            LuaType::Function(_) => f.write_str("function"),
            LuaType::StringLiteral(s) => write!(f, "\"{}\"", bstr::BStr::new(s)),
            LuaType::BoolLiteral(b) => write!(f, "{b}"),
            LuaType::NumberLiteral(n) => write!(f, "{n}"),
            LuaType::TypeParam(name) => write!(f, "{}", bstr::BStr::new(name)),
            LuaType::Variadic(inner) => write!(f, "...{}", DisplayLuaType(inner)),
            LuaType::Tuple(items) => {
                f.write_str("(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", DisplayLuaType(item))?;
                }
                f.write_str(")")
            }
            LuaType::Generic { base, args } => {
                write!(f, "{}<", DisplayLuaType(base))?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    match arg {
                        shingetsu_vm::types::LuaTypeArg::Type(t) => {
                            write!(f, "{}", DisplayLuaType(t))?;
                        }
                        shingetsu_vm::types::LuaTypeArg::Pack(t) => {
                            write!(f, "...{}", DisplayLuaType(t))?;
                        }
                    }
                }
                f.write_str(">")
            }
            LuaType::Intersection(items) => {
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" & ")?;
                    }
                    write!(f, "{}", DisplayLuaType(item))?;
                }
                Ok(())
            }
            LuaType::Module(m) => write!(f, "{}", bstr::BStr::new(&m.name)),
        }
    }
}
