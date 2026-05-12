mod doc;
mod highlight;
mod repl;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use shingetsu::diagnostic::{
    render_compile_error, render_runtime_error, render_warnings, RenderStyle,
};
use shingetsu::{valuevec, Function, GlobalEnv, GlobalTypeMap, Libraries, Task, VmError};
use shingetsu_compiler::{Bytecode, CompileOptions, Compiler, Diagnostic, LintId, Severity};
use shingetsu_docgen::DocModel;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "shingetsu", about = "Shingetsu Lua runtime")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile and run a Lua script.
    Run {
        /// Path to the Lua source file.
        file: PathBuf,

        #[command(flatten)]
        lib_opts: LibraryOpts,

        #[command(flatten)]
        lint_opts: LintOpts,

        /// Set the module search path for file-based `require`.
        /// Semicolon-separated templates where `?` is replaced by the
        /// module name.  Example: `./?.lua;./libs/?.lua`
        #[arg(long)]
        path: Option<String>,
    },

    /// Start an interactive REPL session.
    Repl {
        #[command(flatten)]
        lib_opts: LibraryOpts,

        /// Syntax highlight theme.
        #[arg(
            long,
            default_value = "dark",
            value_parser = clap::builder::PossibleValuesParser::new(
                highlight::HighlightTheme::theme_names().collect::<Vec<_>>()
            ),
        )]
        theme: String,
    },

    /// Type-check one or more Lua scripts without executing them.
    Check {
        /// Paths to Lua source files.
        files: Vec<PathBuf>,

        #[command(flatten)]
        lib_opts: LibraryOpts,

        #[command(flatten)]
        lint_opts: LintOpts,

        /// Path to a `shingetsu doc dump-json`-style JSON file
        /// describing embedder modules / events / globals.  May be
        /// passed multiple times; entries merge into the type
        /// checker's environment view.
        #[arg(long = "types")]
        types: Vec<PathBuf>,
    },

    /// Produce reference documentation from the standard preloaded
    /// `GlobalEnv` (or, for `render-markdown`, from a JSON export).
    Doc {
        #[command(subcommand)]
        action: doc::DocAction,
    },
}

/// Library selection options shared between `run` and `check`.
#[derive(clap::Args)]
struct LibraryOpts {
    /// Equivalent to `--libraries sandboxed`: register only the
    /// sandbox-safe libraries (math, string, table, utf8).  Can be
    /// combined with --libraries to include additional libraries.
    #[arg(long)]
    sandboxed: bool,

    /// Comma-separated list of libraries to register, replacing the
    /// default set (all).  Use --sandboxed to include the sandbox-safe
    /// base set alongside specific additions.
    ///
    /// Valid names: builtins, os, io, stdio, exec, env, exit, debug,
    /// package, all, sandboxed.
    #[arg(long, value_parser = parse_libraries)]
    libraries: Option<Libraries>,
}

impl LibraryOpts {
    /// Resolve the effective library set from the CLI flags.
    fn resolve(&self) -> Libraries {
        match (self.sandboxed, &self.libraries) {
            // --sandboxed --libraries os,io → SANDBOXED | os | io
            (true, Some(extra)) => Libraries::SANDBOXED | *extra,
            // --sandboxed (alone) → SANDBOXED
            (true, None) => Libraries::SANDBOXED,
            // --libraries os,io (no sandbox) → exactly what was listed
            (false, Some(libs)) => *libs,
            // neither flag → ALL
            (false, None) => Libraries::ALL,
        }
    }
}

pub(crate) fn parse_libraries(s: &str) -> Result<Libraries, String> {
    s.parse()
}

/// Lint severity override options for the `check` command.
#[derive(clap::Args)]
struct LintOpts {
    /// Comma-separated lint ids to suppress (allow).
    #[arg(long, value_parser = parse_lint_ids, value_delimiter = ',')]
    allow: Vec<LintId>,

    /// Comma-separated lint ids to set as warnings.
    #[arg(long, value_parser = parse_lint_ids, value_delimiter = ',')]
    warn: Vec<LintId>,

    /// Comma-separated lint ids to set as errors.
    #[arg(long, value_parser = parse_lint_ids, value_delimiter = ',')]
    deny: Vec<LintId>,
}

impl LintOpts {
    fn into_overrides(self) -> HashMap<LintId, Severity> {
        let mut map = HashMap::new();
        for id in self.allow {
            map.insert(id, Severity::Allow);
        }
        for id in self.warn {
            map.insert(id, Severity::Warning);
        }
        for id in self.deny {
            map.insert(id, Severity::Error);
        }
        map
    }
}

fn parse_lint_ids(s: &str) -> Result<LintId, String> {
    LintId::from_name(s).ok_or_else(|| {
        let all: Vec<&str> = LintId::all().iter().map(|l| l.name()).collect();
        format!("unknown lint '{s}'; available: {}", all.join(", "))
    })
}

/// Apply project-level, CLI-level, and in-file lint directives, returning
/// the filtered diagnostics.
/// Load each `--types <path>` JSON file, deserialize as a
/// [`DocModel`], and convert it into a partial [`GlobalTypeMap`]
/// suitable for merging into the live type-checker map.
fn load_doc_model_types(paths: &[PathBuf]) -> anyhow::Result<Vec<GlobalTypeMap>> {
    paths
        .iter()
        .map(|p| {
            let src = std::fs::read_to_string(p)
                .with_context(|| format!("reading types file {}", p.display()))?;
            let model: DocModel = serde_json::from_str(&src)
                .with_context(|| format!("parsing types file {}", p.display()))?;
            Ok(model.to_global_type_map())
        })
        .collect()
}

/// Merge each external type map into `dest`, overwriting on conflict.
/// External entries should not normally collide with the live map
/// since `--types` describes additions, but we accept overrides so
/// that an embedder can re-declare a global it has restyled.
fn merge_global_types(dest: &mut GlobalTypeMap, extra: &[GlobalTypeMap]) {
    for src in extra {
        for (k, v) in &src.types {
            dest.types.insert(k.clone(), v.clone());
        }
        for r in &src.event_registrars {
            dest.event_registrars.insert(r.clone());
        }
        for (k, v) in &src.event_handler_signatures {
            dest.event_handler_signatures.insert(k.clone(), v.clone());
        }
    }
}

fn apply_lint_config(
    file: &std::path::Path,
    bytecode: Bytecode,
    cli_overrides: &HashMap<LintId, Severity>,
) -> Vec<Diagnostic> {
    let project_config = shingetsu::project_config::ProjectConfig::discover(
        file.parent().unwrap_or_else(|| std::path::Path::new(".")),
    )
    .unwrap_or_default();
    let mut overrides = project_config.lints.overrides;
    // CLI overrides take precedence over project config.
    overrides.extend(cli_overrides);
    let mut directives = bytecode.lint_directives;
    directives.project_overrides = overrides;
    directives.filter(bytecode.diagnostics)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            file,
            lib_opts,
            lint_opts,
            path: path_opt,
        } => {
            let cli_overrides = lint_opts.into_overrides();
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;

            let opts = CompileOptions {
                debug_info: true,
                source_name: Arc::new(format!("@{}", file.display())),
                type_check: false,
            };

            let env = GlobalEnv::new();
            let libs = lib_opts.resolve();
            shingetsu::register_libs(&env, libs)?;

            // Set the package search path.  --path takes priority;
            // otherwise default to the script's parent directory.
            if libs.contains(Libraries::PACKAGE) {
                if let Some(ref explicit) = path_opt {
                    env.set_package_path(Some(explicit.clone()));
                } else {
                    let script_dir = file
                        .parent()
                        .and_then(|p| p.canonicalize().ok())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));
                    let sep = std::path::MAIN_SEPARATOR;
                    env.set_package_path(Some(format!(
                        "{dir}{sep}?.lua;{dir}{sep}?.luau",
                        dir = script_dir.display(),
                    )));
                }
            }

            let compiler = Compiler::new(opts, env.global_type_map())
                .with_module_types(env.preload_module_types());

            let style = if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                RenderStyle::Colored
            } else {
                RenderStyle::Plain
            };

            let bytecode = match compiler.compile(&source).await {
                Ok(bc) => bc,
                Err(e) => {
                    eprint!("{}", render_compile_error(&e, &source, style));
                    std::process::exit(1);
                }
            };

            let top_level = bytecode.top_level.clone();
            let diagnostics = apply_lint_config(&file, bytecode, &cli_overrides);
            if !diagnostics.is_empty() {
                eprintln!("{}", render_warnings(&diagnostics, &source, style));
            }
            if diagnostics.iter().any(|d| d.severity == Severity::Error) {
                std::process::exit(1);
            }

            // Load the top-level chunk as a global named "@main".
            // Then create a task and run it.
            let func = Function::lua(top_level, vec![]);

            // Keep a handle to the env for the ExitRequested path — we
            // may need to run `__gc` finalizers via `dispose()` after
            // the task returns.
            let env_for_exit = env.clone();
            let task = Task::new(env, func, valuevec![]);
            let results = match task.await {
                Ok(r) => r,
                Err(re) if matches!(re.error, VmError::ExitRequested { .. }) => {
                    let (code, close) = match re.error {
                        VmError::ExitRequested { code, close } => (code, close),
                        _ => unreachable!(),
                    };
                    if close {
                        // close=true runs __gc finalizers.  __close on
                        // live `<close>` locals has already been
                        // dispatched during task unwind.  Finalizers may
                        // write to stdout, so flush after dispose.
                        env_for_exit.dispose().await;
                    }
                    // Always flush stdio — a script that printed and
                    // then called os.exit expects its output to appear,
                    // as do any __gc finalizers that ran during dispose.
                    shingetsu::io::flush_stdio().await;
                    std::process::exit(code);
                }
                Err(re) => {
                    eprint!("{}", render_runtime_error(&re, style));
                    std::process::exit(1);
                }
            };

            shingetsu::io::flush_stdio().await;

            for v in &results {
                println!("{v}");
            }

            Ok(())
        }

        Command::Repl {
            lib_opts,
            theme: theme_name,
        } => {
            let theme = highlight::HighlightTheme::named(&theme_name).unwrap_or_default();
            let env = GlobalEnv::new();
            shingetsu::register_libs(&env, lib_opts.resolve())?;
            repl::run_repl(env, theme).await?;
            Ok(())
        }

        Command::Doc { action } => doc::run(action).await,

        Command::Check {
            files,
            lib_opts,
            lint_opts,
            types,
        } => {
            let style = if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                RenderStyle::Colored
            } else {
                RenderStyle::Plain
            };

            let cli_overrides = lint_opts.into_overrides();
            let env = GlobalEnv::new();
            shingetsu::register_libs(&env, lib_opts.resolve())?;

            // Load any external `DocModel` JSON files and merge
            // them into the type-checker's global type map.
            let extra_types = load_doc_model_types(&types)?;

            let mut has_errors = false;

            for file in &files {
                let source = match std::fs::read_to_string(file) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("error: reading {}: {e}", file.display());
                        has_errors = true;
                        continue;
                    }
                };

                let opts = CompileOptions {
                    debug_info: true,
                    source_name: Arc::new(format!("@{}", file.display())),
                    type_check: true,
                };

                let mut global_types = env.global_type_map();
                merge_global_types(&mut global_types, &extra_types);
                let compiler =
                    Compiler::new(opts, global_types).with_module_types(env.preload_module_types());

                let bytecode = match compiler.compile(&source).await {
                    Ok(bc) => bc,
                    Err(e) => {
                        eprint!("{}", render_compile_error(&e, &source, style));
                        has_errors = true;
                        continue;
                    }
                };

                let diagnostics = apply_lint_config(file, bytecode, &cli_overrides);
                if !diagnostics.is_empty() {
                    eprintln!("{}", render_warnings(&diagnostics, &source, style));
                }

                if diagnostics.iter().any(|d| d.severity == Severity::Error) {
                    has_errors = true;
                }
            }

            if has_errors {
                std::process::exit(1);
            }

            Ok(())
        }
    }
}
