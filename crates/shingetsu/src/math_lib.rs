//! Lua `math` standard library.
//!
//! Registered as a global `math` table.

use crate::error::VmError;
use crate::value::Value;

#[crate::module(name = "math")]
pub mod math_mod {
    #[field]
    fn pi() -> f64 {
        std::f64::consts::PI
    }

    #[field]
    fn huge() -> f64 {
        f64::INFINITY
    }

    #[field]
    fn maxinteger() -> i64 {
        i64::MAX
    }

    #[field]
    fn mininteger() -> i64 {
        i64::MIN
    }
}

/// Build the math library table and register it as the `math` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = math_mod::build_module_table(env)?;
    env.set_global("math", Value::Table(table));
    Ok(())
}
