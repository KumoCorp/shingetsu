//! Lua `table` standard library.
//!
//! Registered as a global `table` table.  Provides sequential operations
//! (`insert`, `remove`, `concat`), sorting, and packing/unpacking.

use bytes::Bytes;

use crate::convert::FromLua;
use crate::error::VmError;
use crate::table::Table;
use crate::value::Value;

/// Patch a `VmError::BadArgument` with a specific position and function name.
fn patch_arg(e: VmError, position: usize, function: &str) -> VmError {
    match e {
        VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
            position,
            function: function.to_owned(),
            expected,
            got,
        },
        other => other,
    }
}

/// Create a `VmError` for runtime errors.
fn runtime_error(msg: String) -> VmError {
    VmError::LuaError {
        display: msg.clone(),
        value: Value::String(Bytes::from(msg)),
    }
}

/// `table.insert(t, [pos,] value)`
///
/// If `pos` is given, inserts `value` at position `pos`, shifting elements
/// up.  Otherwise appends `value` at the end (`#t + 1`).
fn table_insert(args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    let n = args.len();
    if n < 2 {
        return Err(VmError::BadArgument {
            position: if n == 0 { 1 } else { 2 },
            function: "insert".to_owned(),
            expected: "value".to_owned(),
            got: "no value".to_owned(),
        });
    }

    let t = Table::from_lua(args[0].clone()).map_err(|e| patch_arg(e, 1, "insert"))?;

    if n == 2 {
        // table.insert(t, value) — append.
        let len = t.raw_len() as usize;
        t.raw_insert(len + 1, args[1].clone());
    } else {
        // table.insert(t, pos, value)
        let pos = i64::from_lua(args[1].clone()).map_err(|e| patch_arg(e, 2, "insert"))?;
        let len = t.raw_len();
        if pos < 1 || pos > len + 1 {
            return Err(runtime_error(format!(
                "bad argument #2 to 'insert' (position out of bounds: {} not in [1, {}])",
                pos,
                len + 1
            )));
        }
        t.raw_insert(pos as usize, args[2].clone());
    }

    Ok(vec![])
}

/// `table.remove(t [, pos])`
///
/// Removes the element at position `pos` (default `#t`), shifting elements
/// down.  Returns the removed value.
fn table_remove(args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    if args.is_empty() {
        return Err(VmError::BadArgument {
            position: 1,
            function: "remove".to_owned(),
            expected: "table".to_owned(),
            got: "no value".to_owned(),
        });
    }

    let t = Table::from_lua(args[0].clone()).map_err(|e| patch_arg(e, 1, "remove"))?;
    let len = t.raw_len();

    let pos = if args.len() >= 2 {
        i64::from_lua(args[1].clone()).map_err(|e| patch_arg(e, 2, "remove"))?
    } else {
        len
    };

    if len == 0 && args.len() < 2 {
        // Removing from an empty table with no explicit pos returns nil.
        return Ok(vec![Value::Nil]);
    }

    if pos < 1 || pos > len {
        return Err(runtime_error(format!(
            "bad argument #2 to 'remove' (position out of bounds: {} not in [1, {}])",
            pos, len
        )));
    }

    let removed = t.raw_remove(pos as usize);
    Ok(vec![removed])
}

/// `table.concat(t [, sep [, i [, j]]])`
///
/// Concatenates the string representations of `t[i]` through `t[j]` with
/// `sep` between them.  Defaults: `sep=""`, `i=1`, `j=#t`.
fn table_concat(args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    if args.is_empty() {
        return Err(VmError::BadArgument {
            position: 1,
            function: "concat".to_owned(),
            expected: "table".to_owned(),
            got: "no value".to_owned(),
        });
    }

    let t = Table::from_lua(args[0].clone()).map_err(|e| patch_arg(e, 1, "concat"))?;

    let sep = if args.len() >= 2 && !args[1].is_nil() {
        let s = Bytes::from_lua(args[1].clone()).map_err(|e| patch_arg(e, 2, "concat"))?;
        s
    } else {
        Bytes::new()
    };

    let len = t.raw_len();

    let i = if args.len() >= 3 && !args[2].is_nil() {
        i64::from_lua(args[2].clone()).map_err(|e| patch_arg(e, 3, "concat"))?
    } else {
        1
    };

    let j = if args.len() >= 4 && !args[3].is_nil() {
        i64::from_lua(args[3].clone()).map_err(|e| patch_arg(e, 4, "concat"))?
    } else {
        len
    };

    if i > j {
        return Ok(vec![Value::String(Bytes::new())]);
    }

    let mut result = Vec::new();
    for idx in i..=j {
        if idx > i {
            result.extend_from_slice(&sep);
        }
        let key = Value::Integer(idx);
        let val = t.raw_get(&key)?;
        match &val {
            Value::String(s) => result.extend_from_slice(s),
            Value::Integer(n) => result.extend_from_slice(n.to_string().as_bytes()),
            Value::Float(f) => result.extend_from_slice(format!("{}", f).as_bytes()),
            _ => {
                return Err(runtime_error(format!(
                    "invalid value ({}) at index {} in table for 'concat'",
                    val.type_name(),
                    idx
                )));
            }
        }
    }

    Ok(vec![Value::String(Bytes::from(result))])
}

// =========================================================================
// Registration
// =========================================================================

use std::sync::Arc;

use crate::function::{Function, NativeFunction};
use crate::types::FunctionSignature;

/// Helper: wrap a Rust closure as a `Value::Function`.
fn wrap_native<F>(name: &'static [u8], f: F) -> Value
where
    F: Fn(Vec<Value>) -> Result<Vec<Value>, VmError> + Send + Sync + 'static,
{
    Value::Function(Function::native(NativeFunction {
        signature: Arc::new(FunctionSignature {
            name: Bytes::from_static(name),
            type_params: vec![],
            params: vec![],
            variadic: true,
            returns: None,
            lua_returns: None,
        }),
        call: Arc::new(move |_ctx, args| {
            let result = f(args);
            Box::pin(async move { result })
        }),
    }))
}

/// Build the table library table and register it as the `table` global.
pub fn register(env: &crate::GlobalEnv) -> Result<(), VmError> {
    let table = Table::new();

    table.raw_set(
        Value::String(Bytes::from_static(b"insert")),
        wrap_native(b"insert", table_insert),
    )?;

    table.raw_set(
        Value::String(Bytes::from_static(b"remove")),
        wrap_native(b"remove", table_remove),
    )?;

    table.raw_set(
        Value::String(Bytes::from_static(b"concat")),
        wrap_native(b"concat", table_concat),
    )?;

    env.set_global("table", Value::Table(table));

    Ok(())
}
