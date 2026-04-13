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

/// `table.sort(t [, comp])`
///
/// Sorts the sequence part of `t` in place.  If `comp` is given it must be
/// a function that receives two elements and returns `true` when the first
/// should come before the second.  Otherwise the default `<` order is used.
///
/// Because `comp` may be a Lua function (requiring async dispatch through
/// the VM), we use an insertion sort that `await`s each comparison.
async fn table_sort(ctx: crate::CallContext, args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    if args.is_empty() {
        return Err(VmError::BadArgument {
            position: 1,
            function: "sort".to_owned(),
            expected: "table".to_owned(),
            got: "no value".to_owned(),
        });
    }

    let t = Table::from_lua(args[0].clone()).map_err(|e| patch_arg(e, 1, "sort"))?;
    let comp = if args.len() >= 2 && !args[1].is_nil() {
        Some(crate::Function::from_lua(args[1].clone()).map_err(|e| patch_arg(e, 2, "sort"))?)
    } else {
        None
    };

    // Swap the array out of the table so we can sort in place without
    // cloning.  The table's sequence part is temporarily empty; since Lua
    // execution is single-threaded within a Task this is safe.
    let mut arr = Vec::new();
    t.swap_array(&mut arr);

    // Trim trailing nils — only sort the non-nil prefix.
    while matches!(arr.last(), Some(Value::Nil)) {
        arr.pop();
    }

    let n = arr.len();
    if n > 1 {
        if let Some(comp) = comp {
            // Lua comparator — merge sort with async comparisons.
            let result = async_merge_sort(&mut arr, &ctx, &comp).await;
            if let Err(e) = result {
                // Put the (partially sorted) array back before propagating.
                t.swap_array(&mut arr);
                return Err(e);
            }
        } else {
            // Default `<` order — sort in place synchronously.
            let mut err: Option<VmError> = None;
            arr.sort_by(|a, b| match default_lt(a, b) {
                Ok(true) => std::cmp::Ordering::Less,
                Ok(false) => std::cmp::Ordering::Greater,
                Err(e) => {
                    err.get_or_insert(e);
                    std::cmp::Ordering::Equal
                }
            });
            if let Some(e) = err {
                t.swap_array(&mut arr);
                return Err(e);
            }
        }
    }

    // Put the sorted array back.
    t.swap_array(&mut arr);

    Ok(vec![])
}

/// Async merge sort — O(n log n) comparisons through a Lua function.
async fn async_merge_sort(
    arr: &mut [Value],
    ctx: &crate::CallContext,
    comp: &crate::Function,
) -> Result<(), VmError> {
    let n = arr.len();
    if n <= 1 {
        return Ok(());
    }
    let mid = n / 2;

    // Recurse on each half.
    // Box::pin to avoid infinite-size future from recursion.
    Box::pin(async_merge_sort(&mut arr[..mid], ctx, comp)).await?;
    Box::pin(async_merge_sort(&mut arr[mid..], ctx, comp)).await?;

    // Merge the two sorted halves into a temporary buffer.
    let left = arr[..mid].to_vec();
    let right = arr[mid..].to_vec();
    let mut i = 0;
    let mut j = 0;
    let mut k = 0;

    while i < left.len() && j < right.len() {
        let result = ctx
            .call_function(comp.clone(), vec![left[i].clone(), right[j].clone()])
            .await?;
        let left_first = match result.first() {
            Some(Value::Boolean(false)) | Some(Value::Nil) | None => false,
            _ => true,
        };
        if left_first {
            arr[k] = left[i].clone();
            i += 1;
        } else {
            arr[k] = right[j].clone();
            j += 1;
        }
        k += 1;
    }

    while i < left.len() {
        arr[k] = left[i].clone();
        i += 1;
        k += 1;
    }
    while j < right.len() {
        arr[k] = right[j].clone();
        j += 1;
        k += 1;
    }

    Ok(())
}

/// `table.move(a1, f, e, t [, a2])`
///
/// Copies elements from table `a1` (indices `f` through `e`) into table
/// `a2` starting at index `t`.  The default for `a2` is `a1`.  The
/// destination range may overlap with the source range.  Returns `a2`.
fn table_move(args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    if args.len() < 4 {
        return Err(VmError::BadArgument {
            position: args.len() + 1,
            function: "move".to_owned(),
            expected: "number".to_owned(),
            got: "no value".to_owned(),
        });
    }

    let a1 = Table::from_lua(args[0].clone()).map_err(|e| patch_arg(e, 1, "move"))?;
    let f = i64::from_lua(args[1].clone()).map_err(|e| patch_arg(e, 2, "move"))?;
    let e = i64::from_lua(args[2].clone()).map_err(|e| patch_arg(e, 3, "move"))?;
    let t_idx = i64::from_lua(args[3].clone()).map_err(|e| patch_arg(e, 4, "move"))?;
    let a2 = if args.len() >= 5 && !args[4].is_nil() {
        Table::from_lua(args[4].clone()).map_err(|e| patch_arg(e, 5, "move"))?
    } else {
        a1.clone()
    };

    if f > e {
        return Ok(vec![Value::Table(a2)]);
    }

    // Collect source values first so overlapping src/dst in the same table
    // works correctly.
    let values: Vec<Value> = (f..=e)
        .map(|i| a1.raw_get(&Value::Integer(i)))
        .collect::<Result<_, _>>()?;

    for (offset, val) in values.into_iter().enumerate() {
        a2.raw_set(Value::Integer(t_idx + offset as i64), val)?;
    }

    Ok(vec![Value::Table(a2)])
}

/// `table.pack(...)`
///
/// Returns a new table with all arguments stored in keys 1, 2, ..., plus
/// a field `"n"` with the total number of arguments.
fn table_pack(args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    let t = Table::new();
    let n = args.len() as i64;
    for (i, v) in args.into_iter().enumerate() {
        t.raw_set(Value::Integer(i as i64 + 1), v)?;
    }
    t.raw_set(Value::String(Bytes::from_static(b"n")), Value::Integer(n))?;
    Ok(vec![Value::Table(t)])
}

/// `table.unpack(list [, i [, j]])`
///
/// Returns `list[i], list[i+1], ..., list[j]`.  Defaults: `i=1`, `j=#list`.
fn table_unpack(args: Vec<Value>) -> Result<Vec<Value>, VmError> {
    if args.is_empty() {
        return Err(VmError::BadArgument {
            position: 1,
            function: "unpack".to_owned(),
            expected: "table".to_owned(),
            got: "no value".to_owned(),
        });
    }

    let t = Table::from_lua(args[0].clone()).map_err(|e| patch_arg(e, 1, "unpack"))?;
    let len = t.raw_len();

    let i = if args.len() >= 2 && !args[1].is_nil() {
        i64::from_lua(args[1].clone()).map_err(|e| patch_arg(e, 2, "unpack"))?
    } else {
        1
    };

    let j = if args.len() >= 3 && !args[2].is_nil() {
        i64::from_lua(args[2].clone()).map_err(|e| patch_arg(e, 3, "unpack"))?
    } else {
        len
    };

    if i > j {
        return Ok(vec![]);
    }

    let mut result = Vec::with_capacity((j - i + 1) as usize);
    for idx in i..=j {
        result.push(t.raw_get(&Value::Integer(idx))?);
    }

    Ok(result)
}

/// Default less-than comparison for `table.sort`.
fn default_lt(a: &Value, b: &Value) -> Result<bool, VmError> {
    match (a, b) {
        (Value::Integer(x), Value::Integer(y)) => Ok(x < y),
        (Value::Float(x), Value::Float(y)) => Ok(x < y),
        (Value::Integer(x), Value::Float(y)) => Ok((*x as f64) < *y),
        (Value::Float(x), Value::Integer(y)) => Ok(*x < (*y as f64)),
        (Value::String(x), Value::String(y)) => Ok(x < y),
        _ => Err(runtime_error(format!(
            "attempt to compare {} with {}",
            a.type_name(),
            b.type_name()
        ))),
    }
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

    table.raw_set(
        Value::String(Bytes::from_static(b"sort")),
        Value::Function(Function::native(NativeFunction {
            signature: Arc::new(FunctionSignature {
                name: Bytes::from_static(b"sort"),
                type_params: vec![],
                params: vec![],
                variadic: true,
                returns: None,
                lua_returns: None,
            }),
            call: Arc::new(|ctx, args| Box::pin(table_sort(ctx, args))),
        })),
    )?;

    table.raw_set(
        Value::String(Bytes::from_static(b"move")),
        wrap_native(b"move", table_move),
    )?;

    table.raw_set(
        Value::String(Bytes::from_static(b"pack")),
        wrap_native(b"pack", table_pack),
    )?;

    let unpack = wrap_native(b"unpack", table_unpack);
    table.raw_set(Value::String(Bytes::from_static(b"unpack")), unpack.clone())?;

    env.set_global("table", Value::Table(table));

    // Also register as global `unpack` for Lua 5.1 compat.
    env.set_global("unpack", unpack);

    Ok(())
}
