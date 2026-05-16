//! First-class `serde::Serializer` producing a [`Value`] directly,
//! without an intermediate `serde_json::Value`.
//!
//! Output is byte-for-byte identical to
//! `serde_bridge::value_from_json(serde_json::to_value(t)?)` — the
//! same structural choices serde_json makes (sequences/tuples become
//! 1-indexed tables, structs/maps become string-keyed tables,
//! externally-tagged enums, non-finite floats collapse to `Nil`) —
//! but a single serde pass.  Because it *is* serde, every
//! `#[serde(...)]` attribute (`rename`, `flatten`, `with`,
//! `skip_serializing_if`, `rename_all`, …) is honored exactly.

use crate::error::VmError;
use crate::table::Table;
use crate::value::Value;
use serde::ser::{self, Serialize};
use std::fmt::Display;

/// Serialize any `Serialize` value straight to a [`Value`].
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, VmError> {
    value.serialize(Serializer)
}

impl ser::Error for VmError {
    fn custom<T: Display>(msg: T) -> Self {
        VmError::HostError {
            name: "serde_ser".to_owned(),
            source: msg.to_string().into(),
        }
    }
}

fn int_value(i: i64) -> Value {
    Value::Integer(i)
}

fn float_value(f: f64) -> Value {
    // Mirror serde_json: non-finite floats serialize as `null`,
    // which `value_from_json` turns into `Nil`.
    if f.is_finite() {
        Value::Float(f)
    } else {
        Value::Nil
    }
}

struct Serializer;

impl ser::Serializer for Serializer {
    type Ok = Value;
    type Error = VmError;

    type SerializeSeq = SeqSerializer;
    type SerializeTuple = SeqSerializer;
    type SerializeTupleStruct = SeqSerializer;
    type SerializeTupleVariant = TupleVariantSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = StructSerializer;
    type SerializeStructVariant = StructVariantSerializer;

    fn serialize_bool(self, v: bool) -> Result<Value, VmError> {
        Ok(Value::Boolean(v))
    }

    fn serialize_i8(self, v: i8) -> Result<Value, VmError> {
        Ok(int_value(v as i64))
    }
    fn serialize_i16(self, v: i16) -> Result<Value, VmError> {
        Ok(int_value(v as i64))
    }
    fn serialize_i32(self, v: i32) -> Result<Value, VmError> {
        Ok(int_value(v as i64))
    }
    fn serialize_i64(self, v: i64) -> Result<Value, VmError> {
        Ok(int_value(v))
    }
    fn serialize_i128(self, _v: i128) -> Result<Value, VmError> {
        // serde_json (without `arbitrary_precision`) rejects i128;
        // preserve that behavior.
        Err(<VmError as ser::Error>::custom("i128 is not supported"))
    }

    fn serialize_u8(self, v: u8) -> Result<Value, VmError> {
        Ok(int_value(v as i64))
    }
    fn serialize_u16(self, v: u16) -> Result<Value, VmError> {
        Ok(int_value(v as i64))
    }
    fn serialize_u32(self, v: u32) -> Result<Value, VmError> {
        Ok(int_value(v as i64))
    }
    fn serialize_u64(self, v: u64) -> Result<Value, VmError> {
        // i64-range stays Integer; larger falls back to lossy f64
        // exactly as `value_from_json` does for `u64 > i64::MAX`.
        match i64::try_from(v) {
            Ok(i) => Ok(int_value(i)),
            Err(_) => Ok(float_value(v as f64)),
        }
    }
    fn serialize_u128(self, _v: u128) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("u128 is not supported"))
    }

    fn serialize_f32(self, v: f32) -> Result<Value, VmError> {
        Ok(float_value(v as f64))
    }
    fn serialize_f64(self, v: f64) -> Result<Value, VmError> {
        Ok(float_value(v))
    }

    fn serialize_char(self, v: char) -> Result<Value, VmError> {
        Ok(Value::string(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<Value, VmError> {
        Ok(Value::string(v))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Value, VmError> {
        // serde_json serializes bytes as a sequence of integers.
        let table = Table::new();
        for (idx, b) in v.iter().enumerate() {
            table.raw_set(Value::Integer((idx + 1) as i64), Value::Integer(*b as i64))?;
        }
        Ok(Value::Table(table))
    }

    fn serialize_none(self) -> Result<Value, VmError> {
        Ok(Value::Nil)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Value, VmError> {
        v.serialize(self)
    }

    fn serialize_unit(self) -> Result<Value, VmError> {
        Ok(Value::Nil)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, VmError> {
        Ok(Value::Nil)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Value, VmError> {
        Ok(Value::string(variant))
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        v: &T,
    ) -> Result<Value, VmError> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<Value, VmError> {
        let table = Table::new();
        table.raw_set(Value::string(variant), v.serialize(Serializer)?)?;
        Ok(Value::Table(table))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<SeqSerializer, VmError> {
        Ok(SeqSerializer {
            table: Table::new(),
            next: 1,
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSerializer, VmError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SeqSerializer, VmError> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<TupleVariantSerializer, VmError> {
        Ok(TupleVariantSerializer {
            variant,
            inner: Table::new(),
            next: 1,
        })
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<MapSerializer, VmError> {
        Ok(MapSerializer {
            table: Table::new(),
            pending_key: None,
        })
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<StructSerializer, VmError> {
        Ok(StructSerializer {
            table: Table::new(),
        })
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<StructVariantSerializer, VmError> {
        Ok(StructVariantSerializer {
            variant,
            inner: Table::new(),
        })
    }
}

struct SeqSerializer {
    table: Table,
    next: i64,
}

impl ser::SerializeSeq for SeqSerializer {
    type Ok = Value;
    type Error = VmError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), VmError> {
        self.table
            .raw_set(Value::Integer(self.next), v.serialize(Serializer)?)?;
        self.next += 1;
        Ok(())
    }
    fn end(self) -> Result<Value, VmError> {
        Ok(Value::Table(self.table))
    }
}

impl ser::SerializeTuple for SeqSerializer {
    type Ok = Value;
    type Error = VmError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), VmError> {
        ser::SerializeSeq::serialize_element(self, v)
    }
    fn end(self) -> Result<Value, VmError> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for SeqSerializer {
    type Ok = Value;
    type Error = VmError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), VmError> {
        ser::SerializeSeq::serialize_element(self, v)
    }
    fn end(self) -> Result<Value, VmError> {
        ser::SerializeSeq::end(self)
    }
}

struct TupleVariantSerializer {
    variant: &'static str,
    inner: Table,
    next: i64,
}

impl ser::SerializeTupleVariant for TupleVariantSerializer {
    type Ok = Value;
    type Error = VmError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), VmError> {
        self.inner
            .raw_set(Value::Integer(self.next), v.serialize(Serializer)?)?;
        self.next += 1;
        Ok(())
    }
    fn end(self) -> Result<Value, VmError> {
        let table = Table::new();
        table.raw_set(Value::string(self.variant), Value::Table(self.inner))?;
        Ok(Value::Table(table))
    }
}

struct MapSerializer {
    table: Table,
    pending_key: Option<Value>,
}

impl ser::SerializeMap for MapSerializer {
    type Ok = Value;
    type Error = VmError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), VmError> {
        self.pending_key = Some(key.serialize(MapKeySerializer)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), VmError> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| <VmError as ser::Error>::custom("serialize_value called before serialize_key"))?;
        self.table.raw_set(key, v.serialize(Serializer)?)?;
        Ok(())
    }
    fn end(self) -> Result<Value, VmError> {
        Ok(Value::Table(self.table))
    }
}

impl ser::SerializeStruct for StructSerializer {
    type Ok = Value;
    type Error = VmError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        v: &T,
    ) -> Result<(), VmError> {
        self.table
            .raw_set(Value::string(name), v.serialize(Serializer)?)?;
        Ok(())
    }
    fn skip_field(&mut self, _name: &'static str) -> Result<(), VmError> {
        Ok(())
    }
    fn end(self) -> Result<Value, VmError> {
        Ok(Value::Table(self.table))
    }
}

struct StructSerializer {
    table: Table,
}

struct StructVariantSerializer {
    variant: &'static str,
    inner: Table,
}

impl ser::SerializeStructVariant for StructVariantSerializer {
    type Ok = Value;
    type Error = VmError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        name: &'static str,
        v: &T,
    ) -> Result<(), VmError> {
        self.inner
            .raw_set(Value::string(name), v.serialize(Serializer)?)?;
        Ok(())
    }
    fn skip_field(&mut self, _name: &'static str) -> Result<(), VmError> {
        Ok(())
    }
    fn end(self) -> Result<Value, VmError> {
        let table = Table::new();
        table.raw_set(Value::string(self.variant), Value::Table(self.inner))?;
        Ok(Value::Table(table))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serde_bridge::{value_from_json, value_to_json};
    use serde::Serialize;
    use std::collections::BTreeMap;

    #[derive(Serialize)]
    struct Inner {
        a: i64,
        b: bool,
    }

    #[derive(Serialize)]
    enum Tag {
        Unit,
        New(i64),
        Tup(i64, String),
        Strukt { x: f64 },
    }

    #[derive(Serialize)]
    struct Complex {
        name: String,
        #[serde(rename = "renamed")]
        ren: i64,
        opt_some: Option<i64>,
        opt_none: Option<i64>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        maybe: Vec<i64>,
        seq: Vec<Inner>,
        map: BTreeMap<String, i64>,
        tag_unit: Tag,
        tag_new: Tag,
        tag_tup: Tag,
        tag_strukt: Tag,
        f: f64,
        nonfinite: f64,
        big: u64,
        #[serde(flatten)]
        flat: Inner,
    }

    /// `to_value` must be byte-identical to the old
    /// `value_from_json(serde_json::to_value(t))` path.  Compare via
    /// `value_to_json` (Value's `PartialEq` is identity-based for
    /// tables, so round-trip back to json for a structural check).
    #[test]
    fn parity_with_serde_json_bridge() {
        let mut map = BTreeMap::new();
        map.insert("k1".to_owned(), 1);
        map.insert("k2".to_owned(), 2);
        let v = Complex {
            name: "n".to_owned(),
            ren: 7,
            opt_some: Some(9),
            opt_none: None,
            maybe: vec![],
            seq: vec![Inner { a: 1, b: true }, Inner { a: 2, b: false }],
            map,
            tag_unit: Tag::Unit,
            tag_new: Tag::New(3),
            tag_tup: Tag::Tup(4, "s".to_owned()),
            tag_strukt: Tag::Strukt { x: 1.5 },
            f: 2.0,
            nonfinite: f64::NAN,
            big: u64::MAX,
            flat: Inner { a: 42, b: true },
        };

        let direct = to_value(&v).expect("to_value");
        let json_path =
            value_from_json(serde_json::to_value(&v).expect("serde_json")).expect("bridge");

        k9::assert_equal!(
            value_to_json(&direct).expect("vtj direct"),
            value_to_json(&json_path).expect("vtj bridge")
        );
    }
}

/// serde_json only permits string-like map keys; mirror that: emit a
/// `Value::String`, stringifying integer keys (as serde_json does)
/// and rejecting anything else.
struct MapKeySerializer;

fn key_str(s: String) -> Result<Value, VmError> {
    Ok(Value::string(s))
}

impl ser::Serializer for MapKeySerializer {
    type Ok = Value;
    type Error = VmError;
    type SerializeSeq = ser::Impossible<Value, VmError>;
    type SerializeTuple = ser::Impossible<Value, VmError>;
    type SerializeTupleStruct = ser::Impossible<Value, VmError>;
    type SerializeTupleVariant = ser::Impossible<Value, VmError>;
    type SerializeMap = ser::Impossible<Value, VmError>;
    type SerializeStruct = ser::Impossible<Value, VmError>;
    type SerializeStructVariant = ser::Impossible<Value, VmError>;

    fn serialize_str(self, v: &str) -> Result<Value, VmError> {
        Ok(Value::string(v))
    }
    fn serialize_char(self, v: char) -> Result<Value, VmError> {
        Ok(Value::string(v.to_string()))
    }
    fn serialize_bool(self, v: bool) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_i8(self, v: i8) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_i16(self, v: i16) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_i32(self, v: i32) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_i64(self, v: i64) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_i128(self, v: i128) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_u8(self, v: u8) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_u16(self, v: u16) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_u32(self, v: u32) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_u64(self, v: u64) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_u128(self, v: u128) -> Result<Value, VmError> {
        key_str(v.to_string())
    }
    fn serialize_f32(self, _v: f32) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("float map key is not supported"))
    }
    fn serialize_f64(self, _v: f64) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("float map key is not supported"))
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("bytes map key is not supported"))
    }
    fn serialize_none(self) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("None map key is not supported"))
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _v: &T) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("Option map key is not supported"))
    }
    fn serialize_unit(self) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("unit map key is not supported"))
    }
    fn serialize_unit_struct(self, _n: &'static str) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("unit struct map key is not supported"))
    }
    fn serialize_unit_variant(
        self,
        _n: &'static str,
        _i: u32,
        variant: &'static str,
    ) -> Result<Value, VmError> {
        Ok(Value::string(variant))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _n: &'static str,
        v: &T,
    ) -> Result<Value, VmError> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _n: &'static str,
        _i: u32,
        _variant: &'static str,
        _v: &T,
    ) -> Result<Value, VmError> {
        Err(<VmError as ser::Error>::custom("enum map key is not supported"))
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, VmError> {
        Err(<VmError as ser::Error>::custom("sequence map key is not supported"))
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, VmError> {
        Err(<VmError as ser::Error>::custom("tuple map key is not supported"))
    }
    fn serialize_tuple_struct(
        self,
        _n: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, VmError> {
        Err(<VmError as ser::Error>::custom("tuple struct map key is not supported"))
    }
    fn serialize_tuple_variant(
        self,
        _n: &'static str,
        _i: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, VmError> {
        Err(<VmError as ser::Error>::custom("tuple variant map key is not supported"))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, VmError> {
        Err(<VmError as ser::Error>::custom("map map key is not supported"))
    }
    fn serialize_struct(
        self,
        _n: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, VmError> {
        Err(<VmError as ser::Error>::custom("struct map key is not supported"))
    }
    fn serialize_struct_variant(
        self,
        _n: &'static str,
        _i: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, VmError> {
        Err(<VmError as ser::Error>::custom("struct variant map key is not supported"))
    }
}
