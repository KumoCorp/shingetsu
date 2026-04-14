//! Converts `full_moon` LuaU type annotations into our `LuaType` representation.

use bytes::Bytes;
use full_moon::ast::luau::{TypeInfo, TypeSpecifier};
use shingetsu_vm::types::{GenericTypeParam, LuaType};
use std::collections::HashSet;

/// Set of generic type parameter names currently in scope.
/// When converting type annotations inside a generic function body,
/// names in this set produce `LuaType::TypeParam` instead of
/// `LuaType::Named`.
pub struct TypeContext {
    pub type_params: HashSet<String>,
}

impl TypeContext {
    pub fn empty() -> Self {
        TypeContext {
            type_params: HashSet::new(),
        }
    }

    pub fn from_generic_params(params: &[GenericTypeParam]) -> Self {
        TypeContext {
            type_params: params
                .iter()
                .map(|p| String::from_utf8_lossy(&p.name).into_owned())
                .collect(),
        }
    }
}

/// Convert a `full_moon` `GenericDeclaration` (the `<T, U>` on a function or
/// type alias) into our `Vec<GenericTypeParam>` representation.
pub fn convert_generic_declaration(
    decl: &full_moon::ast::luau::GenericDeclaration,
) -> Vec<GenericTypeParam> {
    decl.generics()
        .iter()
        .map(|param| {
            let (name, is_pack) = match param.parameter() {
                full_moon::ast::luau::GenericParameterInfo::Name(tok) => {
                    (Bytes::from(tok_str(tok)), false)
                }
                full_moon::ast::luau::GenericParameterInfo::Variadic { name, .. } => {
                    (Bytes::from(tok_str(name)), true)
                }
                _ => (Bytes::from_static(b"_"), false),
            };
            let default = param
                .default_type()
                .map(|ti| convert_type_info_ctx(ti, &TypeContext::empty()));
            GenericTypeParam {
                name,
                constraint: None,
                default,
                is_pack,
            }
        })
        .collect()
}

/// Convert a `full_moon` `TypeSpecifier` (`: type`) into our `LuaType`.
/// Uses an empty generic context (no type params in scope).
#[allow(dead_code)]
pub fn convert_type_specifier(ts: &TypeSpecifier) -> LuaType {
    convert_type_info_ctx(ts.type_info(), &TypeContext::empty())
}

/// Convert a `full_moon` `TypeSpecifier` with a generic context.
pub fn convert_type_specifier_ctx(ts: &TypeSpecifier, ctx: &TypeContext) -> LuaType {
    convert_type_info_ctx(ts.type_info(), ctx)
}

/// Convert a `full_moon` `TypeInfo` AST node into our `LuaType` representation.
/// Uses an empty generic context (no type params in scope).
#[allow(dead_code)]
pub fn convert_type_info(ti: &TypeInfo) -> LuaType {
    convert_type_info_ctx(ti, &TypeContext::empty())
}

/// Convert a `full_moon` `TypeInfo` AST node with a generic context.
pub fn convert_type_info_ctx(ti: &TypeInfo, ctx: &TypeContext) -> LuaType {
    match ti {
        TypeInfo::Basic(tok) => convert_basic_name_ctx(&tok_str(tok), ctx),

        TypeInfo::String(tok) => LuaType::StringLiteral(Bytes::from(trimmed_string_value(tok))),

        TypeInfo::Boolean(tok) => match tok_str(tok).as_str() {
            "true" => LuaType::BoolLiteral(true),
            "false" => LuaType::BoolLiteral(false),
            _ => LuaType::Boolean,
        },

        TypeInfo::Optional { base, .. } => {
            LuaType::Optional(Box::new(convert_type_info_ctx(base, ctx)))
        }

        TypeInfo::Union(union) => {
            let types: Vec<LuaType> = union
                .types()
                .iter()
                .map(|t| convert_type_info_ctx(t, ctx))
                .collect();
            if types.len() == 1 {
                types.into_iter().next().expect("non-empty")
            } else {
                LuaType::Union(types)
            }
        }

        TypeInfo::Intersection(isect) => {
            let types: Vec<LuaType> = isect
                .types()
                .iter()
                .map(|t| convert_type_info_ctx(t, ctx))
                .collect();
            if types.len() == 1 {
                types.into_iter().next().expect("non-empty")
            } else {
                LuaType::Intersection(types)
            }
        }

        TypeInfo::Callback {
            arguments,
            return_type,
            ..
        } => {
            let params: Vec<(Option<Bytes>, LuaType)> = arguments
                .iter()
                .map(|arg| {
                    let name = arg.name().map(|(tok, _)| Bytes::from(tok_str(tok)));
                    let ty = convert_type_info_ctx(arg.type_info(), ctx);
                    (name, ty)
                })
                .collect();
            let ret = convert_type_info_ctx(return_type, ctx);
            let returns = match ret {
                LuaType::Tuple(types) => types,
                other => vec![other],
            };
            LuaType::Function(Box::new(shingetsu_vm::types::FunctionLuaType {
                type_params: vec![],
                params,
                variadic: None,
                returns,
            }))
        }

        TypeInfo::Array { type_info, .. } => {
            // { T } is sugar for a table with numeric keys.
            LuaType::Generic {
                base: Box::new(LuaType::Named(Bytes::from_static(b"Array"))),
                args: vec![shingetsu_vm::types::LuaTypeArg::Type(
                    convert_type_info_ctx(type_info, ctx),
                )],
            }
        }

        TypeInfo::Table { fields, .. } => {
            let mut named_fields = Vec::new();
            let mut indexer = None;
            for field in fields.iter() {
                match field.key() {
                    full_moon::ast::luau::TypeFieldKey::Name(tok) => {
                        named_fields.push((
                            Bytes::from(tok_str(tok)),
                            convert_type_info_ctx(field.value(), ctx),
                        ));
                    }
                    full_moon::ast::luau::TypeFieldKey::IndexSignature { inner, .. } => {
                        indexer = Some((
                            Box::new(convert_type_info_ctx(inner, ctx)),
                            Box::new(convert_type_info_ctx(field.value(), ctx)),
                        ));
                    }
                    _ => {}
                }
            }
            LuaType::Table(Box::new(shingetsu_vm::types::TableLuaType {
                fields: named_fields,
                indexer,
            }))
        }

        TypeInfo::Generic { base, generics, .. } => {
            let base_lt = convert_basic_name_ctx(&tok_str(base), ctx);
            let args: Vec<shingetsu_vm::types::LuaTypeArg> = generics
                .iter()
                .map(|g| shingetsu_vm::types::LuaTypeArg::Type(convert_type_info_ctx(g, ctx)))
                .collect();
            LuaType::Generic {
                base: Box::new(base_lt),
                args,
            }
        }

        TypeInfo::GenericPack { name, .. } => {
            LuaType::Variadic(Box::new(LuaType::TypeParam(Bytes::from(tok_str(name)))))
        }

        TypeInfo::Typeof { .. } => {
            // typeof(expr) is opaque at compile time — treat as Any.
            LuaType::Any
        }

        TypeInfo::Tuple { types, .. } => {
            let inner: Vec<LuaType> = types
                .iter()
                .map(|t| convert_type_info_ctx(t, ctx))
                .collect();
            if inner.len() == 1 {
                // Parenthesized type: (T) == T
                inner.into_iter().next().expect("non-empty")
            } else {
                LuaType::Tuple(inner)
            }
        }

        TypeInfo::Variadic { type_info, .. } => {
            LuaType::Variadic(Box::new(convert_type_info_ctx(type_info, ctx)))
        }

        TypeInfo::VariadicPack { name, .. } => {
            LuaType::Variadic(Box::new(LuaType::TypeParam(Bytes::from(tok_str(name)))))
        }

        TypeInfo::Module {
            module, type_info, ..
        } => {
            let module_name = tok_str(module);
            let type_name = match type_info.as_ref() {
                full_moon::ast::luau::IndexedTypeInfo::Basic(tok) => tok_str(tok),
                full_moon::ast::luau::IndexedTypeInfo::Generic { base, .. } => tok_str(base),
                _ => return LuaType::Any,
            };
            LuaType::Named(Bytes::from(format!("{}.{}", module_name, type_name)))
        }

        // Fallback for any variant we don't handle.
        _ => LuaType::Any,
    }
}

/// Convert a basic type name token string to a `LuaType`.
#[allow(dead_code)]
fn convert_basic_name(name: &str) -> LuaType {
    convert_basic_name_ctx(name, &TypeContext::empty())
}

/// Convert a basic type name, checking generic type params in scope.
fn convert_basic_name_ctx(name: &str, ctx: &TypeContext) -> LuaType {
    match name {
        "nil" => LuaType::Nil,
        "boolean" => LuaType::Boolean,
        "number" => LuaType::Number,
        "integer" => LuaType::Integer,
        "float" => LuaType::Float,
        "string" => LuaType::String,
        "any" => LuaType::Any,
        "unknown" => LuaType::Unknown,
        "never" => LuaType::Never,
        other => {
            if ctx.type_params.contains(other) {
                LuaType::TypeParam(Bytes::from(other.to_owned()))
            } else {
                LuaType::Named(Bytes::from(other.to_owned()))
            }
        }
    }
}

/// Extract the string content of a token reference (trimmed).
fn tok_str(tok: &full_moon::tokenizer::TokenReference) -> String {
    tok.token().to_string()
}

/// Extract the inner string value from a string literal token,
/// stripping surrounding quotes.
fn trimmed_string_value(tok: &full_moon::tokenizer::TokenReference) -> String {
    let s = tok.token().to_string();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_owned()
    } else {
        s
    }
}

/// Convert a return type annotation to a list of `LuaType`.
/// A tuple `(A, B)` becomes `vec![A, B]`; a single type becomes a one-element vec.
/// Uses an empty generic context (no type params in scope).
#[allow(dead_code)]
pub fn convert_return_type(ts: &TypeSpecifier) -> Vec<LuaType> {
    convert_return_type_ctx(ts, &TypeContext::empty())
}

/// Convert a return type annotation with a generic context.
pub fn convert_return_type_ctx(ts: &TypeSpecifier, ctx: &TypeContext) -> Vec<LuaType> {
    let lt = convert_type_info_ctx(ts.type_info(), ctx);
    match lt {
        LuaType::Tuple(types) => types,
        other => vec![other],
    }
}
