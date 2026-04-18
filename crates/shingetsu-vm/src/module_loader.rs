use std::path::PathBuf;
use std::sync::Arc;

use crate::error::VmError;
use crate::proto::Proto;
use crate::types::ModuleTypeInfo;

/// The result of loading and compiling a Lua module.
///
/// Carries both the executable bytecode (`proto`) and the type surface
/// (`type_info`) extracted during compilation.  The runtime uses `proto`
/// for execution; the compiler uses `type_info` for cross-module type
/// propagation.
pub struct LoadedModule {
    /// The compiled top-level chunk, ready for execution.
    pub proto: Arc<Proto>,
    /// Type surface of the module (exported types and return type).
    pub type_info: ModuleTypeInfo,
}

/// Trait for loading Lua modules from external sources (e.g. the filesystem).
///
/// The VM crate does not depend on the compiler, so compilation is delegated
/// to the embedder via this trait.  The `shingetsu` top-level crate provides
/// a default implementation that reads a file and compiles it.
#[async_trait::async_trait]
pub trait ModuleLoader: Send + Sync {
    /// Read and compile the module at `path`, returning the compiled module.
    ///
    /// `name` is the original module name passed to `require` (for error
    /// messages).  `path` is a candidate filesystem path generated from the
    /// search templates.
    ///
    /// Implementations should return an error if the file does not exist,
    /// cannot be read, or fails to compile.  The error message is used
    /// as the reason string for that candidate in `require`'s composite
    /// error message.
    async fn load(&self, name: &str, path: &std::path::Path) -> Result<LoadedModule, VmError>;
}

/// Generate candidate file paths for a module name by expanding search
/// templates.
///
/// Templates are separated by `;`.  Within each template, every `?` is
/// replaced by `name` (with `.` converted to the platform path separator).
/// Returns the list of candidate paths to try (in order).
pub fn candidate_paths(name: &str, path_str: &str) -> Vec<PathBuf> {
    let module_path = name.replace('.', std::path::MAIN_SEPARATOR_STR);
    let mut candidates = Vec::new();

    for template in path_str.split(';') {
        let template = template.trim();
        if template.is_empty() {
            continue;
        }
        candidates.push(PathBuf::from(template.replace('?', &module_path)));
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_paths_basic() {
        let paths = candidate_paths("mymod", "./?.lua;./?.luau");
        k9::assert_equal!(
            paths,
            vec![PathBuf::from("./mymod.lua"), PathBuf::from("./mymod.luau"),]
        );
    }

    #[test]
    fn candidate_paths_dot_to_separator() {
        let paths = candidate_paths("foo.bar", "./?.lua");
        let expected = format!("./foo{}bar.lua", std::path::MAIN_SEPARATOR);
        k9::assert_equal!(paths, vec![PathBuf::from(expected)]);
    }

    #[test]
    fn candidate_paths_empty_template_skipped() {
        let paths = candidate_paths("x", ";;./?.lua");
        k9::assert_equal!(paths, vec![PathBuf::from("./x.lua")]);
    }

    #[test]
    fn candidate_paths_multiple_templates() {
        let paths = candidate_paths("mod", "/a/?.lua;/b/?.luau");
        k9::assert_equal!(
            paths,
            vec![PathBuf::from("/a/mod.lua"), PathBuf::from("/b/mod.luau"),]
        );
    }
}
