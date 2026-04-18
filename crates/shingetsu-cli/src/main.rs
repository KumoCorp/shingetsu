use anyhow::Context as _;
use clap::{Parser, Subcommand};
use shingetsu::diagnostic::{
    render_compile_error, render_runtime_error, render_warnings, RenderStyle,
};
use shingetsu::{Function, GlobalEnv, Libraries, Task, VmError};
use shingetsu_compiler::{CompileOptions, Compiler};
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

        /// Run in sandboxed mode: only register sandbox-safe libraries
        /// (math, string, table, utf8).  Use --io, --os, --stdio to
        /// selectively re-enable specific libraries.
        #[arg(long)]
        sandboxed: bool,

        /// Enable the `os` library (os.clock, os.time, etc.).
        #[arg(long, requires = "sandboxed")]
        os: bool,

        /// Enable file I/O (io.open, io.tmpfile, etc.) and the filesystem
        /// subset of the os library (os.remove, os.rename, os.tmpname).
        #[arg(long, requires = "sandboxed")]
        io: bool,

        /// Enable stdio handles (io.stdin, io.stdout, io.stderr,
        /// io.read, io.write, io.flush).  Implies --io.
        #[arg(long, requires = "sandboxed")]
        stdio: bool,

        /// Enable process execution (io.popen, os.execute).  Implies --io.
        #[arg(long, requires = "sandboxed")]
        exec: bool,

        /// Enable environment variable access (os.getenv).
        #[arg(long, requires = "sandboxed")]
        env: bool,

        /// Enable process termination (os.exit).
        #[arg(long, requires = "sandboxed")]
        exit: bool,

        /// Enable debug introspection (debug.getlocal, debug.getupvalue,
        /// debug.setupvalue, debug.upvalueid).  The sandbox-safe debug
        /// functions (traceback, info, getinfo) are always available.
        #[arg(long)]
        debug: bool,

        /// Enable file-based `require`: modules are searched via
        /// `package.path` relative to the script's directory.
        #[arg(long, requires = "sandboxed")]
        package: bool,

        /// Set the module search path for file-based `require`.
        /// Semicolon-separated templates where `?` is replaced by the
        /// module name.  Implies --package in sandboxed mode.
        /// Example: `./?.lua;./libs/?.lua`
        #[arg(long)]
        path: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            file,
            sandboxed,
            os,
            io,
            stdio,
            exec,
            env: env_flag,
            exit: exit_flag,
            debug: debug_flag,
            package: package_flag,
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

            let libs = if sandboxed {
                let mut libs = Libraries::SANDBOXED;
                if os {
                    libs |= Libraries::OS;
                }
                if io {
                    libs |= Libraries::IO;
                }
                if stdio {
                    libs |= Libraries::STDIO;
                }
                if exec {
                    libs |= Libraries::EXEC;
                }
                if env_flag {
                    libs |= Libraries::ENV;
                }
                if exit_flag {
                    libs |= Libraries::EXIT;
                }
                if debug_flag {
                    libs |= Libraries::DEBUG;
                }
                if package_flag || path_opt.is_some() {
                    libs |= Libraries::PACKAGE;
                }
                libs
            } else {
                let mut libs = Libraries::ALL;
                if debug_flag {
                    libs |= Libraries::DEBUG;
                }
                libs
            };
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

            let bytecode = match compiler.compile(&source) {
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
    }
}
