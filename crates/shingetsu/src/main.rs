use anyhow::Context as _;
use clap::{Parser, Subcommand};
use shingetsu_compiler::{compile, CompileOptions, Dialect};
use shingetsu_vm::GlobalEnv;
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
        /// Use LuaU dialect instead of Lua 5.4.
        #[arg(long)]
        luau: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { file, luau } => {
            let source = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;

            let opts = CompileOptions {
                dialect: if luau { Dialect::LuaU } else { Dialect::Lua54 },
                debug_info: true,
                source_name: file.display().to_string(),
            };

            let bytecode =
                compile(&source, &opts).with_context(|| format!("compiling {}", file.display()))?;

            let env = GlobalEnv::new();
            // Load the top-level chunk as a global named "@main".
            // Then create a task and run it.
            let func = shingetsu_vm::Function::lua(bytecode.top_level, vec![]);

            let task = shingetsu_vm::Task::new(env, func, vec![]);
            let results = task.await?;

            for v in &results {
                println!("{v}");
            }

            Ok(())
        }
    }
}
