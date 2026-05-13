mod codegen;
mod error;
mod lint_directives;
pub mod lint_ir;
mod locals;
mod lower;
mod scope;
mod type_check;
mod type_convert;
mod util;

pub use error::{BuiltInLintId, CompileError, Diagnostic, LintId, Severity, SourceLocation};
pub use lint_directives::LintDirectives;
pub use locals::locals_at_cursor;

use shingetsu_vm::proto::Proto;
use shingetsu_vm::types::{ModuleTypeInfo, ModuleTypeRegistry, UserdataTypeRegistry};
use shingetsu_vm::{Bytes, GlobalTypeMap, ModuleLoader};
use std::sync::Arc;

/// The result of compiling a Lua source chunk.
#[derive(Debug)]
pub struct Bytecode {
    pub top_level: Arc<Proto>,
    /// Non-fatal diagnostics (warnings) emitted during compilation.
    pub diagnostics: Vec<Diagnostic>,
    /// Lint directives parsed from source comments.
    pub lint_directives: LintDirectives,
    /// Type surface of the compiled module: exported type declarations
    /// and (when determinable) the return type.  Used by cross-module
    /// type propagation.
    pub module_type_info: ModuleTypeInfo,
}

impl Bytecode {
    /// Wrap the top-level chunk in a [`shingetsu_vm::Function`] ready
    /// to pass to [`shingetsu_vm::Task::new`].
    ///
    /// This is equivalent to
    /// `bytecode.into_function()` and is
    /// the usual way to obtain a callable from a freshly compiled
    /// chunk.
    pub fn into_function(self) -> shingetsu_vm::Function {
        shingetsu_vm::Function::lua(self.top_level, vec![])
    }
}

/// Result of [`Compiler::compile_with_ast`]: the compiled bytecode,
/// the parsed AST it was produced from, and the lowered lint IR
/// when type-checking is enabled.
///
/// `lint_ir` is `None` when [`CompileOptions::type_check`] is
/// `false`; the IR lowering only runs alongside type-checking
/// because that's when the result is actually consumed (by the
/// plugin lint pipeline).  Any [`lint_ir::UnsupportedNode`]
/// entries the lowering produced are surfaced as
/// [`BuiltInLintId::UnsupportedLintIrNode`] warnings on
/// `bytecode.diagnostics`, so callers don't need to inspect the
/// list separately.
pub struct CompiledChunk {
    pub ast: full_moon::ast::Ast,
    pub lint_ir: Option<lint_ir::Chunk>,
    pub bytecode: Bytecode,
}

#[derive(Clone, Debug)]
pub struct CompileOptions {
    /// Embed source locations in bytecode for stack traces.
    pub debug_info: bool,
    /// Name used in error messages and source locations.
    pub source_name: Arc<String>,
    /// Run the type checker after compilation, appending any type
    /// diagnostics to `Bytecode::diagnostics`.
    pub type_check: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            debug_info: true,
            source_name: Arc::new("=<string>".to_string()),
            type_check: false,
        }
    }
}

/// Lua source compiler.
///
/// Holds immutable configuration ([`CompileOptions`]) and the type
/// context ([`GlobalTypeMap`]) used for compile-time diagnostics.
/// Construct via [`Compiler::new`], then call [`Compiler::compile`]
/// for each source chunk.
pub struct Compiler {
    opts: CompileOptions,
    global_types: GlobalTypeMap,
    module_types: ModuleTypeRegistry,
    /// Userdata schemas consulted when resolving methods on a
    /// [`shingetsu_vm::LuaType::Named`] receiver.  Empty by default;
    /// embedders supply one via [`Self::with_userdata_types`].
    userdata_types: Arc<UserdataTypeRegistry>,
    module_loader: Option<Arc<dyn ModuleLoader>>,
    package_path: Option<String>,
}

impl Compiler {
    /// Create a new compiler with the given options and global type map.
    ///
    /// The type map is typically obtained from
    /// `GlobalEnv::global_type_map()`.  Pass `GlobalTypeMap::default()`
    /// when no type information is available.
    ///
    /// Uses an empty module type registry.  Call
    /// [`Compiler::with_module_types`] to provide cross-module type
    /// information.
    pub fn new(opts: CompileOptions, global_types: GlobalTypeMap) -> Self {
        Self {
            opts,
            global_types,
            module_types: ModuleTypeRegistry::default(),
            userdata_types: Arc::new(UserdataTypeRegistry::default()),
            module_loader: None,
            package_path: None,
        }
    }

    /// Set the module type registry for cross-module type propagation.
    pub fn with_module_types(mut self, module_types: ModuleTypeRegistry) -> Self {
        self.module_types = module_types;
        self
    }

    /// Set the userdata type registry consulted when the type
    /// checker resolves methods or fields on a
    /// [`shingetsu_vm::LuaType::Named`] receiver.
    pub fn with_userdata_types(mut self, userdata_types: Arc<UserdataTypeRegistry>) -> Self {
        self.userdata_types = userdata_types;
        self
    }

    /// Set the module loader for demand-driven require resolution.
    ///
    /// When type checking is enabled and a `require("foo")` call is
    /// encountered, the compiler uses this loader to compile the
    /// dependency and extract its type information.
    pub fn with_module_loader(mut self, loader: Arc<dyn ModuleLoader>) -> Self {
        self.module_loader = Some(loader);
        self
    }

    /// Set the initial package search path for require resolution.
    ///
    /// Semicolon-separated templates where `?` is replaced by the
    /// module name (dots converted to path separators).
    pub fn with_package_path(mut self, path: String) -> Self {
        self.package_path = Some(path);
        self
    }

    /// Access the compile options.
    pub fn opts(&self) -> &CompileOptions {
        &self.opts
    }

    /// Access the global type map.
    pub fn global_types(&self) -> &GlobalTypeMap {
        &self.global_types
    }

    /// Access the module type registry.
    pub fn module_types(&self) -> &ModuleTypeRegistry {
        &self.module_types
    }

    /// Access the userdata type registry.
    pub fn userdata_types(&self) -> &UserdataTypeRegistry {
        &self.userdata_types
    }

    /// Compile Lua source to bytecode.
    ///
    /// The parser accepts a blend of Lua 5.5 and LuaU syntax, so both
    /// native bitwise operators and type annotations work in the same source.
    pub async fn compile(&self, source: &str) -> Result<Bytecode, CompileError> {
        let CompiledChunk { bytecode, .. } = self.compile_with_ast(source).await?;
        Ok(bytecode)
    }

    /// Compile Lua source to bytecode and return the AST plus, when
    /// type-checking is enabled, the lowered lint IR alongside.
    ///
    /// Used by the plugin lint pipeline: callers that drive lint
    /// plugins need both the compiled bytecode and the lint-IR view
    /// of the same chunk.  The basic `compile` path discards both
    /// and just returns [`Bytecode`].
    pub async fn compile_with_ast(&self, source: &str) -> Result<CompiledChunk, CompileError> {
        let source_bytes = Bytes::from(source.to_owned());

        let lua_version = full_moon::LuaVersion::lua55().with_luau();

        let ast = full_moon::parse_fallible(source, lua_version);

        // Collect all parse errors up-front.
        let parse_errors = ast.errors();
        if !parse_errors.is_empty() {
            let first = &parse_errors[0];
            let (start, end) = first.range();
            let location = SourceLocation::from_span(&self.opts.source_name, start, end);

            // Build a clean message without the verbose location text
            // that full_moon's Display impl includes.
            let message = match first {
                full_moon::Error::AstError(ast_err) => {
                    let additional = ast_err.error_message();
                    if additional.is_empty() {
                        format!("unexpected token `{}`", ast_err.token())
                    } else {
                        format!("unexpected token `{}`, {additional}", ast_err.token())
                    }
                }
                full_moon::Error::TokenizerError(_) => first.error_message().into_owned(),
            };

            return Err(CompileError::Parse { location, message });
        }

        let ast = ast.into_ast();

        // Extract lint directives from comments before lowering.
        let (lint_directives, directive_diags) =
            lint_directives::extract_directives(&ast, &self.opts.source_name, source);

        let (
            mut proto,
            mut diagnostics,
            module_return_type,
            module_return_location,
            module_has_explicit_return,
            module_documented_locals,
            module_return_local,
        ) = lower::lower_chunk(&ast, self).await?;
        proto.set_source_text(source_bytes);
        proto.set_source_name(Arc::clone(&self.opts.source_name));

        // Run the type checker if enabled.
        if self.opts.type_check {
            let type_diags = type_check::check(&ast, self);
            diagnostics.extend(type_diags);
        }

        // Append directive-parsing diagnostics (e.g. unknown lint names).
        diagnostics.extend(directive_diags);

        // Build module type info from the top-level proto.
        let exported_types = proto
            .type_aliases
            .iter()
            .filter(|(_, alias)| alias.exported)
            .map(|(name, alias)| (name.clone(), alias.clone()))
            .collect();
        let module_type_info = ModuleTypeInfo {
            exported_types,
            return_type: module_return_type,
            return_location: module_return_location.map(Into::into),
            has_explicit_return: module_has_explicit_return,
            documented_locals: module_documented_locals,
            module_return_local,
        };

        // Lint IR is built only when type-checking is enabled --
        // that's the only path that drives plugin lints.  Each
        // unsupported AST node the lowering encounters becomes a
        // separate `unsupported_lint_ir_node` warning so the user
        // (or the compiler maintainer) sees that something fell
        // through.
        let lint_ir = if self.opts.type_check {
            let lowered = lint_ir::lower_ast(&ast);
            for entry in &lowered.unsupported {
                diagnostics.push(Diagnostic {
                    lint: LintId::BuiltIn(BuiltInLintId::UnsupportedLintIrNode),
                    severity: BuiltInLintId::UnsupportedLintIrNode.default_severity(),
                    location: entry.span.to_source_location(&self.opts.source_name),
                    message: format!(
                        "lint IR has no representation for {} -- this AST node \
                         will be invisible to plugin lints until the compiler \
                         is updated",
                        entry.kind_name,
                    ),
                    help: Some(format!(
                        "source spelling: {}",
                        entry.source_text.lines().next().unwrap_or(""),
                    )),
                    primary_label: None,
                    secondary_spans: vec![],
                });
            }
            Some(lowered.chunk)
        } else {
            None
        };

        let bytecode = Bytecode {
            top_level: Arc::new(proto),
            diagnostics,
            lint_directives,
            module_type_info,
        };

        Ok(CompiledChunk {
            ast,
            lint_ir,
            bytecode,
        })
    }
}

#[cfg(test)]
mod compile_with_ast_tests {
    use super::*;

    /// `compile_with_ast` with `type_check: false` returns the
    /// bytecode and AST but no lint IR -- the lowering pass only
    /// runs alongside type-checking, since that's where the IR
    /// would be consumed.
    #[tokio::test]
    async fn returns_no_lint_ir_when_type_check_disabled() {
        let compiler = Compiler::new(CompileOptions::default(), GlobalTypeMap::default());
        let result = compiler
            .compile_with_ast("local x = 1")
            .await
            .expect("compile");
        k9::assert_equal!(result.lint_ir.is_some(), false);
    }

    /// With `type_check: true`, `compile_with_ast` populates
    /// `lint_ir` with a fully lowered [`lint_ir::Chunk`].  Asserting
    /// on the full Debug snapshot keeps the IR-emission path in
    /// sync with the standalone lowering tests in
    /// `lint_ir::lower::tests`.
    #[tokio::test]
    async fn returns_lint_ir_when_type_check_enabled() {
        let opts = CompileOptions {
            type_check: true,
            ..CompileOptions::default()
        };
        let compiler = Compiler::new(opts, GlobalTypeMap::default());
        let result = compiler
            .compile_with_ast("local x = 1")
            .await
            .expect("compile");
        let chunk = result.lint_ir.expect("lint_ir is Some");
        k9::assert_equal!(
            format!("{:#?}", chunk),
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
}"#
        );
    }
}
