//! Converts `full_moon` LuaU type annotations into our `LuaType` representation.

use full_moon::ast::luau::{TypeInfo, TypeSpecifier};
use shingetsu_vm::types::{GenericTypeParam, LuaType, TableField, TypeAlias};
use shingetsu_vm::Bytes;
use std::collections::{HashMap, HashSet};

/// Set of generic type parameter names and type aliases currently in scope.
/// When converting type annotations inside a generic function body,
/// names in `type_params` produce `LuaType::TypeParam` instead of
/// `LuaType::Named`.  Names in `type_aliases` are expanded inline.
pub struct TypeContext<'a> {
    pub type_params: HashSet<String>,
    pub type_aliases: &'a HashMap<Bytes, TypeAlias>,
}

static EMPTY_ALIASES: std::sync::LazyLock<HashMap<Bytes, TypeAlias>> =
    std::sync::LazyLock::new(HashMap::new);

impl<'a> TypeContext<'a> {
    pub fn empty() -> TypeContext<'static> {
        TypeContext {
            type_params: HashSet::new(),
            type_aliases: &EMPTY_ALIASES,
        }
    }

    pub fn with_aliases(
        params: &[GenericTypeParam],
        aliases: &'a HashMap<Bytes, TypeAlias>,
    ) -> TypeContext<'a> {
        TypeContext {
            type_params: params
                .iter()
                .map(|p| String::from_utf8_lossy(&p.name).into_owned())
                .collect(),
            type_aliases: aliases,
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
                _ => (Bytes::from("_"), false),
            };
            let default = param
                .default_type()
                .map(|ti| convert_type_info_ctx(ti, &TypeContext::empty()));
            GenericTypeParam {
                name,
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
            let params: Vec<shingetsu_vm::types::TypedParam> = arguments
                .iter()
                .map(|arg| {
                    let name = arg.name().map(|(tok, _)| Bytes::from(tok_str(tok)));
                    let lua_type = convert_type_info_ctx(arg.type_info(), ctx);
                    shingetsu_vm::types::TypedParam::new(name, lua_type)
                })
                .collect();
            let ret = convert_type_info_ctx(return_type, ctx);
            let returns = match ret {
                LuaType::Tuple(types) => types,
                other => vec![other],
            };
            // A function with `self` as its first parameter name is a method.
            let is_method = params
                .first()
                .and_then(|p| p.name.as_ref())
                .map_or(false, |n| n == "self");
            LuaType::Function(Box::new(shingetsu_vm::types::FunctionLuaType {
                type_params: vec![],
                params,
                variadic: None,
                returns,
                is_method,
                inferred_unannotated: false,
                deprecated: None,
                must_use: None,
            }))
        }

        TypeInfo::Array { type_info, .. } => {
            // { T } is sugar for a table with numeric keys.
            LuaType::Generic {
                base: Box::new(LuaType::named("Array")),
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
                        named_fields.push(TableField::new(
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
            let base_name = tok_str(base);
            let args: Vec<LuaType> = generics
                .iter()
                .map(|g| convert_type_info_ctx(g, ctx))
                .collect();
            // Check if the base name refers to a generic type alias.
            if let Some(alias) = ctx.type_aliases.get(base_name.as_bytes()) {
                if !alias.params.is_empty() {
                    return substitute_alias(alias, &args);
                }
            }
            let base_lt = convert_basic_name_ctx(&base_name, ctx);
            let type_args: Vec<shingetsu_vm::types::LuaTypeArg> = args
                .into_iter()
                .map(shingetsu_vm::types::LuaTypeArg::Type)
                .collect();
            LuaType::Generic {
                base: Box::new(base_lt),
                args: type_args,
            }
        }

        TypeInfo::GenericPack { name, .. } => {
            LuaType::Variadic(Box::new(LuaType::type_param(tok_str(name))))
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
            LuaType::Variadic(Box::new(LuaType::type_param(tok_str(name))))
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
            LuaType::named(format!("{}.{}", module_name, type_name))
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

/// Convert a basic type name, checking generic type params and aliases in scope.
fn convert_basic_name_ctx(name: &str, ctx: &TypeContext) -> LuaType {
    // Atomic types (nil/boolean/number/…) come straight from
    // [`LuaType::from_basic_name`]; only the unknown-name path needs
    // generic-param/alias resolution.
    match LuaType::from_basic_name(name) {
        LuaType::Named(_) => {
            if ctx.type_params.contains(name) {
                LuaType::type_param(name)
            } else if let Some(alias) = ctx.type_aliases.get(name.as_bytes()) {
                // Non-generic alias reference: expand to the body directly.
                // If the alias has generic params but none are supplied,
                // return it as-is (like a raw reference).
                if alias.params.is_empty() {
                    alias.body.clone()
                } else {
                    LuaType::named(name)
                }
            } else {
                LuaType::named(name)
            }
        }
        atomic => atomic,
    }
}

/// Substitute type arguments into a generic alias body.
/// Given `type Pair<A, B> = { first: A, second: B }` and args `[number, string]`,
/// produces `{ first: number, second: string }`.
fn substitute_alias(alias: &TypeAlias, args: &[LuaType]) -> LuaType {
    // Build a substitution map: param name → concrete type.
    // For params beyond the supplied args, use the param's default
    // or fall back to Any.
    let defaults: Vec<LuaType> = alias
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if let Some(arg) = args.get(i) {
                arg.clone()
            } else if let Some(default) = &p.default {
                default.clone()
            } else {
                LuaType::Any
            }
        })
        .collect();
    let subst: HashMap<&[u8], &LuaType> = alias
        .params
        .iter()
        .zip(defaults.iter())
        .map(|(p, a)| (p.name.as_ref(), a))
        .collect();
    substitute_type(&alias.body, &subst)
}

/// Recursively substitute `TypeParam` references using the given map.
fn substitute_type(ty: &LuaType, subst: &HashMap<&[u8], &LuaType>) -> LuaType {
    match ty {
        LuaType::TypeParam(name) => {
            if let Some(replacement) = subst.get(name.as_ref()) {
                (*replacement).clone()
            } else {
                ty.clone()
            }
        }
        LuaType::Optional(inner) => LuaType::Optional(Box::new(substitute_type(inner, subst))),
        LuaType::Union(types) => {
            LuaType::Union(types.iter().map(|t| substitute_type(t, subst)).collect())
        }
        LuaType::Intersection(types) => {
            LuaType::Intersection(types.iter().map(|t| substitute_type(t, subst)).collect())
        }
        LuaType::Table(table) => {
            let fields = table
                .fields
                .iter()
                .map(|f| TableField {
                    name: f.name.clone(),
                    lua_type: substitute_type(&f.lua_type, subst),
                    doc: f.doc.clone(),
                    default: f.default.clone(),
                    deprecated: f.deprecated.clone(),
                })
                .collect();
            let indexer = table.indexer.as_ref().map(|(k, v)| {
                (
                    Box::new(substitute_type(k, subst)),
                    Box::new(substitute_type(v, subst)),
                )
            });
            LuaType::Table(Box::new(shingetsu_vm::types::TableLuaType {
                fields,
                indexer,
            }))
        }
        LuaType::Function(ft) => {
            let params = ft
                .params
                .iter()
                .map(|p| {
                    shingetsu_vm::types::TypedParam::new_with_doc(
                        p.name.clone(),
                        substitute_type(&p.lua_type, subst),
                        p.doc.clone(),
                    )
                })
                .collect();
            let returns = ft
                .returns
                .iter()
                .map(|t| substitute_type(t, subst))
                .collect();
            let variadic = ft
                .variadic
                .as_ref()
                .map(|v| Box::new(substitute_type(v, subst)));
            LuaType::Function(Box::new(shingetsu_vm::types::FunctionLuaType {
                type_params: ft.type_params.clone(),
                params,
                variadic,
                returns,
                is_method: ft.is_method,
                inferred_unannotated: ft.inferred_unannotated,
                deprecated: None,
                must_use: None,
            }))
        }
        LuaType::Generic { base, args } => {
            let new_base = Box::new(substitute_type(base, subst));
            let new_args = args
                .iter()
                .map(|a| match a {
                    shingetsu_vm::types::LuaTypeArg::Type(t) => {
                        shingetsu_vm::types::LuaTypeArg::Type(substitute_type(t, subst))
                    }
                    shingetsu_vm::types::LuaTypeArg::Pack(t) => {
                        shingetsu_vm::types::LuaTypeArg::Pack(substitute_type(t, subst))
                    }
                })
                .collect();
            LuaType::Generic {
                base: new_base,
                args: new_args,
            }
        }
        LuaType::Tuple(types) => {
            LuaType::Tuple(types.iter().map(|t| substitute_type(t, subst)).collect())
        }
        LuaType::Variadic(inner) => LuaType::Variadic(Box::new(substitute_type(inner, subst))),
        // Leaf types that don't contain type params — return as-is.
        _ => ty.clone(),
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
