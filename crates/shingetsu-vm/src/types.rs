use std::collections::HashMap;
use std::fmt;

use bstr::BStr;
use bytes::Bytes;

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
}

/// How a field's value is produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// Value is pre-computed at module construction time.
    Eager,
    /// Value is computed by a Rust getter function each time `__index` is called.
    Getter,
    /// Field has a Rust setter function invoked by `__newindex`.
    Setter,
}

/// A free function or method exposed on a module or userdata type.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDef {
    pub name: Bytes,
    pub doc: Option<String>,
    pub signature: FunctionSignature,
}

/// A metamethod exposed on a module or userdata type.
#[derive(Debug, Clone, PartialEq)]
pub struct MetamethodDef {
    pub method: MetaMethod,
    pub doc: Option<String>,
    pub signature: FunctionSignature,
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
    pub fields: Vec<(Bytes, LuaType)>,
    /// Index signature: `{ [K]: V }`.
    pub indexer: Option<(Box<LuaType>, Box<LuaType>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionLuaType {
    pub type_params: Vec<GenericTypeParam>,
    /// Named parameters with type: `(x: number, y: string)`.
    pub params: Vec<(Option<Bytes>, LuaType)>,
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

/// A generic type parameter declaration, e.g. `T`, `T extends Foo`, or `T...`.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericTypeParam {
    pub name: Bytes,
    /// Upper-bound constraint (`T: Foo` in LuaU).
    pub constraint: Option<LuaType>,
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
/// Provided to the [`Compiler`] so that `require` calls can be
/// resolved to typed module exports at compile time.
///
/// Uses interior mutability so the compiler can insert newly-resolved
/// modules during demand-driven `require` resolution while holding
/// only `&self`.
#[derive(Debug, Default)]
pub struct ModuleTypeRegistry {
    modules: parking_lot::Mutex<HashMap<Bytes, ModuleTypeInfo>>,
    /// Module names currently being compiled — used to detect
    /// circular `require` chains and break the cycle.
    in_progress: parking_lot::Mutex<std::collections::HashSet<Bytes>>,
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

    /// Mark a module as currently being compiled.
    /// Returns `false` if it was already in progress (circular require).
    pub fn begin_compile(&self, name: &[u8]) -> bool {
        self.in_progress.lock().insert(Bytes::copy_from_slice(name))
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
impl LuaType {
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

/// let opt_num = LuaType::Optional(Box::new(LuaType::Number));
/// assert_eq!(opt_num.to_string(), "number?");
/// ```
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
        for (i, (name, ty)) in self.fields.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            let name = BStr::new(name);
            write!(f, "{name}: {ty}")?;
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
impl fmt::Display for FunctionLuaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        for (name, ty) in &self.params {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            if let Some(name) = name {
                let name = BStr::new(name);
                write!(f, "{name}: {ty}")?;
            } else {
                write!(f, "{ty}")?;
            }
        }
        if let Some(va) = &self.variadic {
            if !first {
                f.write_str(", ")?;
            }
            write!(f, "...{va}")?;
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

/// Render a generic parameter declaration, with its type-pack marker,
/// constraint, and default clause.
///
/// Shapes produced (in Luau-adjacent syntax):
///
/// * `T` — plain parameter
/// * `T...` — type pack
/// * `T: Foo` — with constraint (Luau accepts `extends`; we render `:`
///   for brevity, matching the field in [`GenericTypeParam`])
/// * `T = number` — with default
impl fmt::Display for GenericTypeParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = BStr::new(&self.name);
        write!(f, "{name}")?;
        if self.is_pack {
            f.write_str("...")?;
        }
        if let Some(c) = &self.constraint {
            write!(f, ": {c}")?;
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
}

impl GlobalTypeMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up the inferred type for a global name.
    pub fn get(&self, name: &[u8]) -> Option<&LuaType> {
        self.types.get(name)
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

/// Build a `LuaType::Function` from a `FunctionSignature`.
fn infer_function_type(sig: &FunctionSignature) -> LuaType {
    let params: Vec<(Option<Bytes>, LuaType)> = sig
        .params
        .iter()
        .skip(sig.arg_offset)
        .map(|p| {
            let ty = p
                .lua_type
                .clone()
                .or_else(|| p.runtime_type.as_ref().map(valuetype_to_luatype))
                .unwrap_or(LuaType::Any);
            (p.name.clone(), ty)
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
        ValueType::UserdataOf(name) => LuaType::Named(Bytes::from_static(name.as_bytes())),
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
    let mut fields: Vec<(Bytes, LuaType)> = Vec::new();
    let mut key = Value::Nil;
    loop {
        // `next` can fail if the table has exotic keys, but in practice
        // module tables only have string keys.
        match table.next(&key) {
            Ok(Some((k, v))) => {
                if let Value::String(name) = &k {
                    if let Some(ty) = infer_type_from_value(&v) {
                        fields.push((name.clone(), ty));
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
    fields.sort_by(|(a, _), (b, _)| a.cmp(b));
    Some(LuaType::Table(Box::new(TableLuaType {
        fields,
        indexer: None,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(s: &str) -> Bytes {
        Bytes::copy_from_slice(s.as_bytes())
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
        k9::assert_equal!(LuaType::Named(n("Foo")).to_string(), "Foo");
    }

    #[test]
    fn display_type_param() {
        k9::assert_equal!(LuaType::TypeParam(n("T")).to_string(), "T");
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
            LuaType::Intersection(vec![
                LuaType::Named(n("Readable")),
                LuaType::Named(n("Writable")),
            ])
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
        let i = LuaType::Intersection(vec![LuaType::Named(n("A")), LuaType::Named(n("B"))]);
        k9::assert_equal!(LuaType::Optional(Box::new(i)).to_string(), "(A & B)?");
    }

    #[test]
    fn display_union_of_intersections_parenthesises_each() {
        let a_and_b = LuaType::Intersection(vec![LuaType::Named(n("A")), LuaType::Named(n("B"))]);
        let c_and_d = LuaType::Intersection(vec![LuaType::Named(n("C")), LuaType::Named(n("D"))]);
        k9::assert_equal!(
            LuaType::Union(vec![a_and_b, c_and_d]).to_string(),
            "(A & B) | (C & D)"
        );
    }

    #[test]
    fn display_intersection_of_unions_parenthesises_each() {
        let a_or_b = LuaType::Union(vec![LuaType::Named(n("A")), LuaType::Named(n("B"))]);
        let c_or_d = LuaType::Union(vec![LuaType::Named(n("C")), LuaType::Named(n("D"))]);
        k9::assert_equal!(
            LuaType::Intersection(vec![a_or_b, c_or_d]).to_string(),
            "(A | B) & (C | D)"
        );
    }

    // ----- generic instantiation --------------------------------------

    #[test]
    fn display_generic_simple() {
        let t = LuaType::Generic {
            base: Box::new(LuaType::Named(n("Array"))),
            args: vec![LuaTypeArg::Type(LuaType::Number)],
        };
        k9::assert_equal!(t.to_string(), "Array<number>");
    }

    #[test]
    fn display_generic_multiple_args() {
        let t = LuaType::Generic {
            base: Box::new(LuaType::Named(n("Map"))),
            args: vec![
                LuaTypeArg::Type(LuaType::String),
                LuaTypeArg::Type(LuaType::Named(n("User"))),
            ],
        };
        k9::assert_equal!(t.to_string(), "Map<string, User>");
    }

    #[test]
    fn display_generic_with_type_pack_arg() {
        let t = LuaType::Generic {
            base: Box::new(LuaType::Named(n("Callback"))),
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
            fields: vec![(n("x"), LuaType::Number), (n("name"), LuaType::String)],
            indexer: None,
        }));
        k9::assert_equal!(t.to_string(), "{ x: number, name: string }");
    }

    #[test]
    fn display_table_record_with_indexer() {
        let t = LuaType::Table(Box::new(TableLuaType {
            fields: vec![(n("tag"), LuaType::String)],
            indexer: Some((Box::new(LuaType::String), Box::new(LuaType::Any))),
        }));
        k9::assert_equal!(t.to_string(), "{ tag: string, [string]: any }");
    }

    // ----- function types ---------------------------------------------

    fn fn_type_basic() -> LuaType {
        LuaType::Function(Box::new(FunctionLuaType {
            type_params: vec![],
            params: vec![
                (Some(n("x")), LuaType::Number),
                (Some(n("s")), LuaType::String),
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
            params: vec![(Some(n("x")), LuaType::Number)],
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
            params: vec![(None, LuaType::Number), (None, LuaType::String)],
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
            params: vec![(Some(n("first")), LuaType::Number)],
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
                constraint: None,
                default: None,
                is_pack: false,
            }],
            params: vec![(Some(n("x")), LuaType::TypeParam(n("T")))],
            variadic: None,
            returns: vec![LuaType::TypeParam(n("T"))],
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
                    constraint: None,
                    default: None,
                    is_pack: false,
                },
                GenericTypeParam {
                    name: n("U"),
                    constraint: None,
                    default: None,
                    is_pack: true,
                },
            ],
            params: vec![(Some(n("x")), LuaType::TypeParam(n("T")))],
            variadic: None,
            returns: vec![LuaType::TypeParam(n("T"))],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "<T, U...>(x: T) -> T");
    }

    // ----- generic type param with constraint / default ---------------

    #[test]
    fn display_generic_param_with_constraint() {
        let tp = GenericTypeParam {
            name: n("T"),
            constraint: Some(LuaType::Named(n("Foo"))),
            default: None,
            is_pack: false,
        };
        k9::assert_equal!(tp.to_string(), "T: Foo");
    }

    #[test]
    fn display_generic_param_with_default() {
        let tp = GenericTypeParam {
            name: n("T"),
            constraint: None,
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
            params: vec![(Some(n("k")), LuaType::String)],
            variadic: None,
            returns: vec![LuaType::Optional(Box::new(LuaType::Named(n("User"))))],
            is_method: false,
            inferred_unannotated: false,
        }));
        k9::assert_equal!(t.to_string(), "(k: string) -> User?");
    }

    #[test]
    fn display_generic_of_union() {
        let t = LuaType::Generic {
            base: Box::new(LuaType::Named(n("Result"))),
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
            params: params.into_iter().map(|t| (None, t)).collect(),
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
        // |a: i64, b: i64| Ok(a + b)  →  params: (integer, integer), returns: integer
        let f = Function::wrap("add", |a: i64, b: i64| Ok(a + b));
        k9::assert_equal!(
            infer_type_from_value(&Value::Function(f)),
            Some(expected_fn(
                vec![LuaType::Integer, LuaType::Integer],
                false,
                vec![LuaType::Integer]
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
                fields: vec![(
                    n("greet"),
                    expected_fn(vec![LuaType::String], false, vec![LuaType::Any])
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
                    (n("alpha"), no_params_fn.clone()),
                    (n("beta"), no_params_fn),
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
                fields: vec![(n("count"), LuaType::Integer), (n("name"), LuaType::String),],
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
