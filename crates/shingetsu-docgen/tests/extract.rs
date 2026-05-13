//! End-to-end extraction tests for `shingetsu-docgen`.

use shingetsu::{declare_event, module, userdata};
use shingetsu_docgen::{
    render_luau, DocModel, EventDoc, FieldDoc, FieldDocKind, FunctionDoc, ModuleDoc, ParamDoc,
    ReturnDoc, TypeRef, UserdataDoc, SCHEMA_VERSION,
};

mod common;
use common::extract;
use shingetsu_vm::types::{EventHandlerSignature, FunctionLuaType, LuaType, TypedParam};
use shingetsu_vm::GlobalEnv;

/// A counter exposed as userdata.
struct Counter(#[allow(dead_code)] i64);

/// A counter exposed as userdata.
#[userdata]
impl Counter {
    /// The current count.
    #[lua_field]
    fn value(&self) -> i64 {
        self.0
    }

    /// Add `amount` to the counter and return the new value.
    ///
    /// # Parameters
    ///
    /// - `amount` — the number to add
    ///
    /// # Returns
    ///
    /// - the new value of the counter
    #[lua_method]
    fn increment(&self, amount: i64) -> i64 {
        self.0 + amount
    }
}

/// A small math module.
#[module(name = "smallmath")]
#[allow(dead_code)]
mod smallmath_impl {
    /// Return the larger of two numbers.
    ///
    /// # Parameters
    ///
    /// - `a` — the first value
    /// - `b` — the second value
    ///
    /// # Returns
    ///
    /// - the larger of `a` and `b`
    #[function]
    fn max(a: f64, b: f64) -> f64 {
        if a > b {
            a
        } else {
            b
        }
    }

    /// Format-time version string.
    #[field]
    fn version() -> String {
        "1.0".to_owned()
    }
}

fn ty_named(s: &str) -> TypeRef {
    TypeRef::Named { name: s.to_owned() }
}

fn ty_string() -> TypeRef {
    TypeRef::String
}

fn ty_number() -> TypeRef {
    TypeRef::Number
}

fn build_env() -> GlobalEnv {
    let env = GlobalEnv::new();
    smallmath_impl::register_preload(&env);
    env.register_userdata_type(Counter::userdata_type());
    env
}

fn expected_model() -> DocModel {
    DocModel {
        schema_version: SCHEMA_VERSION,
        modules: vec![ModuleDoc {
            name: "smallmath".into(),
            doc: Some("A small math module.".into()),
            strict: false,
            fields: vec![FieldDoc {
                name: "version".into(),
                doc: Some("Format-time version string.".into()),
                ty: ty_string(),
                kind: FieldDocKind::ReadWrite,
                examples: vec![],
                deprecated: None,
            }],
            functions: vec![FunctionDoc {
                name: "max".into(),
                doc: Some("Return the larger of two numbers.".into()),
                synopsis: "smallmath.max(a, b) -> number".into(),
                params: vec![
                    ParamDoc {
                        name: Some("a".into()),
                        ty: ty_number(),
                        optional: false,
                        doc: Some("the first value".into()),
                    },
                    ParamDoc {
                        name: Some("b".into()),
                        ty: ty_number(),
                        optional: false,
                        doc: Some("the second value".into()),
                    },
                ],
                variadic: None,
                returns: vec![ReturnDoc {
                    ty: ty_number(),
                    doc: Some("the larger of `a` and `b`".into()),
                }],
                is_method: false,
                variadic_doc: None,
                examples: vec![],
                deprecated: None,
                must_use: None,
            }],
            partial: false,
        }],
        userdata_types: vec![UserdataDoc {
            name: "Counter".into(),
            doc: Some("A counter exposed as userdata.".into()),
            fields: vec![FieldDoc {
                name: "value".into(),
                doc: Some("The current count.".into()),
                ty: ty_number(),
                kind: FieldDocKind::Getter,
                examples: vec![],
                deprecated: None,
            }],
            methods: vec![FunctionDoc {
                name: "increment".into(),
                doc: Some("Add `amount` to the counter and return the new value.".into()),
                synopsis: "Counter:increment(amount) -> number".into(),
                examples: vec![],
                params: vec![ParamDoc {
                    name: Some("amount".into()),
                    ty: ty_number(),
                    optional: false,
                    doc: Some("the number to add".into()),
                }],
                variadic: None,
                returns: vec![ReturnDoc {
                    ty: ty_number(),
                    doc: Some("the new value of the counter".into()),
                }],
                is_method: true,
                variadic_doc: None,
                deprecated: None,
                must_use: None,
            }],
            metamethods: vec![],
            partial: false,
        }],
        globals: vec![],
        events: vec![],
    }
}

#[test]
fn extract_produces_expected_doc_model() {
    let env = build_env();
    k9::assert_equal!(extract(&env), expected_model());
}

#[test]
fn typeref_named_type_is_collected() {
    // Sanity: a Named type collects the reference for cross-linking.
    let _ = ty_named("Counter");
    let r = TypeRef::from_lua_type(&shingetsu_vm::LuaType::named("Counter"));
    k9::assert_equal!(
        r,
        TypeRef::Named {
            name: "Counter".into()
        }
    );
    k9::assert_equal!(r.references(), vec!["Counter".to_owned()]);
}

#[test]
fn doc_model_extracts_declared_events() {
    declare_event! {
        /// Fired before a tenant migration begins.
        #[returns = "`false` to abort the migration."]
        pub static BEFORE_MIGRATE: Single(
            "before_migrate",
            /// the tenant identifier
            tenant: String,
        ) -> bool;
    }

    let env = GlobalEnv::new();
    // BEFORE_MIGRATE goes through the production register_compile_type
    // path so the macro->signature->registry flow is exercised
    // end-to-end on at least one event.
    let mut tm = env.global_type_map();
    BEFORE_MIGRATE.register_compile_type(&mut tm);
    for (name, sig) in tm.event_handler_signatures.into_iter() {
        env.declare_event_handler_signature(name, sig);
    }

    // on_reset is registered directly via the env API so the test
    // doesn't depend on a second declare_event! invocation.
    env.declare_event_handler_signature(
        "on_reset",
        EventHandlerSignature {
            function_type: FunctionLuaType {
                type_params: vec![],
                params: vec![
                    TypedParam::new_with_doc(
                        Some("queue"),
                        LuaType::String,
                        Some("the queue being reset".to_owned()),
                    ),
                    TypedParam::new_with_doc(
                        Some("manual"),
                        LuaType::Boolean,
                        Some("whether the reset was triggered manually".to_owned()),
                    ),
                ],
                variadic: None,
                returns: vec![LuaType::Boolean],
                is_method: false,
                inferred_unannotated: false,
                deprecated: None,
                must_use: None,
            },
            doc: Some("Fired when a queue is reset.".to_owned()),
            return_doc: Some("`true` to allow the reset.".to_owned()),
        },
    );

    let model = extract(&env);
    k9::assert_equal!(
        model.events,
        vec![
            EventDoc {
                name: "before_migrate".into(),
                doc: Some(" Fired before a tenant migration begins.\n".into()),
                synopsis: "before_migrate(tenant) -> boolean".into(),
                params: vec![ParamDoc {
                    name: Some("tenant".into()),
                    ty: ty_string(),
                    optional: false,
                    doc: Some(" the tenant identifier\n".into()),
                }],
                returns: vec![TypeRef::Boolean],
                return_doc: Some("`false` to abort the migration.".into()),
            },
            EventDoc {
                name: "on_reset".into(),
                doc: Some("Fired when a queue is reset.".into()),
                synopsis: "on_reset(queue, manual) -> boolean".into(),
                params: vec![
                    ParamDoc {
                        name: Some("queue".into()),
                        ty: ty_string(),
                        optional: false,
                        doc: Some("the queue being reset".into()),
                    },
                    ParamDoc {
                        name: Some("manual".into()),
                        ty: TypeRef::Boolean,
                        optional: false,
                        doc: Some("whether the reset was triggered manually".into()),
                    },
                ],
                returns: vec![TypeRef::Boolean],
                return_doc: Some("`true` to allow the reset.".into()),
            },
        ]
    );
}

#[test]
fn doc_model_round_trips_through_json() {
    let model = expected_model();
    let json = serde_json::to_string_pretty(&model).expect("serialize");
    let parsed: DocModel = serde_json::from_str(&json).expect("deserialize");
    k9::assert_equal!(parsed, model);
}

#[test]
fn doc_model_json_snapshot() {
    let json = serde_json::to_string_pretty(&extract(&build_env())).expect("serialize");
    let expected = include_str!("fixtures/doc_model.json");
    k9::assert_equal!(json, expected.trim_end());
}

#[test]
fn luau_definitions_snapshot() {
    let actual = render_luau(&extract(&build_env()));
    let expected = include_str!("fixtures/definitions.d.luau");
    k9::assert_equal!(actual.trim_end(), expected.trim_end());
}
