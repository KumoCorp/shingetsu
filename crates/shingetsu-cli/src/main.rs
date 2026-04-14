use anyhow::Context as _;
use clap::{Parser, Subcommand};
use shingetsu::{Function, GlobalEnv, Task};
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
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { file } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;

            let opts = CompileOptions {
                debug_info: true,
                source_name: file.display().to_string(),
            };

            let bytecode =
                compile(&source, &opts).with_context(|| format!("compiling {}", file.display()))?;

            let env = GlobalEnv::new();
            shingetsu::builtins::register(&env)?;
            // Load the top-level chunk as a global named "@main".
            // Then create a task and run it.
            let func = Function::lua(bytecode.top_level, vec![]);

            let task = Task::new(env, func, vec![]);
            let results = task.await?;

            for v in &results {
                println!("{v}");
            }

            Ok(())
        }
    }
}
