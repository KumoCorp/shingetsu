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

use super::{dispatch_chunk, load_plugin_with_source, new_plugin_env, PluginDeclaration};
use crate::diagnostic::{render_diagnostic_multi_source, RenderStyle};
use crate::GlobalEnv;
use shingetsu_compiler::{lint_ir, Diagnostic, LintId, Severity, SourceLocation};
use std::path::Path;
use std::sync::Arc;

/// Build a fully rendered diagnostic for the cross-plugin
/// duplicate-name collision.  Primary span anchors the second
/// plugin's `lint.declare`; the secondary span points at the
/// first plugin's earlier declaration so the user sees both call
/// sites at once.
fn render_duplicate_name_diagnostic(
    existing: &LoadedPlugin,
    duplicate: &PluginDeclaration,
    duplicate_source: &str,
) -> String {
    let fallback_site = |path: &std::path::Path| SourceLocation {
        source_name: Arc::new(path.display().to_string()),
        line: 0,
        column: 0,
        byte_offset: 0,
        byte_len: 0,
    };
    let dup_site = duplicate
        .declare_call_site
        .clone()
        .unwrap_or_else(|| fallback_site(&duplicate.source_path));
    let existing_site = existing
        .declaration
        .declare_call_site
        .clone()
        .unwrap_or_else(|| fallback_site(&existing.declaration.source_path));
    // Synthesize a `project:plugin_loader` id for the duplicate-
    // name diagnostic.  It isn't a lint against user source, but
    // the LintId / annotate-snippets pipeline insists on one and
    // this surfaces a meaningful label in the rendered title.
    let diag = Diagnostic {
        lint: LintId::Plugin(Arc::from("plugin_loader")),
        severity: Severity::Error,
        location: dup_site.clone(),
        message: format!(
            "lint plugin '{}' is declared more than once",
            duplicate.name,
        ),
        help: Some(
            "each plugin file must declare a unique name; \
             rename one of the conflicting plugins"
                .to_string(),
        ),
        primary_label: Some("this declaration conflicts".to_string()),
        secondary_spans: vec![(existing_site.clone(), "first declared here".to_string())],
    };
    let dup_name = dup_site.source_name.as_str();
    let existing_name = existing_site.source_name.as_str();
    let sources: Vec<(&str, &str)> = if dup_name == existing_name {
        vec![(dup_name, duplicate_source)]
    } else {
        vec![
            (dup_name, duplicate_source),
            (existing_name, existing.source.as_str()),
        ]
    };
    render_diagnostic_multi_source(&diag, &sources, RenderStyle::Plain)
}

/// A single loaded plugin: its dedicated `GlobalEnv`, the
/// declaration the plugin published via `lint.declare {...}`, and
/// the plugin file's source text (kept so cross-plugin
/// duplicate-name diagnostics can render annotated snippets
/// without re-reading the file).
pub struct LoadedPlugin {
    pub env: GlobalEnv,
    pub declaration: PluginDeclaration,
    pub source: Arc<String>,
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
            let source = Arc::new(
                std::fs::read_to_string(path)
                    .map_err(|e| format!("failed to read plugin file {}: {e}", path.display()))?,
            );
            let env = new_plugin_env().map_err(|e| e.to_string())?;
            let decl = load_plugin_with_source(&env, path, &source).await?;
            if let Some(existing) = plugins.iter().find(|p| p.declaration.name == decl.name) {
                return Err(render_duplicate_name_diagnostic(existing, &decl, &source));
            }
            plugins.push(LoadedPlugin {
                env,
                declaration: decl,
                source,
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

    /// Return the declared names of all loaded plugins, in load
    /// order, for validation of `project:`-prefixed lint references
    /// in source directives and config.
    pub fn plugin_names(&self) -> Vec<&str> {
        self.plugins
            .iter()
            .map(|p| p.declaration.name.as_str())
            .collect()
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
        self.lint_chunk_in_sets(source_name, chunk, None).await
    }

    /// Like [`Self::lint_chunk`] but skips plugins whose declared
    /// `sets` don't intersect `active_sets`.  Plugins with an
    /// empty `sets` list always run regardless of filtering
    /// (they're treated as having implicit membership in every
    /// set, matching the behavior for plugins that haven't opted
    /// into the set mechanism).  Pass `None` to disable filtering
    /// entirely; pass `Some(&[])` to skip every plugin that has
    /// declared sets.
    pub async fn lint_chunk_in_sets(
        &self,
        source_name: Arc<String>,
        chunk: &lint_ir::Chunk,
        active_sets: Option<&[String]>,
    ) -> Result<Vec<Diagnostic>, crate::VmError> {
        let mut out: Vec<Diagnostic> = Vec::new();
        for plugin in &self.plugins {
            if !plugin_active(plugin, active_sets) {
                continue;
            }
            let diags = dispatch_chunk(&plugin.env, Arc::clone(&source_name), chunk).await?;
            out.extend(diags);
        }
        Ok(out)
    }
}

fn plugin_active(plugin: &LoadedPlugin, active_sets: Option<&[String]>) -> bool {
    let Some(active) = active_sets else {
        return true;
    };
    if plugin.declaration.sets.is_empty() {
        return true;
    }
    plugin.declaration.sets.iter().any(|s| active.contains(s))
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
        let err = err
            .replace(plugin_a.path().to_str().expect("utf8"), "<plugin_a>")
            .replace(plugin_b.path().to_str().expect("utf8"), "<plugin_b>");
        k9::assert_equal!(
            err,
            concat!(
                r#"error[project:plugin_loader]: lint plugin 'shared' is declared more than once
 --> <plugin_b>:3:1
  |
3 | lint.declare { name = "shared", description = "second" }
  | ^^^^^^^^^^^^ this declaration conflicts
  |
 ::: <plugin_a>:3:1
  |
3 | lint.declare { name = "shared", description = "first" }
  | ------------ first declared here
  |
help: each plugin file must declare a unique name; rename one of the conflicting plugins"#,
            )
        );
    }
}
