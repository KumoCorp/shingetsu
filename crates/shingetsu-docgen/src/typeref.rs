use bstr::ByteSlice;
use serde::{Deserialize, Serialize};
use shingetsu_vm::types::{LuaType, LuaTypeArg, TableField, TableLuaType, TypedParam};
use shingetsu_vm::Bytes;

/// Structured type reference for documentation rendering.
///
/// `TypeRef` mirrors the shape of [`shingetsu_vm::LuaType`] but is
/// `serde`-friendly and contains no runtime-only state.  Each
/// rendering backend (markdown, luau-lsp, lua-language-server)
/// consumes `TypeRef` and produces format-specific output \u2014 Luau,
/// for example, can't express a heterogeneous tuple inside a union,
/// while markdown can render the prose form happily.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeRef {
    Nil,
    Boolean,
    Number,
    Integer,
    Float,
    String,
    Any,
    Unknown,
    Never,
    /// Reference to a named type (userdata, module, type alias).
    /// Renderers should resolve `name` against the surrounding
    /// `DocModel` for cross-page linking.
    Named {
        name: String,
    },
    /// Reference to a generic type parameter, e.g. `T`.
    TypeParam {
        name: String,
    },
    Optional {
        inner: Box<TypeRef>,
    },
    Union {
        arms: Vec<TypeRef>,
    },
    Intersection {
        arms: Vec<TypeRef>,
    },
    /// Heterogeneous tuple `(A, B, C)`.  Emitted as a function
    /// multi-return in Luau; flattened to a union or `any` when used
    /// inside a [`Union`](TypeRef::Union).
    Tuple {
        items: Vec<TypeRef>,
    },
    /// Type pack tail `...T`.
    Variadic {
        inner: Box<TypeRef>,
    },
    StringLiteral {
        value: String,
    },
    BoolLiteral {
        value: bool,
    },
    NumberLiteral {
        value: f64,
    },
    Function {
        params: Vec<TypeRefParam>,
        variadic: Option<Box<TypeRef>>,
        returns: Vec<TypeRef>,
        is_method: bool,
    },
    Table {
        fields: Vec<TypeRefField>,
        indexer: Option<TypeRefIndexer>,
    },
    Generic {
        base: Box<TypeRef>,
        args: Vec<TypeRef>,
    },
    /// Reference to a `#[shingetsu::module]`-defined module by name.
    Module {
        name: String,
    },
}

/// A `(name, type)` pair used inside [`TypeRef::Function`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeRefParam {
    pub name: Option<String>,
    pub ty: TypeRef,
}

/// A named field inside [`TypeRef::Table`].  Carries the rustdoc
/// captured at the field's declaration site (when the type was
/// built via `derive(LuaTable)`) plus the textual rendering of any
/// `#[lua(default = expr)]` annotation.  The markdown renderer
/// surfaces both in the per-parameter documentation when a
/// parameter accepts a table with documented fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeRefField {
    pub name: String,
    pub ty: TypeRef,
    /// rustdoc on the field, joined with `\n`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Textual rendering of `#[lua(default = expr)]`, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// `[K]: V` indexer used inside [`TypeRef::Table`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeRefIndexer {
    pub key: Box<TypeRef>,
    pub value: Box<TypeRef>,
}

impl TypeRef {
    /// Build a [`TypeRef`] from a [`LuaType`].
    pub fn from_lua_type(ty: &LuaType) -> Self {
        match ty {
            LuaType::Nil => TypeRef::Nil,
            LuaType::Boolean => TypeRef::Boolean,
            LuaType::Number => TypeRef::Number,
            LuaType::Integer => TypeRef::Integer,
            LuaType::Float => TypeRef::Float,
            LuaType::String => TypeRef::String,
            LuaType::Any => TypeRef::Any,
            LuaType::Unknown => TypeRef::Unknown,
            LuaType::Never => TypeRef::Never,
            LuaType::Named(n) => TypeRef::Named {
                name: n.to_str_lossy().into_owned(),
            },
            LuaType::TypeParam(n) => TypeRef::TypeParam {
                name: n.to_str_lossy().into_owned(),
            },
            LuaType::Optional(inner) => TypeRef::Optional {
                inner: Box::new(TypeRef::from_lua_type(inner)),
            },
            LuaType::Union(arms) => TypeRef::Union {
                arms: arms.iter().map(TypeRef::from_lua_type).collect(),
            },
            LuaType::Intersection(arms) => TypeRef::Intersection {
                arms: arms.iter().map(TypeRef::from_lua_type).collect(),
            },
            LuaType::Tuple(items) => TypeRef::Tuple {
                items: items.iter().map(TypeRef::from_lua_type).collect(),
            },
            LuaType::Variadic(inner) => TypeRef::Variadic {
                inner: Box::new(TypeRef::from_lua_type(inner)),
            },
            LuaType::StringLiteral(s) => TypeRef::StringLiteral {
                value: s.to_str_lossy().into_owned(),
            },
            LuaType::BoolLiteral(b) => TypeRef::BoolLiteral { value: *b },
            LuaType::NumberLiteral(n) => TypeRef::NumberLiteral { value: *n },
            LuaType::Function(f) => {
                let params = f
                    .params
                    .iter()
                    .map(|p| TypeRefParam {
                        name: p.name.as_ref().map(|n| n.to_str_lossy().into_owned()),
                        ty: TypeRef::from_lua_type(&p.lua_type),
                    })
                    .collect();
                let variadic = f
                    .variadic
                    .as_ref()
                    .map(|v| Box::new(TypeRef::from_lua_type(v)));
                let returns = f.returns.iter().map(TypeRef::from_lua_type).collect();
                TypeRef::Function {
                    params,
                    variadic,
                    returns,
                    is_method: f.is_method,
                }
            }
            LuaType::Table(t) => {
                let fields = t
                    .fields
                    .iter()
                    .map(|f| TypeRefField {
                        name: f.name.to_str_lossy().into_owned(),
                        ty: TypeRef::from_lua_type(&f.lua_type),
                        doc: f.doc.clone(),
                        default: f.default.clone(),
                    })
                    .collect();
                let indexer = t.indexer.as_ref().map(|(k, v)| TypeRefIndexer {
                    key: Box::new(TypeRef::from_lua_type(k)),
                    value: Box::new(TypeRef::from_lua_type(v)),
                });
                TypeRef::Table { fields, indexer }
            }
            LuaType::Generic { base, args } => TypeRef::Generic {
                base: Box::new(TypeRef::from_lua_type(base)),
                args: args
                    .iter()
                    .map(|a| match a {
                        shingetsu_vm::types::LuaTypeArg::Type(t) => TypeRef::from_lua_type(t),
                        shingetsu_vm::types::LuaTypeArg::Pack(t) => TypeRef::Variadic {
                            inner: Box::new(TypeRef::from_lua_type(t)),
                        },
                    })
                    .collect(),
            },
            LuaType::Module(m) => TypeRef::Module {
                name: m.name.to_str_lossy().into_owned(),
            },
        }
    }

    /// Reverse of [`Self::from_lua_type`]: rebuild a [`LuaType`]
    /// from this reference.  Used when a serialized [`crate::DocModel`]
    /// needs to be fed back into the compiler's type checker.
    pub fn to_lua_type(&self) -> LuaType {
        match self {
            TypeRef::Nil => LuaType::Nil,
            TypeRef::Boolean => LuaType::Boolean,
            TypeRef::Number => LuaType::Number,
            TypeRef::Integer => LuaType::Integer,
            TypeRef::Float => LuaType::Float,
            TypeRef::String => LuaType::String,
            TypeRef::Any => LuaType::Any,
            TypeRef::Unknown => LuaType::Unknown,
            TypeRef::Never => LuaType::Never,
            TypeRef::Named { name } => LuaType::named(name.as_str()),
            TypeRef::TypeParam { name } => LuaType::type_param(name.as_str()),
            TypeRef::Optional { inner } => LuaType::Optional(Box::new(inner.to_lua_type())),
            TypeRef::Union { arms } => {
                LuaType::Union(arms.iter().map(TypeRef::to_lua_type).collect())
            }
            TypeRef::Intersection { arms } => {
                LuaType::Intersection(arms.iter().map(TypeRef::to_lua_type).collect())
            }
            TypeRef::Tuple { items } => {
                LuaType::Tuple(items.iter().map(TypeRef::to_lua_type).collect())
            }
            TypeRef::Variadic { inner } => LuaType::Variadic(Box::new(inner.to_lua_type())),
            TypeRef::StringLiteral { value } => LuaType::StringLiteral(Bytes::from(value.as_str())),
            TypeRef::BoolLiteral { value } => LuaType::BoolLiteral(*value),
            TypeRef::NumberLiteral { value } => LuaType::NumberLiteral(*value),
            TypeRef::Function {
                params,
                variadic,
                returns,
                is_method,
            } => {
                let params = params
                    .iter()
                    .map(|p| TypedParam {
                        name: p.name.as_ref().map(|n| Bytes::from(n.as_str())),
                        lua_type: p.ty.to_lua_type(),
                        doc: None,
                    })
                    .collect();
                let variadic = variadic.as_ref().map(|v| Box::new(v.to_lua_type()));
                let returns = returns.iter().map(TypeRef::to_lua_type).collect();
                LuaType::Function(Box::new(shingetsu_vm::types::FunctionLuaType {
                    type_params: vec![],
                    params,
                    variadic,
                    returns,
                    is_method: *is_method,
                    inferred_unannotated: false,
                }))
            }
            TypeRef::Table { fields, indexer } => {
                let fields: Vec<TableField> = fields
                    .iter()
                    .map(|f| TableField {
                        name: Bytes::from(f.name.as_str()),
                        lua_type: f.ty.to_lua_type(),
                        doc: f.doc.clone(),
                        default: f.default.clone(),
                    })
                    .collect();
                let indexer = indexer.as_ref().map(|i| {
                    (
                        Box::new(i.key.to_lua_type()),
                        Box::new(i.value.to_lua_type()),
                    )
                });
                LuaType::Table(Box::new(TableLuaType { fields, indexer }))
            }
            TypeRef::Generic { base, args } => {
                let args = args
                    .iter()
                    .map(|a| match a {
                        TypeRef::Variadic { inner } => LuaTypeArg::Pack(inner.to_lua_type()),
                        other => LuaTypeArg::Type(other.to_lua_type()),
                    })
                    .collect();
                LuaType::Generic {
                    base: Box::new(base.to_lua_type()),
                    args,
                }
            }
            TypeRef::Module { name } => LuaType::named(name.as_str()),
        }
    }

    /// Walk the type expression and collect every named-type
    /// reference, deduplicated, in source order.  Renderers that
    /// emit cross-page links use this to know which substrings are
    /// hyperlinkable.
    pub fn references(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_references(&mut out);
        out
    }

    fn collect_references(&self, out: &mut Vec<String>) {
        match self {
            TypeRef::Named { name } | TypeRef::Module { name } => {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            TypeRef::Optional { inner } | TypeRef::Variadic { inner } => {
                inner.collect_references(out)
            }
            TypeRef::Union { arms } | TypeRef::Intersection { arms } => {
                for a in arms {
                    a.collect_references(out);
                }
            }
            TypeRef::Tuple { items } => {
                for i in items {
                    i.collect_references(out);
                }
            }
            TypeRef::Function {
                params,
                variadic,
                returns,
                ..
            } => {
                for p in params {
                    p.ty.collect_references(out);
                }
                if let Some(v) = variadic {
                    v.collect_references(out);
                }
                for r in returns {
                    r.collect_references(out);
                }
            }
            TypeRef::Table { fields, indexer } => {
                for f in fields {
                    f.ty.collect_references(out);
                }
                if let Some(i) = indexer {
                    i.key.collect_references(out);
                    i.value.collect_references(out);
                }
            }
            TypeRef::Generic { base, args } => {
                base.collect_references(out);
                for a in args {
                    a.collect_references(out);
                }
            }
            _ => {}
        }
    }
}
