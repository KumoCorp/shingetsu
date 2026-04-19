use std::collections::HashMap;
use std::path::Path;

use shingetsu_compiler::{LintId, Severity};

/// Parsed representation of a `shingetsu.toml` project configuration file.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub lints: LintConfig,
}

/// Lint severity overrides from project-level configuration.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct LintConfig {
    #[serde(flatten)]
    pub overrides: HashMap<LintId, Severity>,
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
                return Self::from_toml_with_path(&contents, &candidate.display().to_string());
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_config() {
        let config = ProjectConfig::from_toml("").expect("parse");
        k9::assert_equal!(config.lints.overrides.len(), 0);
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
            config.lints.overrides.get(&LintId::Shadowing),
            Some(&Severity::Allow)
        );
        k9::assert_equal!(
            config.lints.overrides.get(&LintId::ArgCount),
            Some(&Severity::Warning)
        );
        k9::assert_equal!(
            config.lints.overrides.get(&LintId::UnusedVariable),
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
unknown variant `bogus`, expected one of `unused_variable`, `shadowing`, `unreachable_code`, `empty_loop`, `call_convention`, `arg_count`
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
            config.lints.overrides.get(&LintId::Shadowing),
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
            config.lints.overrides.get(&LintId::EmptyLoop),
            Some(&Severity::Error)
        );
    }

    #[test]
    fn discover_returns_default_when_none_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = ProjectConfig::discover(dir.path()).expect("discover");
        k9::assert_equal!(config.lints.overrides.len(), 0);
    }
}
