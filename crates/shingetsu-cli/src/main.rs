use anyhow::Context as _;
use clap::{Parser, Subcommand};
use shingetsu::{Function, GlobalEnv, Libraries, Task};
use shingetsu_compiler::{compile, CompileOptions};
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
        } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;

            let opts = CompileOptions {
                debug_info: true,
                source_name: file.display().to_string(),
            };

            let bytecode =
                compile(&source, &opts).with_context(|| format!("compiling {}", file.display()))?;

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
                libs
            } else {
                Libraries::ALL
            };
            shingetsu::register_libs(&env, libs)?;

            // Load the top-level chunk as a global named "@main".
            // Then create a task and run it.
            let func = Function::lua(bytecode.top_level, vec![]);

            let task = Task::new(env, func, vec![]);
            let results = task.await?;

            shingetsu::io_lib::flush_stdio().await;

            for v in &results {
                println!("{v}");
            }

            Ok(())
        }
    }
}
