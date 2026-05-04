use bstr::ByteSlice;
use serde::{Deserialize, Serialize};
use shingetsu_vm::types::LuaType;

/// A type reference suitable for documentation rendering.
///
/// Carries a pre-rendered display string in Luau syntax (`string?`,
/// `(number, string) -> boolean`, `{[string]: any}`) plus the names of
/// any user-defined types referenced inside it.  Renderers use the
/// `references` list to know which substrings should be turned into
/// hyperlinks to type pages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeRef {
    /// Source-level Luau notation for the type.
    pub display: String,
    /// Names of [`LuaType::Named`] references appearing in this type
    /// expression, deduplicated, in source order.  Renderers should
    /// resolve each entry against the `DocModel`'s userdata-type list
    /// when emitting cross-page links.
    pub references: Vec<String>,
}

impl TypeRef {
    /// Build a [`TypeRef`] from a [`LuaType`].
    pub fn from_lua_type(ty: &LuaType) -> Self {
        let mut references = Vec::new();
        let display = render(ty, &mut references);
        TypeRef {
            display,
            references,
        }
    }

    /// Convenience: the unconstrained `any` type.
    pub fn any() -> Self {
        TypeRef {
            display: "any".to_owned(),
            references: Vec::new(),
        }
    }
}

fn collect_named(name: &[u8], references: &mut Vec<String>) {
    let s = name.to_str_lossy().into_owned();
    if !references.contains(&s) {
        references.push(s);
    }
}

fn render(ty: &LuaType, references: &mut Vec<String>) -> String {
    use std::fmt::Write;
    match ty {
        LuaType::Nil => "nil".to_owned(),
        LuaType::Boolean => "boolean".to_owned(),
        LuaType::Number => "number".to_owned(),
        LuaType::Integer => "integer".to_owned(),
        LuaType::Float => "float".to_owned(),
        LuaType::String => "string".to_owned(),
        LuaType::Any => "any".to_owned(),
        LuaType::Unknown => "unknown".to_owned(),
        LuaType::Never => "never".to_owned(),
        LuaType::Named(n) => {
            collect_named(n, references);
            n.to_str_lossy().into_owned()
        }
        LuaType::TypeParam(n) => n.to_str_lossy().into_owned(),
        LuaType::Optional(inner) => format!("{}?", render(inner, references)),
        LuaType::Union(arms) => arms
            .iter()
            .map(|t| render(t, references))
            .collect::<Vec<_>>()
            .join(" | "),
        LuaType::Intersection(arms) => arms
            .iter()
            .map(|t| render(t, references))
            .collect::<Vec<_>>()
            .join(" & "),
        LuaType::Tuple(items) => {
            let parts: Vec<_> = items.iter().map(|t| render(t, references)).collect();
            format!("({})", parts.join(", "))
        }
        LuaType::Variadic(inner) => format!("...{}", render(inner, references)),
        LuaType::StringLiteral(s) => format!("\"{}\"", s.to_str_lossy()),
        LuaType::BoolLiteral(b) => b.to_string(),
        LuaType::NumberLiteral(n) => n.to_string(),
        LuaType::Function(f) => {
            let params: Vec<String> = f
                .params
                .iter()
                .map(|(name, ty)| match name {
                    Some(n) => {
                        format!("{}: {}", n.to_str_lossy(), render(ty, references))
                    }
                    None => render(ty, references),
                })
                .collect();
            let mut sig = format!("({})", params.join(", "));
            if let Some(v) = &f.variadic {
                if !f.params.is_empty() {
                    sig.pop();
                    sig.push_str(&format!(", ...{})", render(v, references)));
                } else {
                    sig = format!("(...{})", render(v, references));
                }
            }
            let returns = match f.returns.len() {
                0 => "()".to_owned(),
                1 => render(&f.returns[0], references),
                _ => format!(
                    "({})",
                    f.returns
                        .iter()
                        .map(|t| render(t, references))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            format!("{sig} -> {returns}")
        }
        LuaType::Table(t) => {
            let mut out = String::from("{");
            let mut first = true;
            for (name, ty) in &t.fields {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                write!(out, "{}: {}", name.to_str_lossy(), render(ty, references)).ok();
            }
            if let Some((k, v)) = &t.indexer {
                if !first {
                    out.push_str(", ");
                }
                write!(
                    out,
                    "[{}]: {}",
                    render(k, references),
                    render(v, references)
                )
                .ok();
            }
            out.push('}');
            out
        }
        LuaType::Generic { base, args } => {
            let base_str = render(base, references);
            let arg_strs: Vec<String> = args
                .iter()
                .map(|a| match a {
                    shingetsu_vm::types::LuaTypeArg::Type(t) => render(t, references),
                    shingetsu_vm::types::LuaTypeArg::Pack(t) => {
                        format!("{}...", render(t, references))
                    }
                })
                .collect();
            format!("{base_str}<{}>", arg_strs.join(", "))
        }
        LuaType::Module(m) => {
            let s = &m.name.to_str_lossy().into_owned();
            collect_named(&m.name, references);
            format!("module<{s}>")
        }
    }
}
