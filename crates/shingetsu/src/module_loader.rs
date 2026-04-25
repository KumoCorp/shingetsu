use std::path::Path;
use std::sync::Arc;

use shingetsu_compiler::{CompileOptions, Compiler};
use shingetsu_vm::error::VmError;
use shingetsu_vm::types::GlobalTypeMap;
use shingetsu_vm::{LoadedModule, ModuleLoader};

/// Default [`ModuleLoader`] that reads a `.lua`/`.luau` file from disk
/// and compiles it using the shingetsu compiler.
pub struct LuaModuleLoader {
    /// Compile options shared across all loaded modules.
    compiler: Compiler,
}

impl LuaModuleLoader {
    /// Create a new loader.  The provided `GlobalTypeMap` is used for
    /// compile-time diagnostics in loaded modules.
    pub fn new(global_types: GlobalTypeMap) -> Self {
        Self {
            compiler: Compiler::new(
                CompileOptions {
                    debug_info: true,
                    source_name: Arc::new(String::new()), // overridden per-file
                    type_check: false,
                },
                global_types,
            ),
        }
    }
}

#[async_trait::async_trait]
impl ModuleLoader for LuaModuleLoader {
    async fn load(&self, name: &str, path: &Path) -> Result<LoadedModule, VmError> {
        let source = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| VmError::HostError {
                name: "require".to_owned(),
                source: format!("{}", shingetsu_vm::error::portable_io_error_description(&e),)
                    .into(),
            })?;

        // Build a per-file compiler with the correct source name.
        let compiler = Compiler::new(
            CompileOptions {
                debug_info: self.compiler.opts().debug_info,
                source_name: Arc::new(format!("@{}", path.display())),
                type_check: self.compiler.opts().type_check,
            },
            self.compiler.global_types().clone(),
        );

        let bc = compiler
            .compile(&source)
            .await
            .map_err(|e| VmError::HostError {
                name: "require".to_owned(),
                source: format!("error compiling module '{name}': {e}").into(),
            })?;

        Ok(LoadedModule {
            proto: bc.top_level,
            type_info: bc.module_type_info,
        })
    }
}
