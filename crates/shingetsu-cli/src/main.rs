use anyhow::Context as _;
use clap::{Parser, Subcommand};
use shingetsu::diagnostic::{
    render_compile_error, render_runtime_error, render_warnings, RenderStyle,
};
use shingetsu::{Function, GlobalEnv, Libraries, Task, VmError};
use shingetsu_compiler::{CompileOptions, Compiler, Severity};
use std::path::PathBuf;

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

        /// Set the module search path for file-based `require`.
        /// Semicolon-separated templates where `?` is replaced by the
        /// module name.  Example: `./?.lua;./libs/?.lua`
        #[arg(long)]
        path: Option<String>,
    },

    /// Type-check a Lua script without executing it.
    Check {
        /// Path to the Lua source file.
        file: PathBuf,

        #[command(flatten)]
        lib_opts: LibraryOpts,
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

fn parse_libraries(s: &str) -> Result<Libraries, String> {
    s.parse()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            file,
            lib_opts,
            path: path_opt,
        } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;

            let opts = CompileOptions {
                debug_info: true,
                source_name: file.display().to_string(),
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

            let compiler = Compiler::new(opts, env.global_type_map());

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

            // Print any compiler warnings before running.
            if !bytecode.diagnostics.is_empty() {
                eprintln!("{}", render_warnings(&bytecode.diagnostics, &source, style));
            }

            // Load the top-level chunk as a global named "@main".
            // Then create a task and run it.
            let func = Function::lua(bytecode.top_level, vec![]);

            // Keep a handle to the env for the ExitRequested path — we
            // may need to run `__gc` finalizers via `dispose()` after
            // the task returns.
            let env_for_exit = env.clone();
            let task = Task::new(env, func, vec![]);
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
                    shingetsu::io_lib::flush_stdio().await;
                    std::process::exit(code);
                }
                Err(re) => {
                    eprint!("{}", render_runtime_error(&re, style));
                    std::process::exit(1);
                }
            };

            shingetsu::io_lib::flush_stdio().await;

            for v in &results {
                println!("{v}");
            }

            Ok(())
        }

        Command::Check { file, lib_opts } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;

            let opts = CompileOptions {
                debug_info: true,
                source_name: file.display().to_string(),
                type_check: true,
            };

            let env = GlobalEnv::new();
            shingetsu::register_libs(&env, lib_opts.resolve())?;

            let compiler = Compiler::new(opts, env.global_type_map());

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

            if !bytecode.diagnostics.is_empty() {
                eprintln!("{}", render_warnings(&bytecode.diagnostics, &source, style));
            }

            let has_errors = bytecode
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error);
            if has_errors {
                std::process::exit(1);
            }

            Ok(())
        }
    }
}
