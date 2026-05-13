//! Module-level `@deprecated` propagation through field access.
//!
//! When a sub-module is registered as a field on a parent module and
//! the sub-module's `ModuleType.deprecated` is set, accessing the
//! sub-module through the parent (e.g. `kumo.api`) must fire the
//! `deprecated` lint at the access site.  No new lint hook is
//! involved -- the existing field-access deprecation check inherits
//! the message from the sub-module when the field's own `deprecated`
//! is empty.

mod common;

use shingetsu_vm::types::{FieldDef, FieldKind, LuaType, ModuleType};
use shingetsu_vm::{Bytes, GlobalTypeMap};

fn module(name: &str, deprecated: Option<&str>, fields: Vec<FieldDef>) -> LuaType {
    LuaType::Module(Box::new(ModuleType {
        name: Bytes::from(name),
        doc: None,
        strict: false,
        fields,
        functions: vec![],
        methods: vec![],
        metamethods: vec![],
        deprecated: deprecated.map(|s| s.to_string()),
    }))
}

#[tokio::test]
async fn deprecated_submodule_access_through_parent_warns() {
    // `kumo.api` -- the sub-module itself is deprecated.  Its
    // FieldDef on the parent has no `deprecated` of its own, so
    // the lookup inherits the sub-module's message.
    let api_sub = module("kumo.api", Some("use `kumo.newapi` instead"), vec![]);
    let kumo_root = module(
        "kumo",
        None,
        vec![FieldDef {
            name: Bytes::from("api"),
            doc: None,
            lua_type: api_sub,
            kind: FieldKind::ReadWrite,
            examples: vec![],
            deprecated: None,
        }],
    );

    let mut globals = GlobalTypeMap::default();
    globals.types.insert(Bytes::from("kumo"), kumo_root);

    let src = "local _x = kumo.api";
    let rendered = common::compile_diagnostics_with_globals(globals, src).await;
    k9::assert_equal!(
        rendered,
        "warning[deprecated]: access of deprecated field 'api': use `kumo.newapi` instead
 --> test.lua:1:12
  |
1 | local _x = kumo.api
  |            ^^^^^^^^ access of deprecated field 'api'"
    );
}

#[tokio::test]
async fn field_own_deprecation_wins_over_submodule() {
    // When the FieldDef carries its own deprecation message, that
    // message wins -- the sub-module's message is shadowed.  This
    // matches the precedence rule the lookup encodes: an explicit
    // per-field annotation overrides inheritance.
    let api_sub = module("kumo.api", Some("submodule says obsolete"), vec![]);
    let kumo_root = module(
        "kumo",
        None,
        vec![FieldDef {
            name: Bytes::from("api"),
            doc: None,
            lua_type: api_sub,
            kind: FieldKind::ReadWrite,
            examples: vec![],
            deprecated: Some("field says obsolete".to_string()),
        }],
    );

    let mut globals = GlobalTypeMap::default();
    globals.types.insert(Bytes::from("kumo"), kumo_root);

    let src = "local _x = kumo.api";
    let rendered = common::compile_diagnostics_with_globals(globals, src).await;
    k9::assert_equal!(
        rendered,
        "warning[deprecated]: access of deprecated field 'api': field says obsolete
 --> test.lua:1:12
  |
1 | local _x = kumo.api
  |            ^^^^^^^^ access of deprecated field 'api'"
    );
}
