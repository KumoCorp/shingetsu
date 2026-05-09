//! Helpers shared between the integration tests in this crate.

use shingetsu_docgen::DocModel;
use shingetsu_vm::GlobalEnv;

/// Wrap [`shingetsu_docgen::extract`], dropping the always-on VM
/// builtins module so test fixtures stay focused on what each test
/// actually constructs.
pub fn extract(env: &GlobalEnv) -> DocModel {
    let mut m = shingetsu_docgen::extract(env);
    m.modules.retain(|md| md.name != "builtins");
    m
}
