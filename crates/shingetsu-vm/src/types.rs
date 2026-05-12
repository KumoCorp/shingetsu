use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::byte_string::Bytes;
use bstr::BStr;

use crate::meta_method::MetaMethod;
use crate::value::Value;

/// Attribute on a `local` declaration (Lua 5.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LocalAttr {
    None,
    /// `local x <const>`: compile-time write-protection, no runtime cost.
    Const,
    /// `local x <close>`: `__close` is called when the variable goes out of scope.
    Close,
}

/// Simplified runtime-checkable type, used for call boundary validation.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueType {
    Nil,
    Boolean,
    Integer,
    Float,
    /// `Integer` or `Float`.
    Number,
    String,
    Table,
    Function,
    /// Any `Userdata` value.
    Userdata,
    /// `Userdata` whose `type_name()` matches the given string.
    UserdataOf(&'static str),
    /// Unconstrained.
    Any,
}

/// Source-level type expression from Lua 5.4 or LuaU annotations.
#[derive(Debug, Clone, PartialEq)]
pub enum LuaType {
    Nil,
    Boolean,
    /// Lua 5.4 untyped number.
    Number,
    /// LuaU explicitly-integer type.
    Integer,
    /// LuaU explicitly-float type.
    Float,
    String,
    /// Dynamic / unconstrained.
    Any,
    /// LuaU top type.
    Unknown,
    /// LuaU bottom type.
    Never,
    /// Named type reference (type alias, class, userdata type name, etc.).
    Named(Bytes),
    /// Reference to a generic type parameter, e.g. `T` inside a generic body.
    TypeParam(Bytes),
    /// Generic instantiation: `Array<number>`, `Map<string, User>`.
    Generic {
        base: Box<LuaType>,
        args: Vec<LuaTypeArg>,
    },
    /// `T?`  =  `T | nil`.
    Optional(Box<LuaType>),
    Union(Vec<LuaType>),
    /// LuaU intersection.
    Intersection(Vec<LuaType>),
    /// Structural table type: `{ x: number, [string]: boolean }`.
    Table(Box<TableLuaType>),
    /// Function type: `(number, string) -> boolean`.
    Function(Box<FunctionLuaType>),
    StringLiteral(Bytes),
    BoolLiteral(bool),
    NumberLiteral(f64),
    /// Variadic tail: `...T`.
    Variadic(Box<LuaType>),
    /// Tuple return: `(number, string)`.
    Tuple(Vec<LuaType>),
    /// A Lua module exposed from Rust via `#[shingetsu::module]` or similar.
    Module(Box<ModuleType>),
}

/// Metadata describing a Rust-backed userdata type, harvested at
/// macro expansion time from `#[shingetsu::userdata]` impl blocks.
///
/// Peer to [`ModuleType`]: same shape minus the `strict` flag and the
/// free-function list (userdata only exposes methods and metamethods).
/// References to userdata types in [`LuaType`] use
/// [`LuaType::Named`] keyed by the `name` field; the
/// [`UserdataTypeRegistry`] resolves the name back to this descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct UserdataType {
    /// Canonical type name (matches `Userdata::type_name`).
    pub name: Bytes,
    /// Optional documentation string.
    pub doc: Option<String>,
    pub fields: Vec<FieldDef>,
    pub methods: Vec<FunctionDef>,
    pub metamethods: Vec<MetamethodDef>,
}

/// Registry mapping userdata type names to their [`UserdataType`]
/// descriptors.  Populated by the embedder (or stdlib) via
/// [`crate::GlobalEnv::register_userdata_type`].
#[derive(Debug, Default)]
pub struct UserdataTypeRegistry {
    types: crate::sync::Mutex<HashMap<Bytes, UserdataType>>,
}

impl UserdataTypeRegistry {
    pub fn insert(&self, ud: UserdataType) {
        self.types.lock().insert(ud.name.clone(), ud);
    }

    pub fn get(&self, name: &[u8]) -> Option<UserdataType> {
        self.types.lock().get(name).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.types.lock().is_empty()
    }

    pub fn len(&self) -> usize {
        self.types.lock().len()
    }

    /// Snapshot every registered type, sorted by name for stable
    /// docgen output.
    pub fn snapshot(&self) -> Vec<UserdataType> {
        let guard = self.types.lock();
        let mut out: Vec<UserdataType> = guard.values().cloned().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// Metadata describing a Rust-backed Lua module.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleType {
    /// Canonical module name (used by `require`).
    pub name: Bytes,
    /// Optional documentation string.
    pub doc: Option<String>,
    /// When `true`, `__index` and `__newindex` reject unknown keys.
    pub strict: bool,
    pub fields: Vec<FieldDef>,
    pub functions: Vec<FunctionDef>,
    pub methods: Vec<FunctionDef>,
    pub metamethods: Vec<MetamethodDef>,
}

/// A field exposed on a module or userdata type.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: Bytes,
    pub doc: Option<String>,
    pub lua_type: LuaType,
    pub kind: FieldKind,
    /// Structured `# Examples` content harvested from rustdoc.
    /// Empty when no examples were authored.
    pub examples: Vec<DocExample>,
}

/// One fenced code block from a `# Examples` section.
#[derive(Debug, Clone, PartialEq)]
pub struct DocExample {
    /// Optional prose paragraph appearing immediately before the
    /// code block, captured verbatim from the rustdoc.
    pub prose: Option<String>,
    /// Fence language tag, e.g. `"lua"`, `"text"`, `"rust"`.
    pub language: String,
    /// Comma-separated flags following the language in the fence
    /// info string.  `"lua,no_run"` yields `["no_run"]`.  Used by
    /// downstream tooling (e.g. example validators) to opt out of
    /// execution.
    pub flags: Vec<String>,
    /// Code body, verbatim, with the rustdoc `///` prefix removed.
    pub code: String,
}

impl DocExample {
    /// Returns `true` when `flag` is present in [`Self::flags`].
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

/// How a field's value is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Value is pre-computed at module construction time.
    Eager,
    /// Read-only: a Rust getter is invoked on each `__index`.
    Getter,
    /// Write-only: a Rust setter is invoked on each `__newindex`.
    Setter,
    /// Read-write: a getter is invoked on `__index` and a setter on
    /// `__newindex`.  Emitted by the userdata macro when the same
    /// Lua-visible field name has both a getter and a setter.
    ReadWrite,
}

/// A free function or method exposed on a module or userdata type.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    pub name: Bytes,
    pub doc: Option<String>,
    pub signature: FunctionSignature,
    /// Per-return-position documentation harvested from the rustdoc
    /// `# Returns` section.  Empty when no `# Returns` section is
    /// present; otherwise positionally aligned with `signature.lua_returns`.
    pub returns_doc: Vec<String>,
    /// Structured `# Examples` content harvested from rustdoc.
    /// Empty when no examples were authored.
    pub examples: Vec<DocExample>,
}

/// A metamethod exposed on a module or userdata type.
#[derive(Debug, Clone, PartialEq)]
pub struct MetamethodDef {
    pub method: MetaMethod,
    pub doc: Option<String>,
    pub signature: FunctionSignature,
    /// Per-return-position documentation; see [`FunctionDef::returns_doc`].
    pub returns_doc: Vec<String>,
    /// Structured `# Examples` content; see [`FieldDef::examples`].
    pub examples: Vec<DocExample>,
}

/// A type argument in a generic instantiation.
#[derive(Debug, Clone, PartialEq)]
pub enum LuaTypeArg {
    Type(LuaType),
    /// Type pack: `T...`.
    Pack(LuaType),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableLuaType {
    /// Named fields: `{ x: number, y: string }`.
    pub fields: Vec<TableField>,
    /// Index signature: `{ [K]: V }`.
    pub indexer: Option<(Box<LuaType>, Box<LuaType>)>,
}

/// One named field in a [`TableLuaType`].  Carries the rustdoc
/// captured at the field's declaration site (for structs built via
/// `#[derive(LuaTable)]`) and the textual representation of a
/// `#[lua(default = …)]` annotation, when present.  Both `doc` and
/// `default` are surfaced by `shingetsu-docgen` when rendering
/// parameter descriptions.
#[derive(Debug, Clone, PartialEq)]
pub struct TableField {
    pub name: Bytes,
    pub lua_type: LuaType,
    /// rustdoc on the field, joined with `\n`.  `None` when the
    /// field has no doc or the type was constructed programmatically.
    pub doc: Option<String>,
    /// Textual rendering of `#[lua(default = expr)]`, when present.
    /// Stored as a string because the default expression is arbitrary
    /// Rust syntax; consumers render it verbatim.
    pub default: Option<String>,
}

impl TableField {
    /// Construct a field with no doc and no default.  Equivalent to
    /// `(name, lua_type).into()` and intended as the ergonomic
    /// constructor for hand-built `TableLuaType` values (tests,
    /// runtime type inference).
    pub fn new(name: impl Into<Bytes>, lua_type: LuaType) -> Self {
        Self {
            name: name.into(),
            lua_type,
            doc: None,
            default: None,
        }
    }
}

impl From<(Bytes, LuaType)> for TableField {
    fn from((name, lua_type): (Bytes, LuaType)) -> Self {
        Self {
            name,
            lua_type,
            doc: None,
            default: None,
        }
    }
}

/// Named, typed, optionally-documented parameter in a
/// [`FunctionLuaType`].  The type system uses this as a single
/// per-parameter shape that carries an optional name (positional
/// parameters in inferred function types may not have one), the
/// `LuaType` annotation, and any rustdoc captured at the
/// parameter's declaration site.  The `#[function]` /
/// `#[lua_method]` / `declare_event!` macros populate `doc` from
/// rustdoc on the underlying parameter; `shingetsu-docgen` reads
/// it back when rendering reference pages.
///
/// `From<(Option<Bytes>, LuaType)>` is provided for callers that
/// don't capture rustdoc and prefer tuple syntax via `.into()`.
#[derive(Debug, Clone, PartialEq)]
pub struct TypedParam {
    pub name: Option<Bytes>,
    pub lua_type: LuaType,
    pub doc: Option<String>,
}

impl TypedParam {
    /// Construct a named typed parameter with no rustdoc.  The
    /// `name` argument accepts anything convertible into `Bytes`
    /// wrapped in `Some`, so call sites can write `Some("foo")` /
    /// `Some(some_bytes)` / `Some(opt_bytes_var)` without explicit
    /// `Bytes::from(...)` wrapping.  Use [`Self::unnamed`] when
    /// the parameter has no name (the bare `None` form would not
    /// give Rust enough information to infer the type parameter).
    pub fn new(name: Option<impl Into<Bytes>>, lua_type: LuaType) -> Self {
        Self {
            name: name.map(Into::into),
            lua_type,
            doc: None,
        }
    }

    /// Construct a named typed parameter carrying a captured
    /// rustdoc summary.  Used by macros that surface per-parameter
    /// docs into the type system.  See [`Self::new`] for the
    /// `name` argument shape; use [`Self::unnamed_with_doc`] for
    /// unnamed parameters.
    pub fn new_with_doc(
        name: Option<impl Into<Bytes>>,
        lua_type: LuaType,
        doc: Option<String>,
    ) -> Self {
        Self {
            name: name.map(Into::into),
            lua_type,
            doc,
        }
    }

    /// Construct an unnamed typed parameter (positional-only, no
    /// rustdoc).  Common in inferred function types where the
    /// surrounding code didn't have parameter names available.
    pub fn unnamed(lua_type: LuaType) -> Self {
        Self {
            name: None,
            lua_type,
            doc: None,
        }
    }

    /// Construct an unnamed typed parameter carrying a captured
    /// rustdoc summary.
    pub fn unnamed_with_doc(lua_type: LuaType, doc: Option<String>) -> Self {
        Self {
            name: None,
            lua_type,
            doc,
        }
    }
}

impl From<(Option<Bytes>, LuaType)> for TypedParam {
    fn from((name, lua_type): (Option<Bytes>, LuaType)) -> Self {
        Self {
            name,
            lua_type,
            doc: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionLuaType {
    pub type_params: Vec<GenericTypeParam>,
    /// Named parameters with type and optional doc:
    /// `(x: number, y: string)`.
    pub params: Vec<TypedParam>,
    pub variadic: Option<Box<LuaType>>,
    pub returns: Vec<LuaType>,
    /// Whether this function was defined with method syntax (`:`) or has
    /// `arg_offset > 0`, meaning it expects an implicit `self` argument.
    pub is_method: bool,
    /// When `true`, this type was inferred from parameter count alone
    /// (no Luau annotations).  Arg-count mismatches should be reported
    /// as warnings rather than errors.
    pub inferred_unannotated: bool,
}

impl FunctionLuaType {
    /// Returns `true` when this represents a generic untyped function
    /// (`(...any) -> ()`) with no concrete parameter or return information.
    pub fn is_untyped(&self) -> bool {
        self.type_params.is_empty()
            && self.params.is_empty()
            && self.returns.is_empty()
            && matches!(self.variadic.as_deref(), Some(LuaType::Any))
    }
}

/// A generic type parameter declaration, e.g. `T` or `T...`.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericTypeParam {
    pub name: Bytes,
    /// Default type when not explicitly supplied.
    pub default: Option<LuaType>,
    /// True for variadic type packs (`T...`).
    pub is_pack: bool,
}

/// A `type Foo<A, B> = ...` alias declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeAlias {
    /// Generic type parameters declared on this alias.
    pub params: Vec<GenericTypeParam>,
    /// The type expression on the right-hand side of `=`.
    pub body: LuaType,
    /// Whether this alias was declared with `export type` (visible to
    /// `require` consumers) rather than plain `type` (file-local).
    pub exported: bool,
}

/// Type surface of a compiled module, extracted during compilation.
///
/// Used by the cross-module type propagation system to determine
/// what types `require("foo")` makes available.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModuleTypeInfo {
    /// `export type` declarations visible to consumers.
    pub exported_types: HashMap<Bytes, TypeAlias>,
    /// The type of the value returned by the module chunk.
    /// `None` if not determinable at compile time.
    pub return_type: Option<LuaType>,
}

/// Registry mapping module names to their type surfaces.
///
/// Provided to the `Compiler` so that `require` calls can be
/// resolved to typed module exports at compile time.
///
/// Uses interior mutability so the compiler can insert newly-resolved
/// modules during demand-driven `require` resolution while holding
/// only `&self`.
#[derive(Debug, Default)]
pub struct ModuleTypeRegistry {
    modules: crate::sync::Mutex<HashMap<Bytes, ModuleTypeInfo>>,
    /// Module names currently being compiled — used to detect
    /// circular `require` chains and break the cycle.
    in_progress: crate::sync::Mutex<std::collections::HashSet<Bytes>>,
}

impl ModuleTypeRegistry {
    /// Insert a module's type info, keyed by its `require` name
    /// (e.g. `"foo.bar"`).
    pub fn insert(&self, name: impl Into<Bytes>, info: ModuleTypeInfo) {
        self.modules.lock().insert(name.into(), info);
    }

    /// Look up a module by its `require` name and clone the result.
    pub fn get(&self, name: &[u8]) -> Option<ModuleTypeInfo> {
        self.modules.lock().get(name).cloned()
    }

    /// Returns `true` if the registry contains no modules.
    pub fn is_empty(&self) -> bool {
        self.modules.lock().is_empty()
    }

    /// Number of modules in the registry.
    pub fn len(&self) -> usize {
        self.modules.lock().len()
    }

    /// Snapshot every registered module as `(name, info)` pairs,
    /// sorted by name for stable docgen output.
    pub fn snapshot(&self) -> Vec<(Bytes, ModuleTypeInfo)> {
        let guard = self.modules.lock();
        let mut out: Vec<(Bytes, ModuleTypeInfo)> =
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Mark a module as currently being compiled.
    /// Returns `false` if it was already in progress (circular require).
    pub fn begin_compile(&self, name: &[u8]) -> bool {
        self.in_progress.lock().insert(Bytes::from(name))
    }

    /// Remove a module from the in-progress set.
    pub fn end_compile(&self, name: &[u8]) {
        self.in_progress.lock().remove(name);
    }

    /// Check if a module is currently being compiled.
    pub fn is_in_progress(&self, name: &[u8]) -> bool {
        self.in_progress.lock().contains(name)
    }
}

/// Per-parameter specification used in [`FunctionSignature`].
#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    /// Parameter name.
    pub name: Option<Bytes>,
    /// Simplified runtime type for fast call validation.
    /// `None` means unconstrained.
    pub runtime_type: Option<ValueType>,
    /// Full source-level type annotation.
    /// `None` for Lua 5.4 params without annotations.
    pub lua_type: Option<LuaType>,
    /// Documentation harvested from the rustdoc `# Parameters` section
    /// on `#[shingetsu::function]` / `#[shingetsu::userdata]` items.
    /// `None` for Lua-defined functions.
    pub doc: Option<String>,
}

/// Shared between compiled Lua functions and host-registered native functions.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionSignature {
    /// Function name for stack traces and error messages.
    pub name: Bytes,
    /// Source name (e.g. `"<string>"` or a file path) for debug info.
    /// Empty for native functions.
    pub source: Bytes,
    /// Generic type parameter declarations (LuaU; empty for Lua 5.4).
    pub type_params: Vec<GenericTypeParam>,
    pub params: Vec<ParamSpec>,
    pub variadic: bool,
    /// Documentation for the variadic tail (`...`) harvested from
    /// the rustdoc `# Parameters` section as `- \`...\` — desc`.
    /// `None` when the variadic isn't documented.  Ignored when
    /// `variadic` is `false`.
    pub variadic_doc: Option<String>,
    /// Number of leading args to skip before matching `params`.
    /// Used for userdata methods where the first Lua arg is `self`.
    pub arg_offset: usize,
    /// Simplified runtime return types; `None` means unspecified.
    pub returns: Option<Vec<ValueType>>,
    /// Source-level return type annotations; `None` if unavailable.
    pub lua_returns: Option<Vec<LuaType>>,
    /// 1-based source line where the function definition starts.
    /// `0` for the main chunk (Lua 5.4 convention) and native functions.
    pub line_defined: u32,
    /// 1-based source line of the closing `end` token.
    /// For the main chunk, the last line of the source file.
    /// `0` for native functions.
    pub last_line_defined: u32,
    /// Number of upvalues captured by this function.  `0` for native
    /// functions.
    pub num_upvalues: u8,
    /// `true` when at least one param has a `runtime_type` constraint.
    /// Used by `validate_args` to skip iteration entirely for untyped
    /// Lua functions.
    pub has_runtime_types: bool,
}

/// Luau-style textual rendering of a [`LuaType`].
///
/// The rendering is intended for human-facing diagnostics (traceback,
/// error messages, doc extraction) rather than round-tripping through
/// the parser.  Precedence is only made explicit with parentheses in
/// the cases that would otherwise change meaning:
///
/// * `Optional` wrapping a `Union`/`Intersection` is parenthesised
///   (`(A | B)?`), because bare `A | B?` parses as `A | (B?)`.
/// * `Union` whose element is an `Intersection` is parenthesised, and
///   vice versa, because the two operators compose awkwardly without
///   grouping.
///
/// # Examples
///
/// ```
/// use shingetsu_vm::LuaType;
///
/// let opt_num = LuaType::Optional(Box::new(LuaType::Number));
/// assert_eq!(opt_num.to_string(), "number?");
/// ```
impl LuaType {
    /// Construct a basic [`LuaType`] from its source-level name. Recognises
    /// the atomic type names (`"nil"`, `"boolean"`, `"number"`, `"integer"`,
    /// `"float"`, `"string"`, `"any"`, `"unknown"`, `"never"`) and falls
    /// back to [`LuaType::Named`] for anything else.
    ///
    /// Round-trip companion to [`simple_type_name`](Self::simple_type_name)
    /// for those atomic types. Doesn't resolve generic type parameters or
    /// type aliases — callers that need that machinery should go through
    /// the compiler's type-conversion path.
    pub fn from_basic_name(name: &str) -> Self {
        match name {
            "nil" => Self::Nil,
            "boolean" => Self::Boolean,
            "number" => Self::Number,
            "integer" => Self::Integer,
            "float" => Self::Float,
            "string" => Self::String,
            "any" => Self::Any,
            "unknown" => Self::Unknown,
            "never" => Self::Never,
            other => Self::Named(Bytes::from(other)),
        }
    }

    /// Construct a [`LuaType::Named`] from anything convertible into
    /// `Bytes`.  Convenience over the bare variant for call sites
    /// that build named types from string literals or owned
    /// strings without wanting to write `Bytes::from(...)` by hand.
    pub fn named(name: impl Into<Bytes>) -> Self {
        Self::Named(name.into())
    }

    /// Construct a [`LuaType::TypeParam`] from anything convertible
    /// into `Bytes`.  Convenience over the bare variant; same
    /// motivation as [`Self::named`].
    pub fn type_param(name: impl Into<Bytes>) -> Self {
        Self::TypeParam(name.into())
    }

    /// Build a [`LuaType::Function`] from a [`FunctionSignature`].
    ///
    /// Skips the first `arg_offset` parameters (used by userdata methods
    /// to hide the implicit `self`), folds in `runtime_type` annotations
    /// when no Luau type is set, and propagates `lua_returns` /
    /// `variadic` / `type_params` directly.
    pub fn from_function_signature(sig: &FunctionSignature) -> Self {
        infer_function_type(sig)
    }

    /// Three-state member lookup for type-checker validation.
    ///
    /// Returns:
    /// - `None`: this type's schema is unknown or unconstrained — the
    ///   caller should skip "unknown field" diagnostics. Includes empty
    ///   tables, tables with an indexer, scalars, named aliases without a
    ///   resolved schema, and so on.
    /// - `Some(None)`: schema is known but `name` is not in it — the
    ///   caller can confidently emit "unknown field" diagnostics.
    /// - `Some(Some(ty))`: `name` resolves to this type.  Returned as a
    ///   [`std::borrow::Cow`] because [`LuaType::Module`] members synthesize a fresh
    ///   owned [`LuaType::Function`] from a [`FunctionSignature`] on
    ///   demand, while [`LuaType::Table`] entries can be borrowed
    ///   directly.
    ///
    /// When `userdata` is `Some`, [`LuaType::Named`] references are
    /// resolved against the registry: a hit synthesizes the same
    /// `Cow::Owned` shape `LuaType::Module` does, by treating the
    /// userdata's `methods` as method-style functions.  A miss
    /// (`None` from the registry) falls through to the `None`
    /// branch -- treated as an unknown schema.
    pub fn lookup_known_member(
        &self,
        name: &[u8],
        userdata: Option<&UserdataTypeRegistry>,
    ) -> Option<Option<std::borrow::Cow<'_, LuaType>>> {
        match self {
            LuaType::Table(t) => {
                if t.fields.is_empty() || t.indexer.is_some() {
                    return None;
                }
                for field in &t.fields {
                    if field.name.as_ref() == name {
                        return Some(Some(std::borrow::Cow::Borrowed(&field.lua_type)));
                    }
                }
                Some(None)
            }
            LuaType::Module(m) => {
                for f in &m.fields {
                    if matches!(f.kind, FieldKind::Setter) {
                        continue;
                    }
                    if f.name.as_ref() == name {
                        return Some(Some(std::borrow::Cow::Borrowed(&f.lua_type)));
                    }
                }
                for f in m.functions.iter().chain(m.methods.iter()) {
                    if f.name.as_ref() == name {
                        return Some(Some(std::borrow::Cow::Owned(
                            LuaType::from_function_signature(&f.signature),
                        )));
                    }
                }
                Some(None)
            }
            LuaType::Named(ud_name) => match userdata.and_then(|r| r.get(ud_name)) {
                Some(ud) => Some(lookup_in_userdata(&ud, name)),
                None => None,
            },
            LuaType::Optional(inner) => inner.lookup_known_member(name, userdata),
            LuaType::Generic { base, .. } => base.lookup_known_member(name, userdata),
            _ => None,
        }
    }

    /// Look up a member's type by name on a [`LuaType`], descending through
    /// wrappers ([`Optional`](LuaType::Optional),
    /// [`Generic`](LuaType::Generic)).
    ///
    /// Returns `None` for types that have no statically-known members
    /// (scalars, named aliases without a registry, etc.). The string
    /// metatable's `__index` is *not* consulted here — callers that need
    /// `s:method()` lookup on `LuaType::String` should consult the env
    /// separately.
    ///
    /// `userdata` mirrors the parameter on
    /// [`Self::lookup_known_member`]: when supplied, [`LuaType::Named`]
    /// references are followed into the registry.
    pub fn lookup_member(
        &self,
        name: &[u8],
        userdata: Option<&UserdataTypeRegistry>,
    ) -> Option<LuaType> {
        match self {
            LuaType::Module(m) => {
                for f in &m.fields {
                    if matches!(f.kind, FieldKind::Setter) {
                        continue;
                    }
                    if f.name.as_ref() == name {
                        return Some(f.lua_type.clone());
                    }
                }
                for f in m.functions.iter().chain(m.methods.iter()) {
                    if f.name.as_ref() == name {
                        return Some(LuaType::from_function_signature(&f.signature));
                    }
                }
                None
            }
            LuaType::Table(t) => t
                .fields
                .iter()
                .find(|f| f.name.as_ref() == name)
                .map(|f| f.lua_type.clone()),
            LuaType::Named(ud_name) => userdata.and_then(|r| r.get(ud_name)).and_then(|ud| {
                match lookup_in_userdata(&ud, name) {
                    Some(cow) => Some(cow.into_owned()),
                    None => None,
                }
            }),
            LuaType::Optional(inner) => inner.lookup_member(name, userdata),
            LuaType::Generic { base, .. } => base.lookup_member(name, userdata),
            _ => None,
        }
    }

    /// Enumerate the names accessible via `.` or `:` on a value of this type.
    ///
    /// Descends through [`Optional`](LuaType::Optional) and
    /// [`Generic`](LuaType::Generic) wrappers; combines
    /// [`Union`](LuaType::Union) sets the conservative way (only members
    /// present in *every* arm) and [`Intersection`](LuaType::Intersection)
    /// sets liberally (any arm's members). Setter-only fields are
    /// excluded.
    ///
    /// Returns names without de-duplication or sorting; callers that
    /// surface them as a UI list should handle that themselves.
    pub fn member_names(&self) -> Vec<Bytes> {
        let mut out = Vec::new();
        self.collect_member_names(&mut out);
        out
    }

    fn collect_member_names(&self, out: &mut Vec<Bytes>) {
        match self {
            LuaType::Module(m) => {
                for f in &m.fields {
                    if matches!(f.kind, FieldKind::Setter) {
                        continue;
                    }
                    out.push(f.name.clone());
                }
                for f in &m.functions {
                    out.push(f.name.clone());
                }
                for method in &m.methods {
                    out.push(method.name.clone());
                }
            }
            LuaType::Table(t) => {
                for field in &t.fields {
                    out.push(field.name.clone());
                }
            }
            LuaType::Optional(inner) => inner.collect_member_names(out),
            LuaType::Generic { base, .. } => base.collect_member_names(out),
            LuaType::Intersection(arms) => {
                for arm in arms {
                    arm.collect_member_names(out);
                }
            }
            LuaType::Union(arms) => {
                let mut sets: Vec<HashSet<Bytes>> = arms
                    .iter()
                    .map(|t| t.member_names().into_iter().collect())
                    .collect();
                if let Some(first) = sets.pop() {
                    let intersection = sets
                        .into_iter()
                        .fold(first, |acc, s| acc.intersection(&s).cloned().collect());
                    out.extend(intersection);
                }
            }
            _ => {}
        }
    }

    /// First return type when this type is called as a function.
    ///
    /// Returns the first element of [`FunctionLuaType::returns`] for
    /// [`LuaType::Function`]; descends through
    /// [`Optional`](LuaType::Optional) and [`Generic`](LuaType::Generic)
    /// wrappers. Returns `None` for non-callable types and for functions
    /// with no declared return.
    pub fn call_return_type(&self) -> Option<LuaType> {
        match self {
            LuaType::Function(f) => f.returns.first().cloned(),
            LuaType::Optional(inner) => inner.call_return_type(),
            LuaType::Generic { base, .. } => base.call_return_type(),
            _ => None,
        }
    }

    /// Returns the simple Lua type category name, suitable for error
    /// messages (e.g. `"function"` instead of `"(...any) -> ()"`).
    pub fn simple_type_name(&self) -> String {
        match self {
            LuaType::Nil => "nil".to_owned(),
            LuaType::Boolean | LuaType::BoolLiteral(_) => "boolean".to_owned(),
            LuaType::Number | LuaType::NumberLiteral(_) => "number".to_owned(),
            LuaType::Integer => "integer".to_owned(),
            LuaType::Float => "float".to_owned(),
            LuaType::String | LuaType::StringLiteral(_) => "string".to_owned(),
            LuaType::Table(_) => "table".to_owned(),
            LuaType::Function(f) if f.is_untyped() => "function".to_owned(),
            LuaType::Function(f) => f.to_string(),
            LuaType::Named(n) => String::from_utf8_lossy(n).into_owned(),
            LuaType::Optional(inner) => format!("{}?", inner.simple_type_name()),
            LuaType::Union(types) => types
                .iter()
                .map(|t| t.simple_type_name())
                .collect::<Vec<_>>()
                .join(" | "),
            // For everything else, fall back to Display.
            other => other.to_string(),
        }
    }
}

impl fmt::Display for LuaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LuaType::Nil => f.write_str("nil"),
            LuaType::Boolean => f.write_str("boolean"),
            LuaType::Number => f.write_str("number"),
            LuaType::Integer => f.write_str("integer"),
            LuaType::Float => f.write_str("float"),
            LuaType::String => f.write_str("string"),
            LuaType::Any => f.write_str("any"),
            LuaType::Unknown => f.write_str("unknown"),
            LuaType::Never => f.write_str("never"),
            LuaType::Named(n) => write!(f, "{}", BStr::new(n)),
            LuaType::TypeParam(n) => write!(f, "{}", BStr::new(n)),
            LuaType::Generic { base, args } => {
                write!(f, "{base}<")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                f.write_str(">")
            }
            LuaType::Optional(inner) => match inner.as_ref() {
                // `A | B?` parses as `A | (B?)`, so a top-level optional
                // wrapping a union/intersection must be parenthesised.
                LuaType::Union(_) | LuaType::Intersection(_) => {
                    write!(f, "({inner})?")
                }
                _ => write!(f, "{inner}?"),
            },
            LuaType::Union(ts) => {
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    match t {
                        LuaType::Intersection(_) => write!(f, "({t})")?,
                        _ => write!(f, "{t}")?,
                    }
                }
                Ok(())
            }
            LuaType::Intersection(ts) => {
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" & ")?;
                    }
                    match t {
                        LuaType::Union(_) => write!(f, "({t})")?,
                        _ => write!(f, "{t}")?,
                    }
                }
                Ok(())
            }
            LuaType::Table(t) => write!(f, "{t}"),
            LuaType::Function(func) => write!(f, "{func}"),
            LuaType::StringLiteral(s) => {
                let s = BStr::new(s);
                write!(f, "\"{s}\"")
            }
            LuaType::BoolLiteral(b) => write!(f, "{b}"),
            LuaType::NumberLiteral(n) => {
                // Rust's default f64 Display drops the decimal point
                // for integer-valued floats (`1.0` -> "1"), which is
                // what Lua's `%g`-style formatting also does.
                write!(f, "{n}")
            }
            LuaType::Variadic(inner) => write!(f, "...{inner}"),
            LuaType::Tuple(ts) => {
                f.write_str("(")?;
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{t}")?;
                }
                f.write_str(")")
            }
            LuaType::Module(m) => {
                let name = BStr::new(&m.name);
                write!(f, "module<{name}>")
            }
        }
    }
}

/// Render a type argument in a generic instantiation.
///
/// A plain `Type(T)` renders as `T`; a type pack `Pack(T)` renders as
/// `T...` so that, e.g., `Array<number>` vs `Callback<T...>` round-trip
/// distinctly.
impl fmt::Display for LuaTypeArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LuaTypeArg::Type(t) => write!(f, "{t}"),
            LuaTypeArg::Pack(t) => write!(f, "{t}..."),
        }
    }
}

/// Render a structural table type.
///
/// Three shapes are produced:
///
/// * `{T}` — array shorthand: no named fields, and an indexer keyed on
///   `number`/`integer`.
/// * `{ [K]: V }` — map shorthand: no named fields, indexer keyed on a
///   non-numeric type.
/// * `{ field: T, field2: U, [K]: V }` — full record with an optional
///   trailing indexer clause when both named fields and an indexer are
///   present.
///
/// An empty table type renders as `{}`.
impl fmt::Display for TableLuaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.fields.is_empty() {
            match &self.indexer {
                Some((k, v)) => {
                    if matches!(k.as_ref(), LuaType::Number | LuaType::Integer) {
                        return write!(f, "{{{v}}}");
                    }
                    return write!(f, "{{ [{k}]: {v} }}");
                }
                None => return f.write_str("{}"),
            }
        }
        f.write_str("{ ")?;
        for (i, field) in self.fields.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            let name = BStr::new(&field.name);
            write!(f, "{name}: {}", field.lua_type)?;
        }
        if let Some((k, v)) = &self.indexer {
            write!(f, ", [{k}]: {v}")?;
        }
        f.write_str(" }")
    }
}

/// Render a function type in Luau style: `<T>(params) -> returns`.
///
/// The parameter list prints each named parameter as `name: T` and
/// unnamed parameters as the type alone.  A trailing variadic renders
/// as `...T`.  The return clause is:
///
/// * `()` when there are no returns,
/// * `T` for a single return,
/// * `(A, B, ...)` for multiple returns.
impl FunctionLuaType {
    /// Format the function signature, optionally including a function name.
    pub fn display_with_name(&self, f: &mut fmt::Formatter<'_>, name: Option<&str>) -> fmt::Result {
        if let Some(name) = name {
            f.write_str("function ")?;
            f.write_str(name)?;
        }
        if !self.type_params.is_empty() {
            f.write_str("<")?;
            for (i, tp) in self.type_params.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{tp}")?;
            }
            f.write_str(">")?;
        }
        f.write_str("(")?;
        let mut first = true;
        for TypedParam {
            name: param_name,
            lua_type: ty,
            ..
        } in &self.params
        {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            if let Some(param_name) = param_name {
                let param_name = BStr::new(param_name);
                write!(f, "{param_name}: {ty}")?;
            } else {
                write!(f, "{ty}")?;
            }
        }
        if let Some(va) = &self.variadic {
            if !first {
                f.write_str(", ")?;
            }
            // A `Variadic` element type indicates a forwarded type pack
            // (e.g. `...: T...`), which already prints as `...T` and must
            // not pick up a second leading ellipsis.
            if matches!(va.as_ref(), LuaType::Variadic(_)) {
                write!(f, "{va}")?;
            } else {
                write!(f, "...{va}")?;
            }
        }
        f.write_str(") -> ")?;
        match self.returns.as_slice() {
            [] => f.write_str("()"),
            [single] => write!(f, "{single}"),
            many => {
                f.write_str("(")?;
                for (i, r) in many.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{r}")?;
                }
                f.write_str(")")
            }
        }
    }
}

impl fmt::Display for FunctionLuaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.display_with_name(f, None)
    }
}

/// Render a generic parameter declaration, with its type-pack marker
/// and default clause.
///
/// Shapes produced:
///
/// * `T` — plain parameter
/// * `T...` — type pack
/// * `T = number` — with default
impl fmt::Display for GenericTypeParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = BStr::new(&self.name);
        write!(f, "{name}")?;
        if self.is_pack {
            f.write_str("...")?;
        }
        if let Some(d) = &self.default {
            write!(f, " = {d}")?;
        }
        Ok(())
    }
}

/// Derive a runtime-checkable `ValueType` from a source-level `LuaType`
/// annotation.  Returns `None` for types that are too complex or
/// unconstrained to check cheaply at call boundaries.
pub fn derive_runtime_type(lt: &LuaType) -> Option<ValueType> {
    match lt {
        LuaType::Nil => Some(ValueType::Nil),
        LuaType::Boolean => Some(ValueType::Boolean),
        LuaType::Number => Some(ValueType::Number),
        LuaType::Integer => Some(ValueType::Integer),
        LuaType::Float => Some(ValueType::Float),
        LuaType::String => Some(ValueType::String),
        LuaType::Any | LuaType::Unknown => Some(ValueType::Any),
        // Table structural types are all tables at runtime.
        LuaType::Table(_) => Some(ValueType::Table),
        // Function types are all functions at runtime.
        LuaType::Function(_) => Some(ValueType::Function),
        // Optional(T) accepts nil, so we can't reject based on T alone.
        LuaType::Optional(_) => None,
        // Union/intersection — could handle simple cases but for now skip.
        LuaType::Union(_) | LuaType::Intersection(_) => None,
        // Named types could be userdata, but we can't resolve the name
        // to a concrete type at compile time without a type registry.
        LuaType::Named(_) => None,
        // Generic type parameters are erased at runtime (like LuaU).
        // The concrete type is unknown until call-site instantiation,
        // so we treat them as unconstrained.
        LuaType::TypeParam(_) => None,
        // Array shorthand is a table.
        LuaType::Generic { base, .. } => derive_runtime_type(base),
        // Literals — check the base type.
        LuaType::StringLiteral(_) => Some(ValueType::String),
        LuaType::BoolLiteral(_) => Some(ValueType::Boolean),
        LuaType::NumberLiteral(_) => Some(ValueType::Number),
        // Everything else: can't derive.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// GlobalTypeMap — compile-time type information inferred from the runtime
// environment (globals, native modules, userdata).  The compiler consumes
// this to produce diagnostics (e.g. dot-vs-colon call syntax warnings)
// without depending on `shingetsu-vm`.
// ---------------------------------------------------------------------------

/// A snapshot of the inferred types for all globals in a [`GlobalEnv`].
///
/// Built automatically by `GlobalEnv::set_global` and consumed by the
/// compiler's `TypeContext` for compile-time diagnostics.
///
/// [`GlobalEnv`]: crate::global_env::GlobalEnv
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GlobalTypeMap {
    /// Global name → inferred `LuaType`.
    pub types: HashMap<Bytes, LuaType>,
    /// Fully-qualified call paths (e.g. `b"wezterm.on"`) of globals
    /// that the host has marked as event-handler registrars.  When
    /// the type checker sees a call to one of these and the first
    /// argument is a string literal, it looks up the event's typed
    /// signature in [`Self::event_handler_signatures`] and validates
    /// the second (function) argument against it.  Populated by
    /// `#[function(event_registrar)]` and the supporting macros.
    pub event_registrars: HashSet<Bytes>,
    /// Event name → [`EventHandlerSignature`] capturing the
    /// declared handler shape (a [`FunctionLuaType`]) plus any
    /// rustdoc the host attached at declaration time.  Populated
    /// by `CallbackSignature::register_compile_type` (and the
    /// migration facade's `EventSignature::register`).  Walked by
    /// the type checker for handler-lambda validation and by
    /// `shingetsu_docgen::extract` to produce per-event reference
    /// pages.
    pub event_handler_signatures: HashMap<Bytes, EventHandlerSignature>,
}

/// Registry value paired with each event name in
/// [`GlobalTypeMap::event_handler_signatures`].  Carries the
/// `FunctionLuaType` the type checker validates handler lambdas
/// against, and the event-level rustdoc the docgen pipeline
/// renders into per-event reference pages.  Per-parameter docs
/// live inside `function_type.params[i].doc` (the `TypedParam`
/// shape carries them); this struct only adds the event-level
/// summary and return-value description.
#[derive(Debug, Clone, PartialEq)]
pub struct EventHandlerSignature {
    pub function_type: FunctionLuaType,
    pub doc: Option<String>,
    pub return_doc: Option<String>,
}

impl From<FunctionLuaType> for EventHandlerSignature {
    /// Convenience for callers that don't yet capture rustdoc:
    /// wrap a bare `FunctionLuaType` with empty event-level doc
    /// fields.  Per-parameter docs flow through unchanged via
    /// `function_type.params[i].doc`.
    fn from(function_type: FunctionLuaType) -> Self {
        Self {
            function_type,
            doc: None,
            return_doc: None,
        }
    }
}

impl GlobalTypeMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up the inferred type for a global name.
    pub fn get(&self, name: &[u8]) -> Option<&LuaType> {
        self.types.get(name)
    }

    /// Mark a fully-qualified call path as an event-handler registrar.
    /// The type checker will validate `path(name_lit, lambda)` calls
    /// against the signature looked up via the literal name.
    pub fn declare_event_registrar(&mut self, path: impl Into<Bytes>) {
        self.event_registrars.insert(path.into());
    }

    /// Returns `true` when the given call path was declared as an
    /// event-handler registrar.
    pub fn is_event_registrar(&self, path: &[u8]) -> bool {
        self.event_registrars.contains(path)
    }

    /// Record the typed signature for an event name.  Idempotent;
    /// re-registration overwrites.  Accepts anything convertible
    /// into [`EventHandlerSignature`] -- a bare `FunctionLuaType`
    /// works via `From` for callers that don't capture rustdoc.
    pub fn declare_event_handler_signature(
        &mut self,
        name: impl Into<Bytes>,
        sig: impl Into<EventHandlerSignature>,
    ) {
        self.event_handler_signatures
            .insert(name.into(), sig.into());
    }

    /// Look up the typed signature for an event name.
    pub fn event_handler_signature(&self, name: &[u8]) -> Option<&EventHandlerSignature> {
        self.event_handler_signatures.get(name)
    }
}

/// Infer a [`LuaType`] from a runtime [`Value`].
///
/// Returns `None` when the type cannot be meaningfully inferred (e.g. an
/// empty table with no function entries, or a userdata that returns an
/// opaque named type).  The caller should skip storing a type entry in
/// that case — runtime detection handles those values.
pub fn infer_type_from_value(value: &Value) -> Option<LuaType> {
    match value {
        Value::Nil => Some(LuaType::Nil),
        Value::Boolean(_) => Some(LuaType::Boolean),
        Value::Integer(_) => Some(LuaType::Integer),
        Value::Float(_) => Some(LuaType::Float),
        Value::String(_) => Some(LuaType::String),
        Value::Function(f) => Some(infer_function_type(f.signature())),
        Value::Table(t) => infer_table_type(t),
        Value::Userdata(u) => Some(u.lua_type_info()),
    }
}

/// Resolve `name` against a [`UserdataType`]'s fields and methods.
/// Returned shape matches the inner `Option<Cow<...>>` half of
/// [`LuaType::lookup_known_member`]: `Some(cow)` means the name was
/// found, `None` means the schema is known but the name is absent.
fn lookup_in_userdata<'a>(ud: &UserdataType, name: &[u8]) -> Option<std::borrow::Cow<'a, LuaType>> {
    for f in &ud.fields {
        if matches!(f.kind, FieldKind::Setter) {
            continue;
        }
        if f.name.as_ref() == name {
            // The registry returns `UserdataType` by value, so we
            // cannot borrow into it; clone the field type.
            return Some(std::borrow::Cow::Owned(f.lua_type.clone()));
        }
    }
    for m in &ud.methods {
        if m.name.as_ref() == name {
            return Some(std::borrow::Cow::Owned(method_function_type(
                ud,
                &m.signature,
            )));
        }
    }
    None
}

/// Synthesize a `LuaType::Function` for a userdata method.  The
/// type checker's colon-call site expects `params[0]` to be the
/// implicit self receiver (skipped via `is_method`), so we prepend
/// it here regardless of whether the source signature stored self
/// explicitly.  Bypasses `infer_function_type`'s `skip(arg_offset)`,
/// which would drop the first real parameter for the macro
/// convention (`params = [key, value], arg_offset = 1`).
fn method_function_type(ud: &UserdataType, sig: &FunctionSignature) -> LuaType {
    let mut params: Vec<TypedParam> = Vec::with_capacity(sig.params.len() + 1);
    params.push(TypedParam {
        name: Some(Bytes::from("self")),
        lua_type: LuaType::Named(ud.name.clone()),
        doc: None,
    });
    for p in &sig.params {
        // Drop a duplicate self at the front of the source params
        // when it's already represented (test-fixture convention).
        if params.len() == 1 && p.name.as_ref().map(|n| n.as_ref()) == Some(b"self" as &[u8]) {
            continue;
        }
        let lua_type = p
            .lua_type
            .clone()
            .or_else(|| p.runtime_type.as_ref().map(valuetype_to_luatype))
            .unwrap_or(LuaType::Any);
        params.push(TypedParam::new_with_doc(
            p.name.clone(),
            lua_type,
            p.doc.clone(),
        ));
    }
    let variadic = if sig.variadic {
        Some(Box::new(LuaType::Any))
    } else {
        None
    };
    let returns = sig.lua_returns.clone().unwrap_or_default();
    LuaType::Function(Box::new(FunctionLuaType {
        type_params: sig.type_params.clone(),
        params,
        variadic,
        returns,
        is_method: true,
        inferred_unannotated: false,
    }))
}

/// Build a `LuaType::Function` from a `FunctionSignature`.
fn infer_function_type(sig: &FunctionSignature) -> LuaType {
    let params: Vec<TypedParam> = sig
        .params
        .iter()
        .skip(sig.arg_offset)
        .map(|p| {
            let lua_type = p
                .lua_type
                .clone()
                .or_else(|| p.runtime_type.as_ref().map(valuetype_to_luatype))
                .unwrap_or(LuaType::Any);
            TypedParam::new_with_doc(p.name.clone(), lua_type, p.doc.clone())
        })
        .collect();

    let variadic = if sig.variadic {
        Some(Box::new(LuaType::Any))
    } else {
        None
    };

    let returns = sig.lua_returns.clone().unwrap_or_default();

    LuaType::Function(Box::new(FunctionLuaType {
        type_params: sig.type_params.clone(),
        params,
        variadic,
        returns,
        is_method: sig.arg_offset > 0,
        inferred_unannotated: false,
    }))
}

/// Convert a `ValueType` to the corresponding `LuaType`.
fn valuetype_to_luatype(vt: &ValueType) -> LuaType {
    match vt {
        ValueType::Nil => LuaType::Nil,
        ValueType::Boolean => LuaType::Boolean,
        ValueType::Integer => LuaType::Integer,
        ValueType::Float => LuaType::Float,
        ValueType::Number => LuaType::Number,
        ValueType::String => LuaType::String,
        ValueType::Table => LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: None,
        })),
        ValueType::Function => LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![],
            variadic: Some(Box::new(LuaType::Any)),
            returns: vec![],
            is_method: false,
            inferred_unannotated: false,
        })),
        ValueType::Userdata => LuaType::Any,
        ValueType::UserdataOf(name) => LuaType::named(*name),
        ValueType::Any => LuaType::Any,
    }
}

/// Infer a structural `LuaType::Table` from a runtime `Table` value.
///
/// Walks the table's string-keyed entries.  Each `Function`-valued entry
/// contributes a typed field; non-function entries contribute their
/// inferred type.  Returns `None` if the table has no string-keyed
/// entries (nothing useful to infer).
fn infer_table_type(table: &crate::table::Table) -> Option<LuaType> {
    let mut fields: Vec<TableField> = Vec::new();
    let mut key = Value::Nil;
    loop {
        // `next` can fail if the table has exotic keys, but in practice
        // module tables only have string keys.
        match table.next(&key) {
            Ok(Some((k, v))) => {
                if let Value::String(name) = &k {
                    if let Some(ty) = infer_type_from_value(&v) {
                        fields.push(TableField::new(name.clone(), ty));
                    }
                }
                key = k;
            }
            _ => break,
        }
    }
    if fields.is_empty() {
        return None;
    }
    // Sort fields by name for deterministic output.
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    Some(LuaType::Table(Box::new(TableLuaType {
        fields,
        indexer: None,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Bytes {
        Bytes::from(s.as_bytes())
    }

    // ----- primitives / keywords --------------------------------------

    #[test]
    fn display_primitives() {
        k9::assert_equal!(LuaType::Nil.to_string(), "nil");
        k9::assert_equal!(LuaType::Boolean.to_string(), "boolean");
        k9::assert_equal!(LuaType::Number.to_string(), "number");
        k9::assert_equal!(LuaType::Integer.to_string(), "integer");
        k9::assert_equal!(LuaType::Float.to_string(), "float");
        k9::assert_equal!(LuaType::String.to_string(), "string");
        k9::assert_equal!(LuaType::Any.to_string(), "any");
        k9::assert_equal!(LuaType::Unknown.to_string(), "unknown");
        k9::assert_equal!(LuaType::Never.to_string(), "never");
    }

    // ----- names / type params ----------------------------------------

    #[test]
    fn display_named() {
        k9::assert_equal!(LuaType::named("Foo").to_string(), "Foo");
    }

    #[test]
    fn display_type_param() {
        k9::assert_equal!(LuaType::type_param("T").to_string(), "T");
    }

    #[test]
    fn named_constructor_equivalent_to_variant() {
        k9::assert_equal!(LuaType::named("Foo"), LuaType::Named(Bytes::from("Foo")));
        // String, &[u8], owned Bytes all compose via Into<Bytes>.
        k9::assert_equal!(
            LuaType::named(String::from("Foo")),
            LuaType::Named(Bytes::from("Foo"))
        );
        let raw: &[u8] = b"Foo";
        k9::assert_equal!(LuaType::named(raw), LuaType::Named(Bytes::from("Foo")));
    }

    #[test]
    fn type_param_constructor_equivalent_to_variant() {
        k9::assert_equal!(
            LuaType::type_param("T"),
            LuaType::TypeParam(Bytes::from("T"))
        );
    }

    // ----- literals ---------------------------------------------------

    #[test]
    fn display_string_literal() {
        k9::assert_equal!(LuaType::StringLiteral(n("hello")).to_string(), "\"hello\"");
    }

    #[test]
    fn display_bool_literal() {
        k9::assert_equal!(LuaType::BoolLiteral(true).to_string(), "true");
        k9::assert_equal!(LuaType::BoolLiteral(false).to_string(), "false");
    }

    #[test]
    fn display_number_literal_integral() {
        // Integer-valued floats drop their decimal (Rust f64 Display
        // matches Lua's `%g` behaviour here).
        k9::assert_equal!(LuaType::NumberLiteral(42.0).to_string(), "42");
    }

    #[test]
    fn display_number_literal_fraction() {
        k9::assert_equal!(LuaType::NumberLiteral(3.5).to_string(), "3.5");
    }

    // ----- optional / union / intersection ----------------------------

    #[test]
    fn display_optional_primitive() {
        k9::assert_equal!(
            LuaType::Optional(Box::new(LuaType::Number)).to_string(),
            "number?"
        );
    }

    #[test]
    fn display_union_two() {
        k9::assert_equal!(
            LuaType::Union(vec![LuaType::Number, LuaType::String]).to_string(),
            "number | string"
        );
    }

    #[test]
    fn display_union_three() {
        k9::assert_equal!(
            LuaType::Union(vec![LuaType::Number, LuaType::String, LuaType::Boolean]).to_string(),
            "number | string | boolean"
        );
    }

    #[test]
    fn display_intersection() {
        k9::assert_equal!(
            LuaType::Intersection(vec![LuaType::named("Readable"), LuaType::named("Writable"),])
                .to_string(),
            "Readable & Writable"
        );
    }

    #[test]
    fn display_optional_wrapping_union_parenthesises() {
        // Without parens this would parse as `A | (B?)`, not what we
        // meant.  The parens preserve the original meaning.
        let u = LuaType::Union(vec![LuaType::Number, LuaType::String]);
        k9::assert_equal!(
            LuaType::Optional(Box::new(u)).to_string(),
            "(number | string)?"
        );
    }

    #[test]
    fn display_optional_wrapping_intersection_parenthesises() {
        let i = LuaType::Intersection(vec![LuaType::named("A"), LuaType::named("B")]);
        k9::assert_equal!(LuaType::Optional(Box::new(i)).to_string(), "(A & B)?");
    }

    #[test]
    fn display_union_of_intersections_parenthesises_each() {
        let a_and_b = LuaType::Intersection(vec![LuaType::named("A"), LuaType::named("B")]);
        let c_and_d = LuaType::Intersection(vec![LuaType::named("C"), LuaType::named("D")]);
        k9::assert_equal!(
            LuaType::Union(vec![a_and_b, c_and_d]).to_string(),
            "(A & B) | (C & D)"
        );
    }

    #[test]
    fn display_intersection_of_unions_parenthesises_each() {
        let a_or_b = LuaType::Union(vec![LuaType::named("A"), LuaType::named("B")]);
        let c_or_d = LuaType::Union(vec![LuaType::named("C"), LuaType::named("D")]);
        k9::assert_equal!(
            LuaType::Intersection(vec![a_or_b, c_or_d]).to_string(),
            "(A | B) & (C | D)"
        );
    }

    // ----- generic instantiation --------------------------------------

    #[test]
    fn display_generic_simple() {
        let t = LuaType::Generic {
            base: Box::new(LuaType::named("Array")),
            args: vec![LuaTypeArg::Type(LuaType::Number)],
        };
        k9::assert_equal!(t.to_string(), "Array<number>");
    }

    #[test]
    fn display_generic_multiple_args() {
        let t = LuaType::Generic {
            base: Box::new(LuaType::named("Map")),
            args: vec![
                LuaTypeArg::Type(LuaType::String),
                LuaTypeArg::Type(LuaType::named("User")),
            ],
        };
        k9::assert_equal!(t.to_string(), "Map<string, User>");
    }

    #[test]
    fn display_generic_with_type_pack_arg() {
        let t = LuaType::Generic {
            base: Box::new(LuaType::named("Callback")),
            args: vec![LuaTypeArg::Pack(LuaType::Number)],
        };
        k9::assert_equal!(t.to_string(), "Callback<number...>");
    }

    // ----- variadic / tuple -------------------------------------------

    #[test]
    fn display_variadic() {
        k9::assert_equal!(
            LuaType::Variadic(Box::new(LuaType::Number)).to_string(),
            "...number"
        );
    }

    #[test]
    fn display_tuple_empty() {
        k9::assert_equal!(LuaType::Tuple(vec![]).to_string(), "()");
    }

    #[test]
    fn display_tuple_single() {
        k9::assert_equal!(
            LuaType::Tuple(vec![LuaType::Number]).to_string(),
            "(number)"
        );
    }

    #[test]
    fn display_tuple_multiple() {
        k9::assert_equal!(
            LuaType::Tuple(vec![LuaType::Number, LuaType::String, LuaType::Boolean]).to_string(),
            "(number, string, boolean)"
        );
    }

    // ----- table shapes -----------------------------------------------

    #[test]
    fn display_table_empty() {
        let t = LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: None,
        }));
        k9::assert_equal!(t.to_string(), "{}");
    }

    #[test]
    fn display_table_array_shorthand_number_key() {
        let t = LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: Some((Box::new(LuaType::Number), Box::new(LuaType::String))),
        }));
        k9::assert_equal!(t.to_string(), "{string}");
    }

    #[test]
    fn display_table_array_shorthand_integer_key() {
        // `Integer` keys also collapse to the array shorthand.
        let t = LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: Some((Box::new(LuaType::Integer), Box::new(LuaType::Boolean))),
        }));
        k9::assert_equal!(t.to_string(), "{boolean}");
    }

    #[test]
    fn display_table_map_non_numeric_key() {
        let t = LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: Some((Box::new(LuaType::String), Box::new(LuaType::Number))),
        }));
        k9::assert_equal!(t.to_string(), "{ [string]: number }");
    }

    #[test]
    fn display_table_record() {
        let t = LuaType::Table(Box::new(TableLuaType {
            fields: vec![
                TableField::new("x", LuaType::Number),
                TableField::new("name", LuaType::String),
            ],
            indexer: None,
        }));
        k9::assert_equal!(t.to_string(), "{ x: number, name: string }");
    }

    #[test]
    fn display_table_record_with_indexer() {
        let t = LuaType::Table(Box::new(TableLuaType {
            fields: vec![TableField::new("tag", LuaType::String)],
            indexer: Some((Box::new(LuaType::String), Box::new(LuaType::Any))),
        }));
        k9::assert_equal!(t.to_string(), "{ tag: string, [string]: any }");
    }

    // ----- function types ---------------------------------------------

    fn fn_type_basic() -> LuaType {
        LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![
                TypedParam::new(Some("x"), LuaType::Number),
                TypedParam::new(Some("s"), LuaType::String),
            ],
            variadic: None,
            returns: vec![LuaType::Boolean],
            is_method: false,
            inferred_unannotated: false,
        }))
    }

    #[test]
    fn display_function_named_params_single_return() {
        k9::assert_equal!(
            fn_type_basic().to_string(),
            "(x: number, s: string) -> boolean"
        );
    }

    #[test]
    fn display_function_no_returns() {
        let t = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![TypedParam::new(Some("x"), LuaType::Number)],
            variadic: None,
            returns: vec![],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "(x: number) -> ()");
    }

    #[test]
    fn display_function_multiple_returns() {
        let t = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![],
            variadic: None,
            returns: vec![LuaType::Number, LuaType::String],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "() -> (number, string)");
    }

    #[test]
    fn display_function_unnamed_params() {
        let t = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![
                TypedParam::unnamed(LuaType::Number),
                TypedParam::unnamed(LuaType::String),
            ],
            variadic: None,
            returns: vec![LuaType::Boolean],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "(number, string) -> boolean");
    }

    #[test]
    fn display_function_variadic() {
        let t = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![TypedParam::new(Some("first"), LuaType::Number)],
            variadic: Some(Box::new(LuaType::Any)),
            returns: vec![LuaType::Nil],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "(first: number, ...any) -> nil");
    }

    #[test]
    fn display_function_with_type_params() {
        let t = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![GenericTypeParam {
                name: n("T"),
                default: None,
                is_pack: false,
            }],
            params: vec![TypedParam::new(Some("x"), LuaType::type_param("T"))],
            variadic: None,
            returns: vec![LuaType::type_param("T")],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "<T>(x: T) -> T");
    }

    #[test]
    fn display_function_with_multiple_type_params_and_pack() {
        let t = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![
                GenericTypeParam {
                    name: n("T"),
                    default: None,
                    is_pack: false,
                },
                GenericTypeParam {
                    name: n("U"),
                    default: None,
                    is_pack: true,
                },
            ],
            params: vec![TypedParam::new(Some("x"), LuaType::type_param("T"))],
            variadic: None,
            returns: vec![LuaType::type_param("T")],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "<T, U...>(x: T) -> T");
    }

    // ----- generic type param with default ----------------------------

    #[test]
    fn display_generic_param_with_default() {
        let tp = GenericTypeParam {
            name: n("T"),
            default: Some(LuaType::Number),
            is_pack: false,
        };
        k9::assert_equal!(tp.to_string(), "T = number");
    }

    // ----- module -----------------------------------------------------

    #[test]
    fn display_module() {
        let t = LuaType::Module(Box::new(ModuleType {
            name: n("myutil"),
            doc: None,
            strict: false,
            fields: vec![],
            functions: vec![],
            methods: vec![],
            metamethods: vec![],
        }));
        k9::assert_equal!(t.to_string(), "module<myutil>");
    }

    // ----- nested composites ------------------------------------------

    #[test]
    fn display_array_of_optionals() {
        // `{number?}` - array of optional numbers
        let t = LuaType::Table(Box::new(TableLuaType {
            fields: vec![],
            indexer: Some((
                Box::new(LuaType::Number),
                Box::new(LuaType::Optional(Box::new(LuaType::Number))),
            )),
        }));
        k9::assert_equal!(t.to_string(), "{number?}");
    }

    #[test]
    fn display_callback_returning_optional() {
        let t = LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![TypedParam::new(Some("k"), LuaType::String)],
            variadic: None,
            returns: vec![LuaType::Optional(Box::new(LuaType::named("User")))],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "(k: string) -> User?");
    }

    #[test]
    fn display_generic_of_union() {
        let t = LuaType::Generic {
            base: Box::new(LuaType::named("Result")),
            args: vec![LuaTypeArg::Type(LuaType::Union(vec![
                LuaType::Number,
                LuaType::String,
            ]))],
        };
        k9::assert_equal!(t.to_string(), "Result<number | string>");
    }

    // ----- infer_type_from_value --------------------------------------

    #[test]
    fn infer_nil() {
        k9::assert_equal!(infer_type_from_value(&Value::Nil), Some(LuaType::Nil));
    }

    #[test]
    fn infer_boolean() {
        k9::assert_equal!(
            infer_type_from_value(&Value::Boolean(true)),
            Some(LuaType::Boolean)
        );
    }

    #[test]
    fn infer_integer() {
        k9::assert_equal!(
            infer_type_from_value(&Value::Integer(42)),
            Some(LuaType::Integer)
        );
    }

    #[test]
    fn infer_float() {
        k9::assert_equal!(
            infer_type_from_value(&Value::Float(3.14)),
            Some(LuaType::Float)
        );
    }

    #[test]
    fn infer_string() {
        k9::assert_equal!(
            infer_type_from_value(&Value::string("hello")),
            Some(LuaType::String)
        );
    }

    /// Helper: build the `LuaType::Function` that `infer_type_from_value`
    /// should produce for a `Function::wrap` with the given param types,
    /// variadic flag, and return types.
    fn expected_fn(params: Vec<LuaType>, variadic: bool, returns: Vec<LuaType>) -> LuaType {
        LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: params.into_iter().map(|t| TypedParam::unnamed(t)).collect(),
            variadic: if variadic {
                Some(Box::new(LuaType::Any))
            } else {
                None
            },
            returns,
            is_method: false,
            inferred_unannotated: false,
        }))
    }

    #[test]
    fn infer_function_untyped() {
        use crate::function::Function;
        // |_x: Value| Ok(Value::Nil)  →  param: any, returns: any
        let f = Function::wrap("test_fn", |_x: Value| Ok(Value::Nil));
        k9::assert_equal!(
            infer_type_from_value(&Value::Function(f)),
            Some(expected_fn(vec![LuaType::Any], false, vec![LuaType::Any]))
        );
    }

    #[test]
    fn infer_function_with_params() {
        use crate::function::Function;
        // |a: i64, b: i64| Ok(a + b)  →  params: (number, number), returns: number
        let f = Function::wrap("add", |a: i64, b: i64| Ok(a + b));
        k9::assert_equal!(
            infer_type_from_value(&Value::Function(f)),
            Some(expected_fn(
                vec![LuaType::Number, LuaType::Number],
                false,
                vec![LuaType::Number]
            ))
        );
    }

    #[test]
    fn infer_empty_table_returns_none() {
        use crate::table::Table;
        let t = Table::new();
        k9::assert_equal!(infer_type_from_value(&Value::Table(t)), None);
    }

    #[test]
    fn infer_table_with_function_entries() {
        use crate::function::Function;
        use crate::table::Table;
        let t = Table::new();
        // |_name: Bytes| Ok(Value::string("hi"))  →  param: string, returns: any
        let f = Function::wrap("greet", |_name: Bytes| Ok(Value::string("hi")));
        t.raw_set(Value::string("greet"), Value::Function(f))
            .expect("set");
        k9::assert_equal!(
            infer_type_from_value(&Value::Table(t)),
            Some(LuaType::Table(Box::new(TableLuaType {
                fields: vec![TableField::new(
                    n("greet"),
                    expected_fn(vec![LuaType::String], false, vec![LuaType::Any]),
                )],
                indexer: None,
            })))
        );
    }

    #[test]
    fn infer_table_with_non_string_keys_ignored() {
        use crate::table::Table;
        let t = Table::new();
        // Integer key — should be ignored
        t.raw_set(Value::Integer(1), Value::string("val"))
            .expect("set");
        k9::assert_equal!(infer_type_from_value(&Value::Table(t)), None);
    }

    #[test]
    fn infer_table_fields_sorted() {
        use crate::function::Function;
        use crate::table::Table;
        let t = Table::new();
        // || Ok(Value::Nil)  →  no params, returns: any
        let f1 = Function::wrap("beta", || Ok(Value::Nil));
        let f2 = Function::wrap("alpha", || Ok(Value::Nil));
        t.raw_set(Value::string("beta"), Value::Function(f1))
            .expect("set");
        t.raw_set(Value::string("alpha"), Value::Function(f2))
            .expect("set");
        let no_params_fn = expected_fn(vec![], false, vec![LuaType::Any]);
        k9::assert_equal!(
            infer_type_from_value(&Value::Table(t)),
            Some(LuaType::Table(Box::new(TableLuaType {
                fields: vec![
                    TableField::new("alpha", no_params_fn.clone()),
                    TableField::new("beta", no_params_fn),
                ],
                indexer: None,
            })))
        );
    }

    #[test]
    fn infer_table_with_mixed_values() {
        use crate::table::Table;
        let t = Table::new();
        t.raw_set(Value::string("name"), Value::string("test"))
            .expect("set");
        t.raw_set(Value::string("count"), Value::Integer(5))
            .expect("set");
        k9::assert_equal!(
            infer_type_from_value(&Value::Table(t)),
            Some(LuaType::Table(Box::new(TableLuaType {
                fields: vec![
                    TableField::new("count", LuaType::Integer),
                    TableField::new("name", LuaType::String),
                ],
                indexer: None,
            })))
        );
    }

    // ----- GlobalTypeMap ----------------------------------------------

    #[test]
    fn global_type_map_basic() {
        let mut map = GlobalTypeMap::new();
        map.types.insert(n("x"), LuaType::Integer);
        k9::assert_equal!(map.get(b"x"), Some(&LuaType::Integer));
        k9::assert_equal!(map.get(b"y"), None);
    }
}
