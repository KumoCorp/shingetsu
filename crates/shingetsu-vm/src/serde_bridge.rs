//! Bidirectional bridge between [`Value`] and [`serde_json::Value`].
//!
//! Used by:
//! - [`crate::serde_lua::SerdeLua`] to expose any `Serialize + DeserializeOwned`
//!   Rust type to lua via JSON as the intermediate representation.
//! - Host-side caches that need to capture lua return values into a
//!   representation independent of any specific VM context.
//!
//! ## Conversion semantics
//!
//! Lua → JSON:
//! - `nil` → `null`
//! - `boolean`, `integer`, `float` → matching JSON scalars.  Non-finite
//!   floats (`NaN`, `±Infinity`) are not representable in JSON and
//!   produce an error.
//! - `string` → JSON string.  Non-UTF-8 byte strings produce an error.
//! - `table` with a `[1]` key and a contiguous integer prefix is
//!   emitted as a JSON array (using `sequence_values` semantics).
//!   Otherwise it becomes a JSON object; non-string keys produce an
//!   error.
//! - `function` and `userdata` are not representable; they produce
//!   an error.
//!
//! Cyclic tables (a table that transitively contains itself as a value)
//! produce an error rather than overflowing the stack.
//!
//! JSON → Lua:
//! - `null` → `nil`
//! - `bool` / `string` / numbers → matching lua scalars.  Numbers that
//!   are integral and fit in `i64` become `Value::Integer`; everything
//!   else becomes `Value::Float`.
//! - `array` → table with sequential `1..n` integer keys.
//! - `object` → table with string keys.

use crate::error::VmError;
use crate::table::Table;
use crate::value::Value;
use std::collections::HashSet;

/// Convert a [`Value`] into a [`serde_json::Value`].
///
/// See module docs for the exact mapping.  Errors are returned as
/// [`VmError::HostError`] with a descriptive `name` indicating where
/// in the conversion the failure occurred.
pub fn value_to_json(value: &Value) -> Result<serde_json::Value, VmError> {
    let mut visited = HashSet::new();
    value_to_json_inner(value, &mut visited)
}

fn value_to_json_inner(
    value: &Value,
    visited: &mut HashSet<usize>,
) -> Result<serde_json::Value, VmError> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        Value::Integer(n) => Ok(serde_json::Value::Number((*n).into())),
        Value::Float(f) => match serde_json::Number::from_f64(*f) {
            Some(n) => Ok(serde_json::Value::Number(n)),
            None => Err(host_err(
                "value_to_json",
                format!("cannot represent non-finite float {f} as JSON"),
            )),
        },
        Value::String(b) => match std::str::from_utf8(b.as_ref()) {
            Ok(s) => Ok(serde_json::Value::String(s.to_owned())),
            Err(_) => Err(host_err(
                "value_to_json",
                "string is not valid UTF-8 and cannot be encoded as JSON".to_owned(),
            )),
        },
        Value::Table(t) => table_to_json(t, visited),
        Value::Function(_) => Err(host_err(
            "value_to_json",
            "function values cannot be encoded as JSON".to_owned(),
        )),
        Value::Userdata(_) => Err(host_err(
            "value_to_json",
            "userdata values cannot be encoded as JSON".to_owned(),
        )),
    }
}

fn table_to_json(
    table: &Table,
    visited: &mut HashSet<usize>,
) -> Result<serde_json::Value, VmError> {
    let id = table.identity();
    if !visited.insert(id) {
        return Err(host_err(
            "value_to_json",
            "cyclic table cannot be encoded as JSON".to_owned(),
        ));
    }

    if let crate::table::TableShape::Vec { len } = table.detect_shape()? {
        let mut out = Vec::with_capacity(len);
        for i in 1..=len {
            let v = table.raw_get(&Value::Integer(i as i64))?;
            out.push(value_to_json_inner(&v, visited)?);
        }
        visited.remove(&id);
        return Ok(serde_json::Value::Array(out));
    }

    let mut out = serde_json::Map::new();
    let mut k = Value::Nil;
    while let Some((nk, nv)) = table.next(&k)? {
        let key_str = match &nk {
            Value::String(b) => match std::str::from_utf8(b.as_ref()) {
                Ok(s) => s.to_owned(),
                Err(_) => {
                    return Err(host_err(
                        "value_to_json",
                        "object key is not valid UTF-8".to_owned(),
                    ));
                }
            },
            other => {
                return Err(host_err(
                    "value_to_json",
                    format!(
                        "JSON object keys must be strings; got {} key in mixed-shape table",
                        other.type_name()
                    ),
                ));
            }
        };
        out.insert(key_str, value_to_json_inner(&nv, visited)?);
        k = nk;
    }

    visited.remove(&id);
    Ok(serde_json::Value::Object(out))
}

/// Convert a [`serde_json::Value`] into a lua [`Value`].
///
/// See module docs for the exact mapping.  Allocates fresh
/// [`Table`]s as needed; does not require a [`crate::GlobalEnv`].
pub fn value_from_json(json: serde_json::Value) -> Result<Value, VmError> {
    Ok(match json {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                // Spec-allowed but rare: u64 > i64::MAX.  Fall back to f64
                // (lossy) since lua has no u64 representation.
                let f = n.as_f64().ok_or_else(|| {
                    host_err(
                        "value_from_json",
                        format!("number {n} is not representable as i64 or f64"),
                    )
                })?;
                Value::Float(f)
            }
        }
        serde_json::Value::String(s) => Value::string(s),
        serde_json::Value::Array(items) => {
            let table = Table::new();
            for (idx, item) in items.into_iter().enumerate() {
                let v = value_from_json(item)?;
                table.raw_set(Value::Integer((idx + 1) as i64), v)?;
            }
            Value::Table(table)
        }
        serde_json::Value::Object(map) => {
            let table = Table::new();
            for (k, v) in map {
                let key = Value::string(k);
                let val = value_from_json(v)?;
                table.raw_set(key, val)?;
            }
            Value::Table(table)
        }
    })
}

fn host_err(name: &'static str, msg: String) -> VmError {
    VmError::HostError {
        name: name.to_owned(),
        source: msg.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_round_trip() {
        for v in [
            Value::Nil,
            Value::Boolean(true),
            Value::Boolean(false),
            Value::Integer(42),
            Value::Integer(-7),
            Value::Float(3.5),
            Value::String("hello".into()),
        ] {
            let j = value_to_json(&v).expect("to_json");
            let back = value_from_json(j).expect("from_json");
            k9::assert_equal!(format!("{back:?}"), format!("{v:?}"));
        }
    }

    #[test]
    fn float_nan_errors() {
        let err = value_to_json(&Value::Float(f64::NAN)).expect_err("nan");
        let rendered = format!("{err}");
        k9::assert_equal!(
            rendered,
            "error in 'value_to_json': cannot represent non-finite float NaN as JSON"
        );
    }

    #[test]
    fn array_shape_table_becomes_json_array() {
        let t = Table::new();
        t.raw_set(Value::Integer(1), Value::Integer(10)).unwrap();
        t.raw_set(Value::Integer(2), Value::Integer(20)).unwrap();
        t.raw_set(Value::Integer(3), Value::Integer(30)).unwrap();
        let j = value_to_json(&Value::Table(t)).expect("to_json");
        k9::assert_equal!(j, serde_json::json!([10, 20, 30]));
    }

    #[test]
    fn string_key_table_becomes_json_object() {
        let t = Table::new();
        t.raw_set(Value::string("a"), Value::Integer(1)).unwrap();
        t.raw_set(Value::string("b"), Value::Integer(2)).unwrap();
        let j = value_to_json(&Value::Table(t)).expect("to_json");
        k9::assert_equal!(j, serde_json::json!({"a": 1, "b": 2}));
    }

    #[test]
    fn mixed_int_string_key_errors() {
        let t = Table::new();
        t.raw_set(Value::Integer(1), Value::Integer(10)).unwrap();
        t.raw_set(Value::string("name"), Value::string("x"))
            .unwrap();
        // Has [1] but raw_len == 1 != 2 total keys, so falls into object path
        // and rejects the integer key.
        let err = value_to_json(&Value::Table(t)).expect_err("mixed");
        let rendered = format!("{err}");
        k9::assert_equal!(
            rendered,
            "error in 'value_to_json': JSON object keys must be strings; \
             got number key in mixed-shape table"
        );
    }

    #[test]
    fn cyclic_table_errors() {
        let t = Table::new();
        t.raw_set(Value::string("self"), Value::Table(t.clone()))
            .unwrap();
        let err = value_to_json(&Value::Table(t)).expect_err("cycle");
        let rendered = format!("{err}");
        k9::assert_equal!(
            rendered,
            "error in 'value_to_json': cyclic table cannot be encoded as JSON"
        );
    }

    #[test]
    fn function_value_errors() {
        let f = crate::Function::wrap("f", || -> Result<(), VmError> { Ok(()) });
        let err = value_to_json(&Value::Function(f)).expect_err("function");
        k9::assert_equal!(
            format!("{err}"),
            "error in 'value_to_json': function values cannot be encoded as JSON"
        );
    }

    #[test]
    fn json_array_round_trips_to_array_table() {
        let j = serde_json::json!([1, 2, "three"]);
        let v = value_from_json(j.clone()).expect("from_json");
        let back = value_to_json(&v).expect("to_json");
        k9::assert_equal!(back, j);
    }

    #[test]
    fn json_object_round_trips() {
        let j = serde_json::json!({"name": "x", "age": 7});
        let v = value_from_json(j.clone()).expect("from_json");
        let back = value_to_json(&v).expect("to_json");
        k9::assert_equal!(back, j);
    }

    #[test]
    fn nested_round_trip() {
        // JSON `null` values become lua `nil` on `from_json`, which lua
        // semantics treats as key removal.  The round trip is
        // therefore lossy for keys whose value is null — documented
        // behavior, not a bug.
        let j = serde_json::json!({
            "items": [{"k": 1}, {"k": 2}],
            "name": "wide",
            "ok": true,
        });
        let v = value_from_json(j.clone()).expect("from_json");
        let back = value_to_json(&v).expect("to_json");
        k9::assert_equal!(back, j);
    }

    #[test]
    fn null_value_in_object_is_dropped_on_round_trip() {
        // Documents the lossy behavior above.
        let j = serde_json::json!({"keep": 1, "drop": null});
        let v = value_from_json(j).expect("from_json");
        let back = value_to_json(&v).expect("to_json");
        k9::assert_equal!(back, serde_json::json!({"keep": 1}));
    }
}
