mod doc;
mod highlight;
mod repl;

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use shingetsu::diagnostic::{
    render_compile_error, render_runtime_error, render_warnings, RenderStyle,
};
use shingetsu::types::UserdataTypeRegistry;
use shingetsu::{valuevec, Function, GlobalEnv, GlobalTypeMap, Libraries, Task, VmError};
use shingetsu_compiler::{
    BuiltInLintId, Bytecode, CompileOptions, Compiler, Diagnostic, LintId, Severity,
};
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

    /// Comma-separated lint set names to enable on top of
    /// `[check] default_sets`.
    #[arg(long, value_delimiter = ',')]
    enable: Vec<String>,

    /// Comma-separated lint set names to disable; overrides
    /// `--enable` for the same name.
    #[arg(long, value_delimiter = ',')]
    disable: Vec<String>,
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
        let all: Vec<&str> = BuiltInLintId::all().iter().map(|l| l.name()).collect();
        format!("unknown lint '{s}'; available: {}", all.join(", "))
    })
}

/// Apply project-level, CLI-level, and in-file lint directives, returning
/// the filtered diagnostics.
/// Load and merge every `--types <path>` JSON into a single
/// [`DocModel`].  Merge semantics (including `partial = true`) come
/// from [`DocModel::merge`].
fn load_merged_doc_model(paths: &[PathBuf]) -> anyhow::Result<Option<DocModel>> {
    let mut models = Vec::with_capacity(paths.len());
    for p in paths {
        let src = std::fs::read_to_string(p)
            .with_context(|| format!("reading types file {}", p.display()))?;
        let model: DocModel = serde_json::from_str(&src)
            .with_context(|| format!("parsing types file {}", p.display()))?;
        models.push(model);
    }
    let mut iter = models.into_iter();
    let Some(first) = iter.next() else {
        return Ok(None);
    };
    let merged = first
        .merge(iter.collect())
        .map_err(|e| anyhow::anyhow!("merging --types data: {e}"))?;
    Ok(Some(merged))
}

/// Overlay every entry of `src` onto `dest`.  External entries
/// shouldn't normally collide with the live map (the live env owns
/// its own globals), but conflicts overwrite so an embedder can
/// re-declare a restyled global.
fn merge_global_types(dest: &mut GlobalTypeMap, src: &GlobalTypeMap) {
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

/// Layer external userdata schemas into `dest`.  Later entries
/// overwrite earlier ones with the same name.
fn merge_userdata_types(dest: &UserdataTypeRegistry, src: &UserdataTypeRegistry) {
    for ud in src.snapshot() {
        dest.insert(ud);
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
    overrides.extend(cli_overrides.iter().map(|(k, v)| (k.clone(), *v)));
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

            let enable_sets = lint_opts.enable.clone();
            let disable_sets = lint_opts.disable.clone();
            let cli_overrides = lint_opts.into_overrides();
            let env = GlobalEnv::new();
            shingetsu::register_libs(&env, lib_opts.resolve())?;

            // Discover the project config once for the whole check
            // run, using the first file's parent dir (or CWD) as the
            // search anchor.  Per-file lint directives still resolve
            // against each file's own project config via
            // `apply_lint_config`; the type-data set is global.
            let anchor = files
                .first()
                .and_then(|f| f.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let project_config =
                shingetsu::project_config::ProjectConfig::discover(&anchor).unwrap_or_default();

            // Project-declared types come first; CLI flags append.
            let mut all_type_paths = project_config.resolved_types();
            all_type_paths.extend(types.iter().cloned());
            let merged_external = load_merged_doc_model(&all_type_paths)?;

            // Load any project-declared lint plugins.  Each plugin
            // lives in its own sandboxed env; orchestrator detects
            // cross-plugin name collisions.
            let plugin_paths = project_config.resolved_plugins();
            let plugins = if plugin_paths.is_empty() {
                None
            } else {
                match shingetsu::lint_plugin::LoadedPlugins::load_from_paths(&plugin_paths).await {
                    Ok(p) => Some(p),
                    Err(rendered) => {
                        eprint!("{rendered}");
                        if !rendered.ends_with('\n') {
                            eprintln!();
                        }
                        std::process::exit(1);
                    }
                }
            };

            // Active lint sets = project default_sets plus CLI
            // --enable, minus --disable.  Disabled wins.
            let active_sets = project_config.active_sets(&enable_sets, &disable_sets);
            let (extra_globals, extra_userdata) = match &merged_external {
                Some(m) => (m.to_global_type_map(), m.to_userdata_type_registry()),
                None => (GlobalTypeMap::default(), UserdataTypeRegistry::default()),
            };

            // Build the userdata registry the compiler consults:
            // start with the env's snapshot, then layer any external
            // DocModel-derived userdata on top.
            let userdata_registry = env.userdata_type_registry_snapshot();
            merge_userdata_types(&userdata_registry, &extra_userdata);
            let userdata_registry = std::sync::Arc::new(userdata_registry);

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
                merge_global_types(&mut global_types, &extra_globals);
                let compiler = Compiler::new(opts, global_types)
                    .with_module_types(env.preload_module_types())
                    .with_userdata_types(std::sync::Arc::clone(&userdata_registry));

                let compiled = match compiler.compile_with_ast(&source).await {
                    Ok(c) => c,
                    Err(e) => {
                        eprint!("{}", render_compile_error(&e, &source, style));
                        has_errors = true;
                        continue;
                    }
                };

                let mut bytecode = compiled.bytecode;

                // Validate `project:`-prefixed lint names in source
                // directives against the loaded plugin set.  Unknown
                // plugin lints produce Warning diagnostics (the plugin
                // might be temporarily disabled); unknown unprefixed
                // built-in names already produce Error diagnostics
                // during directive extraction.
                let plugin_names: Vec<&str> = plugins
                    .as_ref()
                    .map(|p| p.plugin_names())
                    .unwrap_or_default();
                let plugin_name_diags = bytecode
                    .lint_directives
                    .validate_against_plugins(&plugin_names);
                bytecode.diagnostics.extend(plugin_name_diags);

                // Run loaded plugins against the lint IR if both
                // sides are present.  Plugin-emitted diagnostics
                // join the compiler's stream and ride the same
                // severity-override pipeline below.
                if let (Some(plugins), Some(chunk)) = (plugins.as_ref(), compiled.lint_ir.as_ref())
                {
                    let source_name = Arc::new(format!("@{}", file.display()));
                    match plugins
                        .lint_chunk_in_sets(source_name, chunk, Some(&active_sets))
                        .await
                    {
                        Ok(diags) => bytecode.diagnostics.extend(diags),
                        Err(e) => {
                            eprintln!("error: plugin dispatch failed for {}: {e}", file.display(),);
                            has_errors = true;
                            continue;
                        }
                    }
                }

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
