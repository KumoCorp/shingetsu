//! [`DocModel::merge`]: combine multiple doc models into one.
//!
//! Used by `shingetsu check --types a.json --types b.json` and by
//! `shingetsu doc render-markdown --input a.json --input b.json` so
//! embedders can ship type/doc data in pieces (Rust-extracted +
//! Lua-extracted, core + plugins) and have the consumer see a single
//! merged surface.

use crate::{DocModel, EventDoc, FieldDoc, ModuleDoc, UserdataDoc, SCHEMA_VERSION};

/// Errors returned by [`DocModel::merge`].
#[derive(Debug, Clone, PartialEq)]
pub enum MergeError {
    /// An input declared a `schema_version` that does not match
    /// the [`SCHEMA_VERSION`] this build emits.  Mixing schemas
    /// would risk silent shape drift, so the merge refuses.
    SchemaVersion { found: u32, expected: u32 },

    /// Two inputs declared the same module name, and neither was
    /// marked `partial`.  Use `partial = true` on the additive side
    /// to opt into a field/function merge.
    DuplicateModule { name: String },

    /// Two inputs declared the same userdata type name without
    /// `partial = true`.
    DuplicateUserdata { name: String },

    /// Two inputs declared the same global field name.  Globals do
    /// not have a `partial` concept.
    DuplicateGlobal { name: String },

    /// Two inputs declared the same event name with different
    /// signatures.  Identical re-declarations are accepted.
    DuplicateEvent { name: String },

    /// When merging into an existing module / userdata, two entries
    /// contributed the same field, function, method, or metamethod
    /// name.  Even with `partial = true`, contributions must not
    /// overlap.
    DuplicateMember {
        parent: String,
        kind: &'static str,
        name: String,
    },
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::SchemaVersion { found, expected } => write!(
                f,
                "schema version mismatch: input declares {found}, build expects {expected}"
            ),
            MergeError::DuplicateModule { name } => write!(
                f,
                "duplicate module '{name}': set `partial = true` on the additive side to merge"
            ),
            MergeError::DuplicateUserdata { name } => write!(
                f,
                "duplicate userdata type '{name}': set `partial = true` on the additive side to merge"
            ),
            MergeError::DuplicateGlobal { name } => write!(f, "duplicate global '{name}'"),
            MergeError::DuplicateEvent { name } => {
                write!(f, "conflicting declarations of event '{name}'")
            }
            MergeError::DuplicateMember { parent, kind, name } => {
                write!(f, "merging '{parent}': duplicate {kind} '{name}'")
            }
        }
    }
}

impl std::error::Error for MergeError {}

impl DocModel {
    /// Combine `self` with each entry in `others`, in order.
    ///
    /// Concatenates the four top-level collections (`modules`,
    /// `userdata_types`, `globals`, `events`).  Name collisions are
    /// errors unless one side is `partial = true`, in which case its
    /// fields / functions / methods / metamethods merge into the
    /// non-partial entry.  Within a successful merge, contributing
    /// members must not overlap.
    ///
    /// Result `modules` and `userdata_types` are sorted by name for
    /// stable output, matching what [`crate::extract`] produces.
    pub fn merge(mut self, others: Vec<DocModel>) -> Result<DocModel, MergeError> {
        for other in others {
            self = merge_two(self, other)?;
        }
        self.modules.sort_by(|a, b| a.name.cmp(&b.name));
        self.userdata_types.sort_by(|a, b| a.name.cmp(&b.name));
        self.events.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(self)
    }
}

fn merge_two(mut a: DocModel, b: DocModel) -> Result<DocModel, MergeError> {
    if a.schema_version != SCHEMA_VERSION {
        return Err(MergeError::SchemaVersion {
            found: a.schema_version,
            expected: SCHEMA_VERSION,
        });
    }
    if b.schema_version != SCHEMA_VERSION {
        return Err(MergeError::SchemaVersion {
            found: b.schema_version,
            expected: SCHEMA_VERSION,
        });
    }

    for m in b.modules {
        merge_module(&mut a.modules, m)?;
    }
    for ud in b.userdata_types {
        merge_userdata(&mut a.userdata_types, ud)?;
    }
    for g in b.globals {
        merge_global(&mut a.globals, g)?;
    }
    for e in b.events {
        merge_event(&mut a.events, e)?;
    }
    Ok(a)
}

fn merge_module(modules: &mut Vec<ModuleDoc>, incoming: ModuleDoc) -> Result<(), MergeError> {
    let existing_idx = modules.iter().position(|m| m.name == incoming.name);
    match existing_idx {
        None => {
            modules.push(incoming);
            Ok(())
        }
        Some(idx) => {
            let existing = &mut modules[idx];
            if !existing.partial && !incoming.partial {
                return Err(MergeError::DuplicateModule {
                    name: incoming.name,
                });
            }
            // The non-partial side wins for module-level metadata
            // (doc, strict).  If both are partial we keep whatever
            // was already in `existing`.
            let (base, addition) = if existing.partial && !incoming.partial {
                // Promote the incoming non-partial to the slot, then
                // fold the existing partial's members into it.
                let old = std::mem::replace(existing, incoming);
                (existing, old)
            } else {
                (existing, incoming)
            };
            for f in addition.fields {
                if base.fields.iter().any(|x| x.name == f.name) {
                    return Err(MergeError::DuplicateMember {
                        parent: base.name.clone(),
                        kind: "field",
                        name: f.name,
                    });
                }
                base.fields.push(f);
            }
            for f in addition.functions {
                if base.functions.iter().any(|x| x.name == f.name) {
                    return Err(MergeError::DuplicateMember {
                        parent: base.name.clone(),
                        kind: "function",
                        name: f.name,
                    });
                }
                base.functions.push(f);
            }
            // The merged result is no longer partial; it represents
            // a complete merged module surface.
            base.partial = false;
            Ok(())
        }
    }
}

fn merge_userdata(
    userdata_types: &mut Vec<UserdataDoc>,
    incoming: UserdataDoc,
) -> Result<(), MergeError> {
    let existing_idx = userdata_types.iter().position(|u| u.name == incoming.name);
    match existing_idx {
        None => {
            userdata_types.push(incoming);
            Ok(())
        }
        Some(idx) => {
            let existing = &mut userdata_types[idx];
            if !existing.partial && !incoming.partial {
                return Err(MergeError::DuplicateUserdata {
                    name: incoming.name,
                });
            }
            let (base, addition) = if existing.partial && !incoming.partial {
                let old = std::mem::replace(existing, incoming);
                (existing, old)
            } else {
                (existing, incoming)
            };
            for f in addition.fields {
                if base.fields.iter().any(|x| x.name == f.name) {
                    return Err(MergeError::DuplicateMember {
                        parent: base.name.clone(),
                        kind: "field",
                        name: f.name,
                    });
                }
                base.fields.push(f);
            }
            for m in addition.methods {
                if base.methods.iter().any(|x| x.name == m.name) {
                    return Err(MergeError::DuplicateMember {
                        parent: base.name.clone(),
                        kind: "method",
                        name: m.name,
                    });
                }
                base.methods.push(m);
            }
            for mm in addition.metamethods {
                if base.metamethods.iter().any(|x| x.method == mm.method) {
                    return Err(MergeError::DuplicateMember {
                        parent: base.name.clone(),
                        kind: "metamethod",
                        name: mm.method,
                    });
                }
                base.metamethods.push(mm);
            }
            base.partial = false;
            Ok(())
        }
    }
}

fn merge_global(globals: &mut Vec<FieldDoc>, incoming: FieldDoc) -> Result<(), MergeError> {
    if globals.iter().any(|g| g.name == incoming.name) {
        return Err(MergeError::DuplicateGlobal {
            name: incoming.name,
        });
    }
    globals.push(incoming);
    Ok(())
}

fn merge_event(events: &mut Vec<EventDoc>, incoming: EventDoc) -> Result<(), MergeError> {
    if let Some(existing) = events.iter().find(|e| e.name == incoming.name) {
        if existing != &incoming {
            return Err(MergeError::DuplicateEvent {
                name: incoming.name,
            });
        }
        // Identical re-declaration: no-op.
        return Ok(());
    }
    events.push(incoming);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldDocKind, FunctionDoc, ParamDoc, TypeRef, SCHEMA_VERSION};

    fn empty_model() -> DocModel {
        DocModel {
            schema_version: SCHEMA_VERSION,
            modules: vec![],
            userdata_types: vec![],
            globals: vec![],
            events: vec![],
        }
    }

    fn module(name: &str, partial: bool, functions: Vec<FunctionDoc>) -> ModuleDoc {
        ModuleDoc {
            name: name.to_string(),
            doc: None,
            strict: true,
            fields: vec![],
            functions,
            partial,
        }
    }

    fn func(name: &str) -> FunctionDoc {
        FunctionDoc {
            name: name.to_string(),
            doc: None,
            synopsis: format!("kumo.{name}() -> nil"),
            params: vec![],
            variadic: None,
            variadic_doc: None,
            returns: vec![],
            is_method: false,
            examples: vec![],
            deprecated: None,
            must_use: None,
        }
    }

    #[test]
    fn merge_disjoint_models_concatenates() {
        let a = DocModel {
            modules: vec![module("kumo", false, vec![func("a")])],
            ..empty_model()
        };
        let b = DocModel {
            modules: vec![module("dns", false, vec![func("b")])],
            ..empty_model()
        };
        let merged = a.merge(vec![b]).expect("merge");
        let names: Vec<&str> = merged.modules.iter().map(|m| m.name.as_str()).collect();
        k9::assert_equal!(names, vec!["dns", "kumo"]);
    }

    #[test]
    fn duplicate_module_without_partial_errors() {
        let a = DocModel {
            modules: vec![module("kumo", false, vec![func("a")])],
            ..empty_model()
        };
        let b = DocModel {
            modules: vec![module("kumo", false, vec![func("b")])],
            ..empty_model()
        };
        let err = a.merge(vec![b]).unwrap_err();
        k9::assert_equal!(
            err,
            MergeError::DuplicateModule {
                name: "kumo".to_string()
            }
        );
    }

    #[test]
    fn partial_module_merges_functions() {
        let a = DocModel {
            modules: vec![module("kumo", false, vec![func("a")])],
            ..empty_model()
        };
        let b = DocModel {
            modules: vec![module("kumo", true, vec![func("b")])],
            ..empty_model()
        };
        let merged = a.merge(vec![b]).expect("merge");
        k9::assert_equal!(merged.modules.len(), 1);
        let fn_names: Vec<&str> = merged.modules[0]
            .functions
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        k9::assert_equal!(fn_names, vec!["a", "b"]);
        k9::assert_equal!(merged.modules[0].partial, false);
    }

    #[test]
    fn partial_first_merges_into_full_second() {
        // The partial appears in the *first* DocModel; the
        // full (non-partial) declaration comes from the second.
        // After merging, the full one should win on metadata and
        // the partial's members should be folded in.
        let a = DocModel {
            modules: vec![module("kumo", true, vec![func("a")])],
            ..empty_model()
        };
        let b = DocModel {
            modules: vec![module("kumo", false, vec![func("b")])],
            ..empty_model()
        };
        let merged = a.merge(vec![b]).expect("merge");
        let fn_names: Vec<&str> = merged.modules[0]
            .functions
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        // The non-partial side's members come first because the
        // partial folds *into* it.
        k9::assert_equal!(fn_names, vec!["b", "a"]);
    }

    #[test]
    fn partial_member_collision_errors() {
        let a = DocModel {
            modules: vec![module("kumo", false, vec![func("a")])],
            ..empty_model()
        };
        let b = DocModel {
            modules: vec![module("kumo", true, vec![func("a")])],
            ..empty_model()
        };
        let err = a.merge(vec![b]).unwrap_err();
        k9::assert_equal!(
            err,
            MergeError::DuplicateMember {
                parent: "kumo".to_string(),
                kind: "function",
                name: "a".to_string(),
            }
        );
    }

    #[test]
    fn schema_version_mismatch_errors() {
        let a = DocModel {
            schema_version: SCHEMA_VERSION - 1,
            ..empty_model()
        };
        let err = a.merge(vec![empty_model()]).unwrap_err();
        k9::assert_equal!(
            err,
            MergeError::SchemaVersion {
                found: SCHEMA_VERSION - 1,
                expected: SCHEMA_VERSION
            }
        );
    }

    #[test]
    fn duplicate_global_errors() {
        let g = FieldDoc {
            name: "VERSION".to_string(),
            doc: None,
            ty: TypeRef::String,
            kind: FieldDocKind::ReadWrite,
            examples: vec![],
            deprecated: None,
        };
        let a = DocModel {
            globals: vec![g.clone()],
            ..empty_model()
        };
        let b = DocModel {
            globals: vec![g],
            ..empty_model()
        };
        let err = a.merge(vec![b]).unwrap_err();
        k9::assert_equal!(
            err,
            MergeError::DuplicateGlobal {
                name: "VERSION".to_string()
            }
        );
    }

    #[test]
    fn identical_event_redeclaration_is_noop() {
        let e = EventDoc {
            name: "on_thing".to_string(),
            doc: None,
            synopsis: "on_thing() -> nil".to_string(),
            params: vec![],
            returns: vec![],
            return_doc: None,
        };
        let a = DocModel {
            events: vec![e.clone()],
            ..empty_model()
        };
        let b = DocModel {
            events: vec![e],
            ..empty_model()
        };
        let merged = a.merge(vec![b]).expect("merge");
        k9::assert_equal!(merged.events.len(), 1);
    }

    #[test]
    fn conflicting_event_errors() {
        let mk = |params: Vec<ParamDoc>| EventDoc {
            name: "on_thing".to_string(),
            doc: None,
            synopsis: "on_thing".to_string(),
            params,
            returns: vec![],
            return_doc: None,
        };
        let a = DocModel {
            events: vec![mk(vec![])],
            ..empty_model()
        };
        let b = DocModel {
            events: vec![mk(vec![ParamDoc {
                name: Some("x".to_string()),
                ty: TypeRef::Number,
                optional: false,
                doc: None,
            }])],
            ..empty_model()
        };
        let err = a.merge(vec![b]).unwrap_err();
        k9::assert_equal!(
            err,
            MergeError::DuplicateEvent {
                name: "on_thing".to_string()
            }
        );
    }
}
