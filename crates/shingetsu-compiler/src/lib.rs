mod codegen;
mod error;
mod lower;
mod scope;
mod type_convert;

pub use error::{CompileError, SourceLocation};

use shingetsu_vm::proto::Proto;
use std::sync::Arc;

/// The result of compiling a Lua source chunk.
#[derive(Debug)]
pub struct Bytecode {
    pub top_level: Arc<Proto>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dialect {
    Lua54,
    LuaU,
}

#[derive(Clone, Debug)]
pub struct CompileOptions {
    pub dialect: Dialect,
    /// Embed source locations in bytecode for stack traces.
    pub debug_info: bool,
    /// Name used in error messages and source locations.
    pub source_name: String,
}

impl Default for CompileOptions {
    fn default() -> Self {
        CompileOptions {
            dialect: Dialect::Lua54,
            debug_info: true,
            source_name: "<string>".to_string(),
        }
    }
}

/// Compile Lua source to bytecode.
pub fn compile(source: &str, opts: &CompileOptions) -> Result<Bytecode, CompileError> {
    let lua_version = match opts.dialect {
        Dialect::Lua54 => full_moon::LuaVersion::lua54(),
        Dialect::LuaU => full_moon::LuaVersion::luau(),
    };

    let ast = full_moon::parse_fallible(source, lua_version);

    // Collect all parse errors up-front.
    let parse_errors = ast.errors();
    if !parse_errors.is_empty() {
        let first = &parse_errors[0];
        let (pos, _) = first.range();
        return Err(CompileError::Parse {
            location: SourceLocation {
                source_name: opts.source_name.clone(),
                line: pos.line() as u32,
                column: pos.character() as u32,
            },
            message: first.to_string(),
        });
    }

    let ast = ast.into_ast();
    let proto = lower::lower_chunk(&ast, opts)?;
    Ok(Bytecode {
        top_level: Arc::new(proto),
    })
}
