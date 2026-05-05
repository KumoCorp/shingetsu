//! Compile and run every fenced ` ```lua ` block in the standard
//! library's documentation, asserting that each example executes
//! without error.
//!
//! This catches doc-comment bit-rot: when an example's `assert`
//! becomes false (or the example calls something that no longer
//! exists), the test fails with the rendered diagnostic and the path
//! to the offending item.
//!
//! Examples written in any other language fence (e.g. ` ```text ` for
//! sample output) are skipped \u2014 only ` ```lua ` blocks are executed.

use std::sync::Arc;

use shingetsu::compiler::{CompileOptions, Compiler};
use shingetsu::diagnostic::{render_runtime_error, RenderStyle};
use shingetsu::{valuevec, Function, GlobalEnv, Libraries, Task};
use shingetsu_docgen::{extract, DocModel, FunctionDoc, MetamethodDoc, ModuleDoc, UserdataDoc};

#[tokio::test]
async fn every_documentation_example_executes() {
    // Build the model once from a fully-loaded env so every example
    // sees the same library surface a real script would.
    let env = build_env();
    let model = extract(&env);

    let examples = collect_examples(&model);
    assert!(
        !examples.is_empty(),
        "no examples found — the validator should have something to check"
    );

    let mut failures: Vec<String> = Vec::new();
    for ex in &examples {
        if let Err(diag) = run_example(&ex.code).await {
            failures.push(format!(
                "--- {} (example {} of {}) ---\nsource:\n{}\n\nerror:\n{}",
                ex.path,
                ex.index + 1,
                ex.total_for_item,
                indent(&ex.code, "  "),
                diag,
            ));
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} of {} documentation examples failed:\n\n{}",
            failures.len(),
            examples.len(),
            failures.join("\n\n"),
        );
    }
}

/// One ` ```lua ` block extracted from a doc item's `# Examples`
/// section.
struct Example {
    /// Human-readable path identifying the source item, e.g.
    /// `math.floor` or `file:read`.
    path: String,
    /// 0-based block index within the item's examples.
    index: usize,
    /// Total number of `lua` blocks the item declares.
    total_for_item: usize,
    /// Lua source code, ready to compile.
    code: String,
}

fn build_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    shingetsu::register_libs(&env, Libraries::ALL).expect("register_libs");
    env
}

fn collect_examples(model: &DocModel) -> Vec<Example> {
    let mut out = Vec::new();
    for m in &model.modules {
        collect_module(m, &mut out);
    }
    for u in &model.userdata_types {
        collect_userdata(u, &mut out);
    }
    out
}

fn collect_module(m: &ModuleDoc, out: &mut Vec<Example>) {
    for f in &m.fields {
        push_examples(&format!("{}.{}", m.name, f.name), &f.examples, out);
    }
    for f in &m.functions {
        push_function(&m.name, ".", f, out);
    }
}

fn collect_userdata(u: &UserdataDoc, out: &mut Vec<Example>) {
    for f in &u.fields {
        push_examples(&format!("{}.{}", u.name, f.name), &f.examples, out);
    }
    for f in &u.methods {
        push_function(&u.name, ":", f, out);
    }
    for mm in &u.metamethods {
        push_metamethod(&u.name, mm, out);
    }
}

fn push_function(parent: &str, sep: &str, f: &FunctionDoc, out: &mut Vec<Example>) {
    push_examples(&format!("{parent}{sep}{}", f.name), &f.examples, out);
}

fn push_metamethod(parent: &str, mm: &MetamethodDoc, out: &mut Vec<Example>) {
    push_examples(&format!("{parent}.{}", mm.method), &mm.examples, out);
}

fn push_examples(path: &str, examples: &Option<String>, out: &mut Vec<Example>) {
    let Some(text) = examples else { return };
    let blocks = extract_lua_blocks(text);
    let total = blocks.len();
    for (index, code) in blocks.into_iter().enumerate() {
        out.push(Example {
            path: path.to_owned(),
            index,
            total_for_item: total,
            code,
        });
    }
}

/// Pull every fenced ` ```lua ... ``` ` block out of a markdown
/// string.  Blocks tagged with any other language (e.g. ` ```text `)
/// are skipped.
fn extract_lua_blocks(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut current = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !in_block {
            if trimmed == "```lua" {
                in_block = true;
                current.clear();
            }
        } else if trimmed == "```" {
            in_block = false;
            blocks.push(std::mem::take(&mut current));
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks
}

async fn run_example(src: &str) -> Result<(), String> {
    // Each example gets a fresh env so they can't leak state into
    // each other (e.g. one example calling math.randomseed shouldn't
    // affect a later one).
    let env = build_env();
    let opts = CompileOptions {
        debug_info: true,
        source_name: Arc::new("@example".to_owned()),
        type_check: false,
    };
    let compiler =
        Compiler::new(opts, env.global_type_map()).with_module_types(env.preload_module_types());
    let bytecode = match compiler.compile(src).await {
        Ok(bc) => bc,
        Err(err) => return Err(format!("compile failed: {err:?}")),
    };
    let func = Function::lua(bytecode.top_level, vec![]);
    let task = Task::new(env, func, valuevec![]);
    match task.await {
        Ok(_) => Ok(()),
        Err(err) => Err(render_runtime_error(&err, RenderStyle::Plain)),
    }
}

fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}
