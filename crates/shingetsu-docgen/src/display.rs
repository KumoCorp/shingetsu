//! Source-level display of [`TypeRef`] for prose / markdown / synopsis.
//!
//! This rendering uses Luau-style notation (`T?`, `T | U`, `(A, B) -> C`,
//! `{[K]: V}`) and renders heterogeneous tuples as `(A, B, C)` even
//! inside unions \u2014 the consumer is human-readable prose, not a
//! type-checker.  For a Luau-syntactic-correct rendering see the
//! `luau` module.

use std::fmt::Write;

use crate::{TypeRef, TypeRefField, TypeRefParam};

/// Render `ty` in Luau-style notation suitable for prose, markdown,
/// and synopsis lines.
pub fn display(ty: &TypeRef) -> String {
    let mut out = String::new();
    write_display(&mut out, ty);
    out
}

fn write_display(out: &mut String, ty: &TypeRef) {
    match ty {
        TypeRef::Nil => out.push_str("nil"),
        TypeRef::Boolean => out.push_str("boolean"),
        TypeRef::Number => out.push_str("number"),
        TypeRef::Integer => out.push_str("integer"),
        TypeRef::Float => out.push_str("float"),
        TypeRef::String => out.push_str("string"),
        TypeRef::Any => out.push_str("any"),
        TypeRef::Unknown => out.push_str("unknown"),
        TypeRef::Never => out.push_str("never"),
        TypeRef::Named { name } => out.push_str(name),
        TypeRef::TypeParam { name } => out.push_str(name),
        TypeRef::Optional { inner } => {
            write_display(out, inner);
            out.push('?');
        }
        TypeRef::Union { arms } => {
            for (i, a) in arms.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                write_display(out, a);
            }
        }
        TypeRef::Intersection { arms } => {
            for (i, a) in arms.iter().enumerate() {
                if i > 0 {
                    out.push_str(" & ");
                }
                write_display(out, a);
            }
        }
        TypeRef::Tuple { items } => {
            out.push('(');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_display(out, item);
            }
            out.push(')');
        }
        TypeRef::Variadic { inner } => {
            out.push_str("...");
            write_display(out, inner);
        }
        TypeRef::StringLiteral { value } => {
            write!(out, "\"{value}\"").ok();
        }
        TypeRef::BoolLiteral { value } => {
            write!(out, "{value}").ok();
        }
        TypeRef::NumberLiteral { value } => {
            write!(out, "{value}").ok();
        }
        TypeRef::Function {
            params,
            variadic,
            returns,
            ..
        } => {
            write_function(out, params, variadic.as_deref(), returns);
        }
        TypeRef::Table { fields, indexer } => {
            write_table(out, fields, indexer.as_ref());
        }
        TypeRef::Generic { base, args } => {
            write_display(out, base);
            out.push('<');
            for (i, a) in args.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_display(out, a);
            }
            out.push('>');
        }
        TypeRef::Module { name } => {
            write!(out, "module<{name}>").ok();
        }
    }
}

fn write_function(
    out: &mut String,
    params: &[TypeRefParam],
    variadic: Option<&TypeRef>,
    returns: &[TypeRef],
) {
    out.push('(');
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match &p.name {
            Some(n) => {
                write!(out, "{n}: ").ok();
                write_display(out, &p.ty);
            }
            None => write_display(out, &p.ty),
        }
    }
    if let Some(v) = variadic {
        if !params.is_empty() {
            out.push_str(", ");
        }
        out.push_str("...");
        write_display(out, v);
    }
    out.push_str(") -> ");
    match returns.len() {
        0 => out.push_str("()"),
        1 => write_display(out, &returns[0]),
        _ => {
            out.push('(');
            for (i, r) in returns.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_display(out, r);
            }
            out.push(')');
        }
    }
}

fn write_table(out: &mut String, fields: &[TypeRefField], indexer: Option<&crate::TypeRefIndexer>) {
    out.push('{');
    let mut first = true;
    for f in fields {
        if !first {
            out.push_str(", ");
        }
        first = false;
        write!(out, "{}: ", f.name).ok();
        write_display(out, &f.ty);
    }
    if let Some(i) = indexer {
        if !first {
            out.push_str(", ");
        }
        out.push('[');
        write_display(out, &i.key);
        out.push_str("]: ");
        write_display(out, &i.value);
    }
    out.push('}');
}
