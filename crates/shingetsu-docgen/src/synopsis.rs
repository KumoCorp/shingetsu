use crate::{ParamDoc, ReturnDoc, TypeRef};

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
        if v.display == "any" {
            parts.push("...".to_owned());
        } else {
            parts.push(format!("...{}", v.display));
        }
    }

    let mut out = format!("{qualified}({})", parts.join(", "));

    if !returns.is_empty() {
        let returns_str = returns
            .iter()
            .map(|r| r.ty.display.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(" -> ");
        out.push_str(&returns_str);
    }

    out
}
