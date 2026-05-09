use std::fmt::Write;

use crate::display::display;
use crate::{ParamDoc, ReturnDoc, TypeRef, TypeRefField, TypeRefIndexer};

/// Default wrap width for [`render_synopsis_pretty`].
pub const DEFAULT_SYNOPSIS_WIDTH: usize = 80;

/// Render a single-line synopsis for a function or method.
///
/// Examples:
/// - `io.open(filename, mode?) -> file | nil, errmsg`
/// - `math.max(x, ...) -> number`
/// - `file:read(...) -> string`
///
/// Optional parameters render as `name?`.  A trailing variadic renders
/// as `...` (or `...T` when the type is constrained).  Returns are
/// joined with `, `.  When returns are empty the `-> ...` arrow is
/// omitted entirely.
///
/// `parent` is the qualifier shown before the function name.  For
/// methods on userdata it's joined with `:` (e.g. `file:read`); for
/// module functions it's joined with `.` (e.g. `io.open`).  An empty
/// `parent` renders the bare function name.
pub fn render_synopsis(
    parent: &str,
    name: &str,
    params: &[ParamDoc],
    variadic: Option<&TypeRef>,
    returns: &[ReturnDoc],
    is_method: bool,
) -> String {
    let qualified = if parent.is_empty() {
        name.to_owned()
    } else {
        let sep = if is_method { ":" } else { "." };
        format!("{parent}{sep}{name}")
    };

    let mut parts: Vec<String> = params
        .iter()
        .map(|p| {
            let n = p.name.as_deref().unwrap_or("_");
            if p.optional {
                format!("{n}?")
            } else {
                n.to_owned()
            }
        })
        .collect();
    if let Some(v) = variadic {
        if matches!(v, TypeRef::Any) {
            parts.push("...".to_owned());
        } else {
            parts.push(format!("...{}", display(v)));
        }
    }

    let mut out = format!("{qualified}({})", parts.join(", "));

    if !returns.is_empty() {
        let returns_str = returns
            .iter()
            .map(|r| display(&r.ty))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(" -> ");
        out.push_str(&returns_str);
    }

    out
}

/// Like [`render_synopsis`] but breaks the result over multiple lines
/// when it would exceed `max_width` characters.  The single-line form
/// is preferred when it fits; otherwise parameters are placed one per
/// line and the return type, if it is a (possibly optional) table, is
/// expanded with one field per line.
pub fn render_synopsis_pretty(
    parent: &str,
    name: &str,
    params: &[ParamDoc],
    variadic: Option<&TypeRef>,
    returns: &[ReturnDoc],
    is_method: bool,
    max_width: usize,
) -> String {
    let single = render_synopsis(parent, name, params, variadic, returns, is_method);
    if single.len() <= max_width {
        return single;
    }

    let qualified = if parent.is_empty() {
        name.to_owned()
    } else {
        let sep = if is_method { ":" } else { "." };
        format!("{parent}{sep}{name}")
    };

    let mut out = String::new();
    out.push_str(&qualified);
    out.push_str("(\n");
    for p in params {
        let n = p.name.as_deref().unwrap_or("_");
        if p.optional {
            writeln!(out, "    {n}?,").ok();
        } else {
            writeln!(out, "    {n},").ok();
        }
    }
    if let Some(v) = variadic {
        if matches!(v, TypeRef::Any) {
            out.push_str("    ...,\n");
        } else {
            writeln!(out, "    ...{},", display(v)).ok();
        }
    }
    out.push(')');

    if !returns.is_empty() {
        let returns_inline = returns
            .iter()
            .map(|r| display(&r.ty))
            .collect::<Vec<_>>()
            .join(", ");
        let arrow_inline = format!(" -> {returns_inline}");
        // The arrow line continues from the closing `)`; check it on its
        // own (no leading indent) against max_width.
        if arrow_inline.len() <= max_width && returns.len() == 1 {
            // Even if the inline form fits, prefer expanding when the
            // single return is a table-shaped type so structure is visible.
            if is_table_shaped(&returns[0].ty) {
                out.push_str(" -> ");
                out.push_str(&display_pretty(&returns[0].ty, 0, max_width));
            } else {
                out.push_str(&arrow_inline);
            }
        } else if arrow_inline.len() <= max_width {
            out.push_str(&arrow_inline);
        } else if returns.len() == 1 {
            out.push_str(" -> ");
            out.push_str(&display_pretty(&returns[0].ty, 0, max_width));
        } else {
            out.push_str(" -> (\n");
            for r in returns {
                writeln!(out, "    {},", display(&r.ty)).ok();
            }
            out.push(')');
        }
    }

    out
}

/// `true` when `ty` is a table or an optional/union wrapping a table
/// whose multi-line expansion would meaningfully aid readability.
fn is_table_shaped(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Table { .. } => true,
        TypeRef::Optional { inner } => is_table_shaped(inner),
        TypeRef::Union { arms } => arms.iter().any(is_table_shaped),
        _ => false,
    }
}

/// Render `ty` with multi-line expansion of nested tables when the
/// inline form wouldn't fit.  `indent` is the column at which the
/// rendering starts; nested fields are indented one level deeper.
fn display_pretty(ty: &TypeRef, indent: usize, max_width: usize) -> String {
    let inline = display(ty);
    if indent + inline.len() <= max_width {
        return inline;
    }
    match ty {
        TypeRef::Optional { inner } => {
            let inner_pretty = display_pretty(inner, indent, max_width);
            format!("{inner_pretty}?")
        }
        TypeRef::Table { fields, indexer } => {
            render_table_multiline(fields, indexer.as_ref(), indent, max_width)
        }
        TypeRef::Union { arms } => arms
            .iter()
            .map(|arm| {
                if is_table_shaped(arm) {
                    display_pretty(arm, indent, max_width)
                } else {
                    display(arm)
                }
            })
            .collect::<Vec<_>>()
            .join(" | "),
        _ => inline,
    }
}

fn render_table_multiline(
    fields: &[TypeRefField],
    indexer: Option<&TypeRefIndexer>,
    indent: usize,
    max_width: usize,
) -> String {
    let inner_indent = indent + 4;
    let pad = " ".repeat(inner_indent);
    let mut s = String::from("{\n");
    for f in fields {
        let value = display_pretty(&f.ty, inner_indent + f.name.len() + 2, max_width);
        writeln!(s, "{pad}{}: {value},", f.name).ok();
    }
    if let Some(idx) = indexer {
        let key = display(&idx.key);
        let value = display_pretty(&idx.value, inner_indent + key.len() + 4, max_width);
        writeln!(s, "{pad}[{key}]: {value},").ok();
    }
    s.push_str(&" ".repeat(indent));
    s.push('}');
    s
}
