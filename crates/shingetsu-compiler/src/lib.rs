mod codegen;
mod error;
mod lint_directives;
mod lower;
mod scope;
mod type_check;
mod type_convert;
mod util;

pub use error::{CompileError, Diagnostic, LintId, Severity, SourceLocation};
pub use lint_directives::LintDirectives;

use shingetsu_vm::proto::Proto;
use shingetsu_vm::types::{ModuleTypeInfo, ModuleTypeRegistry};
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

#[derive(Clone, Debug)]
pub struct CompileOptions {
    /// Embed source locations in bytecode for stack traces.
    pub debug_info: bool,
    /// Name used in error messages and source locations.
    pub source_name: String,
    /// Run the type checker after compilation, appending any type
    /// diagnostics to `Bytecode::diagnostics`.
    pub type_check: bool,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            debug_info: true,
            source_name: "=<string>".to_string(),
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
            module_loader: None,
            package_path: None,
        }
    }

    /// Set the module type registry for cross-module type propagation.
    pub fn with_module_types(mut self, module_types: ModuleTypeRegistry) -> Self {
        self.module_types = module_types;
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

    /// Compile Lua source to bytecode.
    ///
    /// The parser accepts a blend of Lua 5.4 and LuaU syntax, so both
    /// native bitwise operators and type annotations work in the same source.
    pub async fn compile(&self, source: &str) -> Result<Bytecode, CompileError> {
        let source_bytes = Bytes::from(source.to_owned());

        let lua_version = full_moon::LuaVersion::lua54().with_luau();

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

        let (mut proto, mut diagnostics, module_return_type) =
            lower::lower_chunk(&ast, self).await?;
        proto.set_source_text(source_bytes);

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
        };

        Ok(Bytecode {
            top_level: Arc::new(proto),
            diagnostics,
            lint_directives,
            module_type_info,
        })
    }
}
