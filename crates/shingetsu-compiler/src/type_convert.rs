//! Converts `full_moon` LuaU type annotations into our `LuaType` representation.

use bytes::Bytes;
use full_moon::ast::luau::{TypeInfo, TypeSpecifier};
use shingetsu_vm::types::LuaType;

/// Convert a `full_moon` `TypeSpecifier` (`: type`) into our `LuaType`.
pub fn convert_type_specifier(ts: &TypeSpecifier) -> LuaType {
    convert_type_info(ts.type_info())
}

/// Convert a `full_moon` `TypeInfo` AST node into our `LuaType` representation.
pub fn convert_type_info(ti: &TypeInfo) -> LuaType {
    match ti {
        TypeInfo::Basic(tok) => convert_basic_name(&tok_str(tok)),

        TypeInfo::String(tok) => LuaType::StringLiteral(Bytes::from(trimmed_string_value(tok))),

        TypeInfo::Boolean(tok) => match tok_str(tok).as_str() {
            "true" => LuaType::BoolLiteral(true),
            "false" => LuaType::BoolLiteral(false),
            _ => LuaType::Boolean,
        },

        TypeInfo::Optional { base, .. } => LuaType::Optional(Box::new(convert_type_info(base))),

        TypeInfo::Union(union) => {
            let types: Vec<LuaType> = union.types().iter().map(|t| convert_type_info(t)).collect();
            if types.len() == 1 {
                types.into_iter().next().expect("non-empty")
            } else {
                LuaType::Union(types)
            }
        }

        TypeInfo::Intersection(isect) => {
            let types: Vec<LuaType> = isect.types().iter().map(|t| convert_type_info(t)).collect();
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
                    let ty = convert_type_info(arg.type_info());
                    (name, ty)
                })
                .collect();
            let ret = convert_type_info(return_type);
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
                args: vec![shingetsu_vm::types::LuaTypeArg::Type(convert_type_info(
                    type_info,
                ))],
            }
        }

        TypeInfo::Table { fields, .. } => {
            let mut named_fields = Vec::new();
            let mut indexer = None;
            for field in fields.iter() {
                match field.key() {
                    full_moon::ast::luau::TypeFieldKey::Name(tok) => {
                        named_fields
                            .push((Bytes::from(tok_str(tok)), convert_type_info(field.value())));
                    }
                    full_moon::ast::luau::TypeFieldKey::IndexSignature { inner, .. } => {
                        indexer = Some((
                            Box::new(convert_type_info(inner)),
                            Box::new(convert_type_info(field.value())),
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
            let base_lt = convert_basic_name(&tok_str(base));
            let args: Vec<shingetsu_vm::types::LuaTypeArg> = generics
                .iter()
                .map(|g| shingetsu_vm::types::LuaTypeArg::Type(convert_type_info(g)))
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
            let inner: Vec<LuaType> = types.iter().map(|t| convert_type_info(t)).collect();
            if inner.len() == 1 {
                // Parenthesized type: (T) == T
                inner.into_iter().next().expect("non-empty")
            } else {
                LuaType::Tuple(inner)
            }
        }

        TypeInfo::Variadic { type_info, .. } => {
            LuaType::Variadic(Box::new(convert_type_info(type_info)))
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
fn convert_basic_name(name: &str) -> LuaType {
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
        other => LuaType::Named(Bytes::from(other.to_owned())),
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
pub fn convert_return_type(ts: &TypeSpecifier) -> Vec<LuaType> {
    let lt = convert_type_info(ts.type_info());
    match lt {
        LuaType::Tuple(types) => types,
        other => vec![other],
    }
}
