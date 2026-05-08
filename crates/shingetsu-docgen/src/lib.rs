//! Documentation and type-definition extraction for shingetsu.
//!
//! The entry point is [`extract`], which walks a populated
//! [`shingetsu_vm::GlobalEnv`] and produces a [`DocModel`].  The
//! [`DocModel`] is a stable, `serde`-serializable IR consumed by the
//! markdown / LuaLS / luau-lsp emitters and by external doc tooling
//! via the JSON export.
//!
//! # Embedder workflow
//!
//! Embedders (kumomta, wezterm, …) typically extend the standard
//! shingetsu environment with their own modules and userdata types.
//! Reference docs covering both the shingetsu core and embedder-
//! specific extensions can be produced in two flows.
//!
//! ## Library: produce JSON from your own `GlobalEnv`
//!
//! ```no_run
//! use shingetsu_docgen::extract;
//! use shingetsu_vm::GlobalEnv;
//!
//! # fn main() -> std::io::Result<()> {
//! let env = GlobalEnv::new();
//! // shingetsu::register_libs(&env, shingetsu::Libraries::ALL).unwrap();
//! // your_embedder::register_extensions(&env);
//! let model = extract(&env);
//! let json = serde_json::to_string_pretty(&model).unwrap();
//! std::fs::write("docs.json", json)?;
//! # Ok(()) }
//! ```
//!
//! ## CLI: render markdown from a JSON export
//!
//! Once the JSON is on disk the `shingetsu` binary handles the
//! markdown emission, so embedders don't need to link the markdown
//! emitter into their own build:
//!
//! ```text
//! $ shingetsu doc render-markdown \
//!     --input docs.json \
//!     --out site/reference/ \
//!     --front-matter zensical
//! ```
//!
//! The `shingetsu doc dump-json` and `shingetsu doc render-luau`
//! subcommands cover the symmetric flow when the embedder runs the
//! shingetsu binary directly without producing JSON first.

mod display;
mod luau;
mod markdown;
mod populate;
mod synopsis;
mod typeref;

use bstr::ByteSlice;
use serde::{Deserialize, Serialize};
use shingetsu_vm::types::{
    DocExample as VmDocExample, FieldDef, FieldKind, FunctionDef, LuaType, MetamethodDef,
    ModuleType, UserdataType,
};
use shingetsu_vm::GlobalEnv;

pub use display::display as display_type;
pub use luau::render_luau;
pub use markdown::{
    render_markdown, render_nav_fragment, FrontMatterStyle, MdFile, MdOptions, SplitMode,
};
pub use populate::{populate_example_outputs, ExampleFailure, ExampleOutcome};
pub use synopsis::render_synopsis;
pub use typeref::{TypeRef, TypeRefField, TypeRefIndexer, TypeRefParam};

/// Schema version for the JSON export.  Incremented by 1 on every
/// breaking change to the [`DocModel`] shape.
pub const SCHEMA_VERSION: u32 = 8;

/// Top-level documentation model produced by [`extract`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocModel {
    pub schema_version: u32,
    pub modules: Vec<ModuleDoc>,
    pub userdata_types: Vec<UserdataDoc>,
    pub globals: Vec<FieldDoc>,
    /// Events declared via `declare_event!` (or the migration
    /// facade's equivalent).  Sorted by name for stable output.
    /// Defaults to an empty vec when deserializing JSON produced
    /// against an older schema, so consumers can keep loading
    /// historical doc-model dumps without explicit migration.
    #[serde(default)]
    pub events: Vec<EventDoc>,
}

/// A `require`-able module exposed from Rust via `#[shingetsu::module]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModuleDoc {
    pub name: String,
    pub doc: Option<String>,
    pub strict: bool,
    pub fields: Vec<FieldDoc>,
    pub functions: Vec<FunctionDoc>,
}

/// A userdata type exposed from Rust via `#[shingetsu::userdata]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserdataDoc {
    pub name: String,
    pub doc: Option<String>,
    pub fields: Vec<FieldDoc>,
    pub methods: Vec<FunctionDoc>,
    pub metamethods: Vec<MetamethodDoc>,
}

/// A typed event handler slot, populated from
/// `GlobalTypeMap::event_handler_signatures` (the registry the
/// type checker consults when validating handler lambdas).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventDoc {
    /// Event name as it appears in `host.on("name", ...)` calls.
    pub name: String,
    /// Event-level summary captured on the `static` declaration
    /// inside `declare_event!`.  `None` when no rustdoc was attached.
    pub doc: Option<String>,
    /// Pre-rendered synopsis, e.g. `on_reset(queue, manual) -> bool`.
    pub synopsis: String,
    /// Per-parameter shape that handler lambdas must accept.
    pub params: Vec<ParamDoc>,
    /// Return types the handler must produce.  Empty for `()` /
    /// fire-and-forget events.
    pub returns: Vec<TypeRef>,
    /// Single prose description applying to the return tuple as a
    /// whole, captured via `#[returns = "..."]` inside
    /// `declare_event!`.  Distinct from `FunctionDoc`'s per-return
    /// `doc` because event signatures expose only one combined
    /// description.
    pub return_doc: Option<String>,
}

/// A function or method.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionDoc {
    pub name: String,
    pub doc: Option<String>,
    /// Pre-rendered single-line synopsis suitable for display in
    /// generated docs (e.g. `io.open(filename [, mode]) -> file | nil, errmsg`).
    /// Populated during extraction so JSON consumers don't have to
    /// recompute it.
    pub synopsis: String,
    pub params: Vec<ParamDoc>,
    pub variadic: Option<TypeRef>,
    /// Documentation for the variadic tail (`...`).  `None` unless
    /// the rustdoc `# Parameters` section had a `` - `...` — desc ``
    /// entry.
    pub variadic_doc: Option<String>,
    pub returns: Vec<ReturnDoc>,
    /// `true` for userdata methods (the receiver is implicit).
    pub is_method: bool,
    /// Structured `# Examples` content; one entry per fenced block.
    pub examples: Vec<DocExample>,
}

/// A metamethod entry on a userdata type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetamethodDoc {
    /// The metamethod name as it appears in Lua (e.g. `"__index"`).
    pub method: String,
    pub doc: Option<String>,
    pub synopsis: String,
    pub params: Vec<ParamDoc>,
    pub variadic: Option<TypeRef>,
    pub variadic_doc: Option<String>,
    pub returns: Vec<ReturnDoc>,
    pub examples: Vec<DocExample>,
}

/// One fenced code block from a rustdoc `# Examples` section.
///
/// `output` is populated by
/// [`populate_example_outputs`] after running the example; it stays
/// `None` until then or when the example is marked
/// `\`\`\`lua,no_run`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocExample {
    pub prose: Option<String>,
    pub language: String,
    pub flags: Vec<String>,
    pub code: String,
    /// Captured stdout from running the example, if populated.
    pub output: Option<String>,
}

impl From<&VmDocExample> for DocExample {
    fn from(ex: &VmDocExample) -> Self {
        DocExample {
            prose: ex.prose.clone(),
            language: ex.language.clone(),
            flags: ex.flags.clone(),
            code: ex.code.clone(),
            output: None,
        }
    }
}

/// A function or method parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamDoc {
    /// Parameter name.  `None` for positional-only parameters where the
    /// macro could not derive a name (rare).
    pub name: Option<String>,
    pub ty: TypeRef,
    /// `true` when the Rust signature wraps the type in `Option<T>`.
    pub optional: bool,
    pub doc: Option<String>,
}

/// A function return value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReturnDoc {
    pub ty: TypeRef,
    pub doc: Option<String>,
}

/// A field on a module or userdata type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldDoc {
    pub name: String,
    pub doc: Option<String>,
    pub ty: TypeRef,
    pub kind: FieldDocKind,
    pub examples: Vec<DocExample>,
}

/// User-visible access mode for a [`FieldDoc`].
///
/// Mirrors what a Lua user can do with the field, ignoring the
/// underlying runtime mechanism.  Module-level eager constants and
/// userdata fields with both a getter and setter both surface as
/// [`ReadWrite`](Self::ReadWrite) since the user can read or write
/// either of them; the distinction between them is an implementation
/// detail not surfaced through docgen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldDocKind {
    /// Read-only: writes are rejected.
    Getter,
    /// Write-only: reads are rejected.
    Setter,
    /// Both reads and writes are allowed.
    ReadWrite,
}

impl From<FieldKind> for FieldDocKind {
    fn from(k: FieldKind) -> Self {
        match k {
            FieldKind::Eager | FieldKind::ReadWrite => FieldDocKind::ReadWrite,
            FieldKind::Getter => FieldDocKind::Getter,
            FieldKind::Setter => FieldDocKind::Setter,
        }
    }
}

/// Walk a populated [`GlobalEnv`] and build a [`DocModel`].
///
/// Sources, in order:
/// - every module registered with
///   [`GlobalEnv::register_preload_typed`](shingetsu_vm::GlobalEnv::register_preload_typed)
///   (typically via `#[shingetsu::module]`-generated `register_preload`).
/// - every userdata type registered with
///   [`GlobalEnv::register_userdata_type`](shingetsu_vm::GlobalEnv::register_userdata_type).
///
/// The extractor sorts modules and userdata types by name for stable
/// output; functions, fields, and metamethods retain their declared
/// order.
pub fn extract(env: &GlobalEnv) -> DocModel {
    let mut modules: Vec<ModuleDoc> = env
        .preload_module_types()
        .snapshot()
        .into_iter()
        .filter_map(|(name, info)| match info.return_type {
            Some(LuaType::Module(m)) => Some(module_doc_from(name.to_str_lossy().into_owned(), &m)),
            _ => None,
        })
        .collect();
    modules.sort_by(|a, b| a.name.cmp(&b.name));

    let userdata_types: Vec<UserdataDoc> =
        env.userdata_types().iter().map(userdata_doc_from).collect();

    let mut events: Vec<EventDoc> = env
        .global_type_map()
        .event_handler_signatures
        .iter()
        .map(|(name, sig)| event_doc_from(name.to_str_lossy().into_owned(), sig))
        .collect();
    events.sort_by(|a, b| a.name.cmp(&b.name));

    DocModel {
        schema_version: SCHEMA_VERSION,
        modules,
        userdata_types,
        globals: Vec::new(),
        events,
    }
}

fn event_doc_from(name: String, sig: &shingetsu_vm::EventHandlerSignature) -> EventDoc {
    let mut params = Vec::with_capacity(sig.function_type.params.len());
    for p in sig.function_type.params.iter() {
        let (inner_ty, optional) = match &p.lua_type {
            LuaType::Optional(inner) => ((**inner).clone(), true),
            other => (other.clone(), false),
        };
        params.push(ParamDoc {
            name: p.name.as_ref().map(|b| b.to_str_lossy().into_owned()),
            ty: TypeRef::from_lua_type(&inner_ty),
            optional,
            doc: p.doc.clone(),
        });
    }
    let returns: Vec<TypeRef> = sig
        .function_type
        .returns
        .iter()
        .map(TypeRef::from_lua_type)
        .collect();
    // Event synopses use bare names with no module qualifier and
    // never render as method-style with a `:` separator.
    let returns_for_synopsis: Vec<ReturnDoc> = returns
        .iter()
        .cloned()
        .map(|ty| ReturnDoc { ty, doc: None })
        .collect();
    let synopsis = render_synopsis("", &name, &params, None, &returns_for_synopsis, false);
    EventDoc {
        name,
        doc: sig.doc.clone(),
        synopsis,
        params,
        returns,
        return_doc: sig.return_doc.clone(),
    }
}

/// Returns the qualifier shown in front of a function name in synopses
/// and page headings. The `builtins` module is special: its functions
/// are bound directly into `_G`, so they should display as bare names
/// (e.g. `error(msg?)`, not `builtins.error(msg?)`).
pub(crate) fn display_parent(module_name: &str) -> &str {
    if module_name == "builtins" {
        ""
    } else {
        module_name
    }
}

fn module_doc_from(name: String, m: &ModuleType) -> ModuleDoc {
    let display = display_parent(&name);
    let fields = m.fields.iter().map(field_doc_from).collect();
    let functions = m
        .functions
        .iter()
        .map(|f| function_doc_from(display, f, false))
        .collect();
    ModuleDoc {
        name,
        doc: m.doc.clone(),
        strict: m.strict,
        fields,
        functions,
    }
}

fn userdata_doc_from(ud: &UserdataType) -> UserdataDoc {
    let name = ud.name.to_str_lossy().into_owned();
    let fields = ud.fields.iter().map(field_doc_from).collect();
    let methods = ud
        .methods
        .iter()
        .map(|f| function_doc_from(&name, f, true))
        .collect();
    let metamethods = ud
        .metamethods
        .iter()
        .map(|mm| metamethod_doc_from(&name, mm))
        .collect();
    UserdataDoc {
        name,
        doc: ud.doc.clone(),
        fields,
        methods,
        metamethods,
    }
}

fn field_doc_from(f: &FieldDef) -> FieldDoc {
    FieldDoc {
        name: f.name.to_str_lossy().into_owned(),
        doc: f.doc.clone(),
        ty: TypeRef::from_lua_type(&f.lua_type),
        kind: f.kind.into(),
        examples: f.examples.iter().map(DocExample::from).collect(),
    }
}

fn function_doc_from(parent: &str, f: &FunctionDef, is_method: bool) -> FunctionDoc {
    let (params, variadic, returns) = signature_to_doc(&f.signature, &f.returns_doc);
    let name = f.name.to_str_lossy().into_owned();
    let synopsis = render_synopsis(
        parent,
        &name,
        &params,
        variadic.as_ref(),
        &returns,
        is_method,
    );
    FunctionDoc {
        name,
        doc: f.doc.clone(),
        synopsis,
        params,
        variadic,
        variadic_doc: f.signature.variadic_doc.clone(),
        returns,
        is_method,
        examples: f.examples.iter().map(DocExample::from).collect(),
    }
}

fn metamethod_doc_from(parent: &str, mm: &MetamethodDef) -> MetamethodDoc {
    let (params, variadic, returns) = signature_to_doc(&mm.signature, &mm.returns_doc);
    let method = mm.method.name().to_owned();
    let synopsis = render_synopsis(parent, &method, &params, variadic.as_ref(), &returns, true);
    MetamethodDoc {
        method,
        doc: mm.doc.clone(),
        synopsis,
        params,
        variadic,
        variadic_doc: mm.signature.variadic_doc.clone(),
        returns,
        examples: mm.examples.iter().map(DocExample::from).collect(),
    }
}

fn signature_to_doc(
    sig: &shingetsu_vm::FunctionSignature,
    returns_doc: &[String],
) -> (Vec<ParamDoc>, Option<TypeRef>, Vec<ReturnDoc>) {
    // `params` already excludes the implicit `self` for userdata
    // methods — `arg_offset` adjusts the *runtime* args list, not the
    // declared parameter list, so we iterate the full vec here.
    let mut params = Vec::with_capacity(sig.params.len());
    for p in sig.params.iter() {
        let raw_ty = p.lua_type.clone().unwrap_or(LuaType::Any);
        let (inner_ty, optional) = match raw_ty {
            LuaType::Optional(inner) => (*inner, true),
            other => (other, false),
        };
        params.push(ParamDoc {
            name: p.name.as_ref().map(|b| b.to_str_lossy().into_owned()),
            ty: TypeRef::from_lua_type(&inner_ty),
            optional,
            doc: p.doc.clone(),
        });
    }

    let variadic = if sig.variadic {
        Some(TypeRef::Any)
    } else {
        None
    };

    let returns: Vec<ReturnDoc> = sig
        .lua_returns
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .enumerate()
        .map(|(i, ty)| ReturnDoc {
            ty: TypeRef::from_lua_type(ty),
            doc: returns_doc.get(i).cloned(),
        })
        .collect();

    (params, variadic, returns)
}
