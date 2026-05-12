//! Reverse direction of `extract`: build the compiler-facing
//! [`GlobalTypeMap`] from a serialized [`DocModel`].
//!
//! Used by `shingetsu check --types <file.json>` so embedder type
//! data can drive offline type-checking without an in-process
//! [`GlobalEnv`].

use shingetsu_vm::types::{
    EventHandlerSignature, FieldDef, FieldKind, FunctionDef, FunctionLuaType, FunctionSignature,
    LuaType, ModuleType, ParamSpec, TypedParam,
};
use shingetsu_vm::{Bytes, GlobalTypeMap};

use crate::{DocModel, EventDoc, FieldDoc, FieldDocKind, FunctionDoc, ModuleDoc, ParamDoc};

impl DocModel {
    /// Produce a [`GlobalTypeMap`] sufficient for the compiler's
    /// type checker to resolve calls into the modules, events, and
    /// globals described by this model.
    ///
    /// Userdata types are emitted as named references in returned
    /// `LuaType`s but their method tables are not currently exposed
    /// to the type checker (the checker has no userdata registry
    /// today; this is tracked for a follow-up phase).
    pub fn to_global_type_map(&self) -> GlobalTypeMap {
        let mut map = GlobalTypeMap::new();
        for m in &self.modules {
            let module = module_doc_to_module_type(m);
            map.types.insert(
                Bytes::from(m.name.as_str()),
                LuaType::Module(Box::new(module)),
            );
        }
        for g in &self.globals {
            map.types
                .insert(Bytes::from(g.name.as_str()), field_doc_to_lua_type(g));
        }
        for e in &self.events {
            let sig = event_doc_to_handler_signature(e);
            map.event_handler_signatures
                .insert(Bytes::from(e.name.as_str()), sig);
        }
        map
    }
}

fn module_doc_to_module_type(m: &ModuleDoc) -> ModuleType {
    let fields = m.fields.iter().map(field_doc_to_field_def).collect();
    let functions = m
        .functions
        .iter()
        .map(|f| function_doc_to_function_def(&m.name, f))
        .collect();
    ModuleType {
        name: Bytes::from(m.name.as_str()),
        doc: m.doc.clone(),
        strict: m.strict,
        fields,
        functions,
        methods: vec![],
        metamethods: vec![],
    }
}

fn field_doc_to_lua_type(f: &FieldDoc) -> LuaType {
    // `FieldDoc.ty` already includes any `Optional` wrapper when
    // applicable; unlike `ParamDoc`, there is no separate flag.
    f.ty.to_lua_type()
}

fn field_doc_to_field_def(f: &FieldDoc) -> FieldDef {
    FieldDef {
        name: Bytes::from(f.name.as_str()),
        doc: f.doc.clone(),
        lua_type: field_doc_to_lua_type(f),
        kind: match f.kind {
            FieldDocKind::Getter => FieldKind::Getter,
            FieldDocKind::Setter => FieldKind::Setter,
            FieldDocKind::ReadWrite => FieldKind::ReadWrite,
        },
        examples: vec![],
    }
}

fn function_doc_to_function_def(module_name: &str, f: &FunctionDoc) -> FunctionDef {
    let params = f
        .params
        .iter()
        .map(param_doc_to_param_spec)
        .collect::<Vec<_>>();
    let variadic = f.variadic.is_some();
    let variadic_doc = f.variadic_doc.clone();
    let lua_returns: Vec<LuaType> = f.returns.iter().map(|r| r.ty.to_lua_type()).collect();

    let signature = FunctionSignature {
        name: Bytes::from(f.name.as_str()),
        source: Bytes::from(module_name),
        type_params: vec![],
        params,
        variadic,
        variadic_doc,
        arg_offset: if f.is_method { 1 } else { 0 },
        returns: None,
        lua_returns: Some(lua_returns),
        line_defined: 0,
        last_line_defined: 0,
        num_upvalues: 0,
        has_runtime_types: false,
    };
    FunctionDef {
        name: Bytes::from(f.name.as_str()),
        doc: f.doc.clone(),
        signature,
        returns_doc: f
            .returns
            .iter()
            .map(|r| r.doc.clone().unwrap_or_default())
            .collect(),
        examples: vec![],
    }
}

fn param_doc_to_param_spec(p: &ParamDoc) -> ParamSpec {
    let inner = p.ty.to_lua_type();
    let lua_type = if p.optional {
        LuaType::Optional(Box::new(inner))
    } else {
        inner
    };
    ParamSpec {
        name: p.name.as_ref().map(|n| Bytes::from(n.as_str())),
        runtime_type: None,
        lua_type: Some(lua_type),
        doc: p.doc.clone(),
    }
}

fn event_doc_to_handler_signature(e: &EventDoc) -> EventHandlerSignature {
    let params: Vec<TypedParam> = e
        .params
        .iter()
        .map(|p| {
            let inner = p.ty.to_lua_type();
            let lua_type = if p.optional {
                LuaType::Optional(Box::new(inner))
            } else {
                inner
            };
            TypedParam {
                name: p.name.as_ref().map(|n| Bytes::from(n.as_str())),
                lua_type,
                doc: p.doc.clone(),
            }
        })
        .collect();
    let returns: Vec<LuaType> = e.returns.iter().map(|r| r.to_lua_type()).collect();
    EventHandlerSignature {
        function_type: FunctionLuaType {
            type_params: vec![],
            params,
            variadic: None,
            returns,
            is_method: false,
            inferred_unannotated: false,
        },
        doc: e.doc.clone(),
        return_doc: e.return_doc.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use super::*;
    use crate::extract;
    use shingetsu_vm::GlobalEnv;

    #[test]
    fn empty_model_yields_empty_map() {
        let model = DocModel {
            schema_version: crate::SCHEMA_VERSION,
            modules: vec![],
            userdata_types: vec![],
            globals: vec![],
            events: vec![],
        };
        let map = model.to_global_type_map();
        k9::assert_equal!(map.types, HashMap::new());
        k9::assert_equal!(map.event_registrars, HashSet::new());
        k9::assert_equal!(map.event_handler_signatures, HashMap::new());
    }

    #[test]
    fn round_trip_through_globalenv() {
        // Build a real GlobalEnv with the standard set, extract a
        // DocModel, then feed it back into a GlobalTypeMap.  The two
        // representations differ structurally (the live env stores
        // `math` as a `LuaType::Table` inferred from the runtime
        // table; the rebuilt one stores it as `LuaType::Module`),
        // but the user-facing behaviour through `lookup_known_member`
        // -- the path the type checker actually walks -- must match
        // for a representative function.
        let env = GlobalEnv::new();
        shingetsu::register_libs(&env, shingetsu::Libraries::ALL).expect("register libs");
        let original = env.global_type_map();
        let model = extract(&env);
        let rebuilt = model.to_global_type_map();

        let resolve_floor = |map: &GlobalTypeMap| -> LuaType {
            let math = map.get(b"math").expect("math global");
            match math.lookup_known_member(b"floor") {
                Some(Some(cow)) => cow.into_owned(),
                other => panic!("expected math.floor function, got {other:?}"),
            }
        };
        k9::assert_equal!(resolve_floor(&rebuilt), resolve_floor(&original));
    }
}
