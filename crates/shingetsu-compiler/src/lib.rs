mod codegen;
mod error;
mod lower;
mod scope;
mod type_convert;

pub use error::{CompileError, Diagnostic, Severity, SourceLocation};

use bytes::Bytes;
use shingetsu_vm::proto::Proto;
use shingetsu_vm::GlobalTypeMap;
use std::sync::Arc;

/// The result of compiling a Lua source chunk.
#[derive(Debug)]
pub struct Bytecode {
    pub top_level: Arc<Proto>,
    /// Non-fatal diagnostics (warnings) emitted during compilation.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct CompileOptions {
    /// Embed source locations in bytecode for stack traces.
    pub debug_info: bool,
    /// Name used in error messages and source locations.
    pub source_name: String,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            debug_info: true,
            source_name: "<string>".to_string(),
        }
    }
}

/// Lua source compiler.
///
/// Holds immutable configuration ([`CompileOptions`]) and the type
/// context ([`GlobalTypeMap`]) used for compile-time diagnostics.
/// Construct via [`Compiler::new`], then call [`Compiler::compile`]
/// for each source chunk.
#[derive(Clone, Debug)]
pub struct Compiler {
    opts: CompileOptions,
    global_types: GlobalTypeMap,
}

impl Compiler {
    /// Create a new compiler with the given options and global type map.
    ///
    /// The type map is typically obtained from
    /// `GlobalEnv::global_type_map()`.  Pass `GlobalTypeMap::default()`
    /// when no type information is available.
    pub fn new(opts: CompileOptions, global_types: GlobalTypeMap) -> Self {
        Self { opts, global_types }
    }

    /// Access the compile options.
    pub fn opts(&self) -> &CompileOptions {
        &self.opts
    }

    /// Access the global type map.
    pub fn global_types(&self) -> &GlobalTypeMap {
        &self.global_types
    }

    /// Compile Lua source to bytecode.
    ///
    /// The parser accepts a blend of Lua 5.4 and LuaU syntax, so both
    /// native bitwise operators and type annotations work in the same source.
    pub fn compile(&self, source: &str) -> Result<Bytecode, CompileError> {
        let source_bytes = Bytes::from(source.to_owned());

        let lua_version = full_moon::LuaVersion::lua54().with_luau();

        let ast = full_moon::parse_fallible(source, lua_version);

        // Collect all parse errors up-front.
        let parse_errors = ast.errors();
        if !parse_errors.is_empty() {
            let first = &parse_errors[0];
            let (pos, _) = first.range();
            return Err(CompileError::Parse {
                location: SourceLocation::from_pos(&self.opts.source_name, pos),
                message: first.to_string(),
            });
        }

        let ast = ast.into_ast();
        let (mut proto, diagnostics) = lower::lower_chunk(&ast, self)?;
        proto.set_source_text(source_bytes);
        Ok(Bytecode {
            top_level: Arc::new(proto),
            diagnostics,
        })
    }
}
