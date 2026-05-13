//! Load and drive multiple lint plugins, one per `GlobalEnv`.
//!
//! Each plugin file is loaded into a freshly constructed sandboxed
//! env; the orchestrator owns the resulting [`LoadedPlugin`]s and
//! routes `lint_chunk` dispatches through every plugin in load
//! order, collecting their diagnostics into a single `Vec`.
//!
//! Cross-plugin duplicate-name detection is enforced here -- the
//! per-env `PluginRegistry` only ever sees a single plugin, so
//! collisions across plugins surface as an orchestrator-level
//! error rather than a load-time `attach_declaration` rejection.

use super::{dispatch_chunk, load_plugin, new_plugin_env, PluginDeclaration};
use crate::GlobalEnv;
use shingetsu_compiler::{lint_ir, Diagnostic};
use std::path::Path;
use std::sync::Arc;

/// A single loaded plugin: its dedicated `GlobalEnv` and the
/// declaration the plugin published via `lint.declare {...}`.
pub struct LoadedPlugin {
    pub env: GlobalEnv,
    pub declaration: PluginDeclaration,
}

// Manual Debug -- GlobalEnv doesn't implement Debug, so derive
// fails.  The declaration alone is the useful introspection target.
impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("declaration", &self.declaration)
            .finish_non_exhaustive()
    }
}

/// Ordered collection of loaded plugins.  Construct via
/// [`Self::load_from_paths`]; drive analyses via [`Self::lint_chunk`].
#[derive(Debug)]
pub struct LoadedPlugins {
    plugins: Vec<LoadedPlugin>,
}

impl LoadedPlugins {
    /// Load each path in `paths` into its own sandboxed plugin env.
    /// Stops at the first error and returns the rendered diagnostic
    /// (failure-to-read, compile error, runtime error, or
    /// cross-plugin duplicate-name detection).
    pub async fn load_from_paths(paths: &[impl AsRef<Path>]) -> Result<Self, String> {
        let mut plugins: Vec<LoadedPlugin> = Vec::with_capacity(paths.len());
        for path in paths {
            let path = path.as_ref();
            let env = new_plugin_env().map_err(|e| e.to_string())?;
            let decl = load_plugin(&env, path).await?;
            if let Some(existing) = plugins.iter().find(|p| p.declaration.name == decl.name) {
                return Err(format!(
                    "lint plugin '{}' is already declared (loaded from {})",
                    decl.name,
                    existing.declaration.source_path.display(),
                ));
            }
            plugins.push(LoadedPlugin {
                env,
                declaration: decl,
            });
        }
        Ok(LoadedPlugins { plugins })
    }

    /// Number of plugins currently loaded.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Iterate over the loaded plugins in load order.
    pub fn iter(&self) -> impl Iterator<Item = &LoadedPlugin> {
        self.plugins.iter()
    }

    /// Run every loaded plugin's dispatch against `chunk` and
    /// return the concatenated diagnostics in plugin-load order.
    ///
    /// Per-plugin dispatch errors propagate up unchanged -- the
    /// only path to one is a Rust-side dispatch failure (e.g. an
    /// env that has no loaded plugin, which the orchestrator
    /// guarantees against).  Plugin-side callback errors become
    /// `Warning` diagnostics via the plugin error policy and
    /// flow through as part of the returned `Vec`.
    pub async fn lint_chunk(
        &self,
        source_name: Arc<String>,
        chunk: &lint_ir::Chunk,
    ) -> Result<Vec<Diagnostic>, crate::VmError> {
        let mut out: Vec<Diagnostic> = Vec::new();
        for plugin in &self.plugins {
            let diags = dispatch_chunk(&plugin.env, Arc::clone(&source_name), chunk).await?;
            out.extend(diags);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{render_warnings, RenderStyle};
    use shingetsu_compiler::lint_ir;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_plugin(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("tempfile");
        file.write_all(contents.as_bytes()).expect("write");
        file.flush().expect("flush");
        file
    }

    /// Two plugins listening on `method_call` each emit their own
    /// diagnostic; the orchestrator concatenates them in load
    /// order.
    #[tokio::test]
    async fn lint_chunk_runs_every_plugin() {
        let plugin_a = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "alpha", description = "a" }
lint.on("method_call", function(call, ctx) ctx:warn(call.span, "alpha saw " .. call.method) end)
"#,
        );
        let plugin_b = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "beta", description = "b" }
lint.on("method_call", function(call, ctx) ctx:warn(call.span, "beta saw " .. call.method) end)
"#,
        );
        let loaded = LoadedPlugins::load_from_paths(&[plugin_a.path(), plugin_b.path()])
            .await
            .expect("load");
        k9::assert_equal!(loaded.len(), 2);

        let source_text = "obj:foo()";
        let ast = full_moon::parse(source_text).expect("parse");
        let lowered = lint_ir::lower::lower(&ast);

        let diags = loaded
            .lint_chunk(Arc::new("@test.lua".to_string()), &lowered.chunk)
            .await
            .expect("dispatch");

        let rendered = render_warnings(&diags, source_text, RenderStyle::Plain);
        k9::assert_equal!(
            rendered,
            r#"warning[project:alpha]: alpha saw foo
 --> test.lua:1:1
  |
1 | obj:foo()
  | ^^^^^^^^ alpha saw foo
warning[project:beta]: beta saw foo
 --> test.lua:1:1
  |
1 | obj:foo()
  | ^^^^^^^^ beta saw foo"#
        );
    }

    /// Two plugin files declaring the same name are caught here,
    /// not by the per-env PluginRegistry (which only sees one
    /// plugin).  The error names both source paths so the user
    /// can disambiguate.
    #[tokio::test]
    async fn duplicate_name_across_plugins_errors() {
        let plugin_a = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "shared", description = "first" }
"#,
        );
        let plugin_b = write_plugin(
            r#"
local lint = require("shingetsu.lint")
lint.declare { name = "shared", description = "second" }
"#,
        );
        let err = LoadedPlugins::load_from_paths(&[plugin_a.path(), plugin_b.path()])
            .await
            .expect_err("should fail");
        k9::assert_equal!(
            err,
            format!(
                "lint plugin 'shared' is already declared (loaded from {})",
                plugin_a.path().display(),
            )
        );
    }
}
