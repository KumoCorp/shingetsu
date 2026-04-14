use bytes::Bytes;

use crate::meta_method::MetaMethod;

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
