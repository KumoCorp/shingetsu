//! Run every executable example in a [`crate::DocModel`] and store
//! the captured stdout into each [`crate::DocExample::output`] field.
//!
//! Recognised fence flags:
//!
//! - `no_run` — skip the example entirely.  Useful when the example
//!   has side effects, can't be run in isolation, or is purely
//!   illustrative.
//! - `no_capture` — run the example to verify it doesn't error,
//!   but discard its captured stdout.  Useful when the output
//!   varies between runs (random numbers, current time, generated
//!   paths, …) and including it in the rendered docs would just
//!   churn.
//!
//! Non-`lua` fences are also skipped (they're typically `text`
//! blocks showing sample output rather than runnable code).

use std::sync::Arc;

use shingetsu::compiler::{CompileOptions, Compiler};
use shingetsu::diagnostic::{render_runtime_error, RenderStyle};
use shingetsu::{valuevec, Function, GlobalEnv, Libraries, PrintCapture, Task};

use crate::{DocExample, DocModel};

/// Outcome for a single example run.
#[derive(Debug, Clone, PartialEq)]
pub enum ExampleOutcome {
    /// The example ran to completion.  `output` is whatever it
    /// printed (an empty string when nothing was printed).
    Ok { output: String },
    /// The example was not executed (skipped flag, non-`lua` fence,
    /// etc.).  No output captured.
    Skipped,
    /// Compile or runtime error.  The fully rendered diagnostic
    /// string is included so callers can surface a clear failure
    /// message.
    Failed { diagnostic: String },
}

/// Per-example failure record returned by
/// [`populate_example_outputs`] when one or more examples fail to
/// compile or run.
#[derive(Debug, Clone)]
pub struct ExampleFailure {
    /// Dotted path identifying the source item, e.g.
    /// `math.floor` or `file:read`.
    pub path: String,
    /// 0-based block index within the item's examples.
    pub index: usize,
    /// Number of `lua` blocks the item declares.
    pub total: usize,
    /// Rendered diagnostic.
    pub diagnostic: String,
    /// The example's source code, for inclusion in error reports.
    pub code: String,
}

/// Walk the model, run every executable `lua` example in a fresh
/// [`GlobalEnv`], and store the captured stdout in each example's
/// `output` field.  Examples are run with all standard libraries
/// available.
///
/// Returns the list of examples that failed to compile or run; a
/// successful run returns an empty `Vec`.  All non-failing examples
/// have their `output` populated regardless of whether other
/// examples failed.
pub async fn populate_example_outputs(model: &mut DocModel) -> Vec<ExampleFailure> {
    let mut failures = Vec::new();
    for m in &mut model.modules {
        for f in &mut m.fields {
            run_examples(
                &format!("{}.{}", m.name, f.name),
                &mut f.examples,
                &mut failures,
            )
            .await;
        }
        for f in &mut m.functions {
            run_examples(
                &format!("{}.{}", m.name, f.name),
                &mut f.examples,
                &mut failures,
            )
            .await;
        }
    }
    for u in &mut model.userdata_types {
        for f in &mut u.fields {
            run_examples(
                &format!("{}.{}", u.name, f.name),
                &mut f.examples,
                &mut failures,
            )
            .await;
        }
        for f in &mut u.methods {
            run_examples(
                &format!("{}:{}", u.name, f.name),
                &mut f.examples,
                &mut failures,
            )
            .await;
        }
        for mm in &mut u.metamethods {
            run_examples(
                &format!("{}.{}", u.name, mm.method),
                &mut mm.examples,
                &mut failures,
            )
            .await;
        }
    }
    failures
}

async fn run_examples(path: &str, examples: &mut [DocExample], failures: &mut Vec<ExampleFailure>) {
    let total = examples.len();
    for (index, ex) in examples.iter_mut().enumerate() {
        match run_one(ex).await {
            ExampleOutcome::Ok { output } => {
                ex.output = Some(output);
            }
            ExampleOutcome::Skipped => {
                // Leave output as None.
            }
            ExampleOutcome::Failed { diagnostic } => {
                failures.push(ExampleFailure {
                    path: path.to_owned(),
                    index,
                    total,
                    diagnostic,
                    code: ex.code.clone(),
                });
            }
        }
    }
}

async fn run_one(ex: &DocExample) -> ExampleOutcome {
    if ex.language != "lua" || ex.flags.iter().any(|f| f == "no_run") {
        return ExampleOutcome::Skipped;
    }
    let suppress_output = ex.flags.iter().any(|f| f == "no_capture");

    // Each example gets its own env so they can't leak state into
    // each other (e.g. one example calling math.randomseed must not
    // affect a later one).
    let env = GlobalEnv::new();
    if let Err(err) = shingetsu::register_libs(&env, Libraries::ALL) {
        return ExampleOutcome::Failed {
            diagnostic: format!("register_libs failed: {err:?}"),
        };
    }
    let capture = env.extension_or_init::<PrintCapture, _>(PrintCapture::new);

    let opts = CompileOptions {
        debug_info: true,
        source_name: Arc::new("@example".to_owned()),
        type_check: false,
    };
    let compiler =
        Compiler::new(opts, env.global_type_map()).with_module_types(env.preload_module_types());
    let bytecode = match compiler.compile(&ex.code).await {
        Ok(bc) => bc,
        Err(err) => {
            return ExampleOutcome::Failed {
                diagnostic: format!("compile failed: {err:?}"),
            }
        }
    };
    let func = Function::lua(bytecode.top_level, vec![]);
    let task = Task::new(env, func, valuevec![]);
    match task.await {
        Ok(_) => {
            let output = if suppress_output {
                String::new()
            } else {
                capture.take()
            };
            ExampleOutcome::Ok { output }
        }
        Err(err) => ExampleOutcome::Failed {
            diagnostic: render_runtime_error(&err, RenderStyle::Plain),
        },
    }
}
