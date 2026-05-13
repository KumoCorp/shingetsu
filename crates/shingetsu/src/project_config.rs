use std::collections::HashMap;
use std::path::{Path, PathBuf};

use shingetsu_compiler::{LintId, Severity};

/// Parsed representation of a `shingetsu.toml` project configuration file.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub lints: LintConfig,
    #[serde(default)]
    pub check: CheckConfig,
    /// Directory containing the discovered `shingetsu.toml`.  Relative
    /// paths in the config (e.g. `[check] types`) resolve against this.
    /// `None` when constructed from a TOML string without a backing file.
    #[serde(skip)]
    pub config_dir: Option<PathBuf>,
}

/// Lint severity overrides from project-level configuration.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct LintConfig {
    #[serde(flatten)]
    pub overrides: HashMap<LintId, Severity>,
}

/// `[check]` section of `shingetsu.toml`: configuration for the
/// `shingetsu check` subcommand.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct CheckConfig {
    /// Paths to `DocModel` JSON files merged into the type checker's
    /// view.  Relative paths are resolved against the directory
    /// containing `shingetsu.toml`.
    #[serde(default)]
    pub types: Vec<PathBuf>,
    /// Paths to lint plugin files.  Each plugin is loaded into a
    /// fresh sandboxed [`crate::GlobalEnv`] and dispatched against
    /// every file under check.  Relative paths resolve against the
    /// directory containing `shingetsu.toml`.
    #[serde(default)]
    pub plugins: Vec<PathBuf>,
    /// Lint sets enabled by default.  When unset, defaults to
    /// `["builtins"]`.  See [`Self::resolved_default_sets`].
    #[serde(default)]
    pub default_sets: Vec<String>,
    /// Lint sets recognised as opt-in but off by default.
    /// Currently advisory -- the CLI's `--enable`/`--disable`
    /// flags don't validate against this list yet.
    #[serde(default)]
    pub optional_sets: Vec<String>,
}

/// Errors that can occur while loading project configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },
}

impl ProjectConfig {
    /// Walk from `start_dir` upward looking for `shingetsu.toml`.
    /// Returns `Default::default()` if none is found.
    pub fn discover(start_dir: &Path) -> Result<ProjectConfig, ConfigError> {
        let mut dir = start_dir;
        loop {
            let candidate = dir.join("shingetsu.toml");
            if candidate.is_file() {
                let contents =
                    std::fs::read_to_string(&candidate).map_err(|e| ConfigError::Io {
                        path: candidate.display().to_string(),
                        source: e,
                    })?;
                let mut config =
                    Self::from_toml_with_path(&contents, &candidate.display().to_string())?;
                config.config_dir = Some(dir.to_path_buf());
                return Ok(config);
            }
            match dir.parent() {
                Some(parent) => dir = parent,
                None => return Ok(ProjectConfig::default()),
            }
        }
    }

    /// Parse from an explicit TOML string (for testing or embedding).
    pub fn from_toml(toml_str: &str) -> Result<ProjectConfig, ConfigError> {
        Self::from_toml_with_path(toml_str, "<string>")
    }

    fn from_toml_with_path(toml_str: &str, path: &str) -> Result<ProjectConfig, ConfigError> {
        toml::from_str(toml_str).map_err(|e| ConfigError::Parse {
            path: path.to_string(),
            source: e,
        })
    }

    /// Resolve every `[check] types` entry against [`Self::config_dir`].
    /// Absolute paths are returned as-is.
    pub fn resolved_types(&self) -> Vec<PathBuf> {
        self.resolve_against_config_dir(&self.check.types)
    }

    /// Resolve every `[check] plugins` entry against
    /// [`Self::config_dir`].  Absolute paths are returned as-is.
    pub fn resolved_plugins(&self) -> Vec<PathBuf> {
        self.resolve_against_config_dir(&self.check.plugins)
    }

    /// Sets enabled by default.  Returns the configured list or
    /// `["builtins"]` when unset.  Combine with CLI overrides via
    /// [`Self::active_sets`].
    pub fn resolved_default_sets(&self) -> Vec<String> {
        if self.check.default_sets.is_empty() {
            vec!["builtins".to_string()]
        } else {
            self.check.default_sets.clone()
        }
    }

    /// Compute the active set list from the project config's
    /// defaults plus CLI overrides.  `enabled` adds sets; `disabled`
    /// removes them.  Disabled wins over enabled for the same name
    /// (so `--enable foo --disable foo` leaves `foo` disabled).
    pub fn active_sets(&self, enabled: &[String], disabled: &[String]) -> Vec<String> {
        let mut active: std::collections::BTreeSet<String> =
            self.resolved_default_sets().into_iter().collect();
        for s in enabled {
            active.insert(s.clone());
        }
        for s in disabled {
            active.remove(s);
        }
        active.into_iter().collect()
    }

    fn resolve_against_config_dir(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        paths
            .iter()
            .map(|p| match (&self.config_dir, p.is_absolute()) {
                (Some(base), false) => base.join(p),
                _ => p.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shingetsu_compiler::BuiltInLintId;

    #[test]
    fn parse_empty_config() {
        let config = ProjectConfig::from_toml("").expect("parse");
        k9::assert_equal!(config.lints.overrides, HashMap::new());
    }

    #[test]
    fn parse_lint_overrides() {
        let config = ProjectConfig::from_toml(
            r#"
[lints]
shadowing = "allow"
arg_count = "warn"
unused_variable = "deny"
"#,
        )
        .expect("parse");
        k9::assert_equal!(
            config
                .lints
                .overrides
                .get(&LintId::BuiltIn(BuiltInLintId::Shadowing)),
            Some(&Severity::Allow)
        );
        k9::assert_equal!(
            config
                .lints
                .overrides
                .get(&LintId::BuiltIn(BuiltInLintId::ArgCount)),
            Some(&Severity::Warning)
        );
        k9::assert_equal!(
            config
                .lints
                .overrides
                .get(&LintId::BuiltIn(BuiltInLintId::UnusedVariable)),
            Some(&Severity::Error)
        );
    }

    #[test]
    fn unknown_lint_errors() {
        let err = ProjectConfig::from_toml(
            r#"
[lints]
bogus = "allow"
"#,
        )
        .unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "failed to parse <string>: TOML parse error at line 2, column 1
  |
2 | [lints]
  | ^^^^^^^
unknown variant `bogus`, expected one of `arg_count`, `arg_type`, `assign_type`, `call_convention`, `deprecated`, `empty_loop`, `event_handler_arity`, `event_handler_transposition`, `event_name_unknown`, `field_access`, `interrupted_doc_comment`, `missing_return`, `module_shape`, `must_use`, `return_type`, `shadowing`, `undeclared_global`, `unreachable_code`, `unsupported_lint_ir_node`, `unused_variable`
"
        );
    }

    #[test]
    fn invalid_severity_errors() {
        let err = ProjectConfig::from_toml(
            r#"
[lints]
shadowing = "forbid"
"#,
        )
        .unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "failed to parse <string>: TOML parse error at line 2, column 1
  |
2 | [lints]
  | ^^^^^^^
unknown variant `forbid`, expected one of `allow`, `warn`, `deny`
"
        );
    }

    #[test]
    fn discover_finds_config_in_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("shingetsu.toml"),
            "[lints]\nshadowing = \"allow\"\n",
        )
        .expect("write");
        let config = ProjectConfig::discover(dir.path()).expect("discover");
        k9::assert_equal!(
            config
                .lints
                .overrides
                .get(&LintId::BuiltIn(BuiltInLintId::Shadowing)),
            Some(&Severity::Allow)
        );
    }

    #[test]
    fn discover_walks_upward() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("shingetsu.toml"),
            "[lints]\nempty_loop = \"deny\"\n",
        )
        .expect("write");
        let subdir = dir.path().join("src").join("nested");
        std::fs::create_dir_all(&subdir).expect("mkdir");
        let config = ProjectConfig::discover(&subdir).expect("discover");
        k9::assert_equal!(
            config
                .lints
                .overrides
                .get(&LintId::BuiltIn(BuiltInLintId::EmptyLoop)),
            Some(&Severity::Error)
        );
    }

    #[test]
    fn discover_returns_default_when_none_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = ProjectConfig::discover(dir.path()).expect("discover");
        k9::assert_equal!(config.lints.overrides, HashMap::new());
    }

    #[test]
    fn extra_toml_sections_are_ignored() {
        let config = ProjectConfig::from_toml(
            r#"
[lints]
shadowing = "allow"

[format]
indent = 4

[type_check]
strict = true
"#,
        )
        .expect("parse");
        k9::assert_equal!(
            config.lints.overrides,
            HashMap::from([(LintId::BuiltIn(BuiltInLintId::Shadowing), Severity::Allow)])
        );
    }

    #[test]
    fn parse_check_types() {
        let config = ProjectConfig::from_toml(
            r#"
[check]
types = ["./build/types.json", "/abs/path.json"]
"#,
        )
        .expect("parse");
        k9::assert_equal!(
            config.check.types,
            vec![
                PathBuf::from("./build/types.json"),
                PathBuf::from("/abs/path.json")
            ]
        );
    }

    /// `active_sets` defaults to `["builtins"]`, accepts CLI
    /// `enable` additions, and lets `disable` win for the same
    /// name.
    #[test]
    fn active_sets_resolves_defaults_and_overrides() {
        let cfg = ProjectConfig::from_toml("[check]\ndefault_sets = [\"builtins\", \"core\"]\n")
            .expect("parse");
        // No CLI overrides: defaults pass through (sorted).
        k9::assert_equal!(
            cfg.active_sets(&[], &[]),
            vec!["builtins".to_string(), "core".to_string()]
        );
        // `--enable extra` adds; `--disable core` removes.
        k9::assert_equal!(
            cfg.active_sets(&["extra".to_string()], &["core".to_string()]),
            vec!["builtins".to_string(), "extra".to_string()]
        );
        // Disabled wins over enabled for the same name.
        k9::assert_equal!(
            cfg.active_sets(&["foo".to_string()], &["foo".to_string()]),
            vec!["builtins".to_string(), "core".to_string()]
        );
    }

    /// When `default_sets` is unset, the implicit default is
    /// `["builtins"]`.
    #[test]
    fn active_sets_implicit_default_builtins() {
        let cfg = ProjectConfig::default();
        k9::assert_equal!(cfg.active_sets(&[], &[]), vec!["builtins".to_string()]);
    }

    #[test]
    fn resolved_types_joins_relative_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("shingetsu.toml"),
            "[check]\ntypes = [\"build/types.json\", \"/abs.json\"]\n",
        )
        .expect("write");
        let config = ProjectConfig::discover(dir.path()).expect("discover");
        k9::assert_equal!(
            config.resolved_types(),
            vec![
                dir.path().join("build/types.json"),
                PathBuf::from("/abs.json")
            ]
        );
    }

    #[test]
    fn empty_lints_table() {
        let config = ProjectConfig::from_toml(
            r#"
[lints]
"#,
        )
        .expect("parse");
        k9::assert_equal!(config.lints.overrides, HashMap::new());
    }
}
