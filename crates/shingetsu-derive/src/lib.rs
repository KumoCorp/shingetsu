//! Proc-macro entry points for shingetsu's derive surface.  The
//! actual codegen lives in `shingetsu-derive-impl`; this crate is a
//! thin wrapper so the codegen is reachable from non-proc-macro
//! consumers (notably the migration facade in
//! `shingetsu-migrate-derive`).

use proc_macro::TokenStream;
use shingetsu_derive_impl::{lua_enum, lua_struct, module, userdata};

/// Derive macro for simple `Userdata` types with no annotated methods.
///
/// Generates:
/// - `impl Userdata for T` with the default (error-returning) dispatch and a
///   `type_name` derived from the struct name.
/// - `impl_downcast!(sync T)` so `Arc<T>` is recoverable from `Arc<dyn Userdata>`.
/// - `impl LuaTyped for T` returning `LuaType::Named`.
#[proc_macro_derive(UserData, attributes(lua))]
pub fn derive_userdata(input: TokenStream) -> TokenStream {
    userdata::derive(input.into()).into()
}

/// Attribute macro for `impl T { ... }` blocks with `#[lua_method]`,
/// `#[lua_field]`, and `#[lua_metamethod]` annotations.
///
/// Generates:
/// - The original `impl T { ... }` (annotations stripped).
/// - `impl Userdata for T` with a `dispatch` that routes `__index`,
///   `__newindex`, and named metamethods to the annotated Rust methods.
/// - `impl_downcast!(sync T)`.
/// - `impl LuaTyped for T`.
///
/// ## Annotations
///
/// - `#[lua_method]` / `#[lua_method(rename = "x")]` /
///   `#[lua_method(variadic)]` - exposes the function as a Lua method.
///   The first Lua argument (the object) is skipped; remaining
///   arguments are extracted via `FromLua`.  With `variadic`, the
///   last typed parameter is decoded via `FromLuaMulti` instead
///   (e.g. an enum derived with `FromLuaMulti` for arity overloading).
///   Returns a `NativeFunction` from `__index`.
/// - `#[lua_field]` - getter when the function name does **not** start with
///   `set_`; setter otherwise.  Also `#[lua_field(setter)]` or
///   `#[lua_field(rename = "x")]`.
/// - `#[lua_metamethod(Name)]` or `#[lua_metamethod("__name")]` - dispatched
///   when the metamethod matches exactly.
///
/// ## Binary metamethods and `BinOpSide`
///
/// Binary metamethods (`__add`, `__sub`, `__lt`, etc.) may be invoked with
/// the userdata on either side of the operator — for example, both `obj + 3`
/// and `3 + obj` dispatch to `obj`'s `__add`.  The macro identifies `self`
/// by pointer identity regardless of argument position.
///
/// For **commutative** operations (add, mul, bitwise and/or/xor), a plain
/// parameter works fine since operand order doesn't matter:
///
/// ```rust,ignore
/// #[lua_metamethod(Add)]
/// fn add_mm(&self, rhs: i64) -> i64 {
///     self.0 + rhs
/// }
/// ```
///
/// For **non-commutative** operations (sub, div, mod, comparisons, etc.),
/// use `BinOpSide<T>` to receive the other
/// operand with its position.  Convenience methods like
/// `BinOpSide::impl_sub` and
/// `BinOpSide::impl_lt` delegate to the
/// corresponding `std::ops` trait with correct operand ordering:
///
/// ```rust,ignore
/// #[lua_metamethod(Sub)]
/// fn sub_mm(&self, other: BinOpSide<i64>) -> i64 {
///     other.impl_sub(self.0)
/// }
///
/// #[lua_metamethod(Lt)]
/// fn lt_mm(&self, other: BinOpSide<i64>) -> bool {
///     other.impl_lt(self.0)
/// }
/// ```
///
/// See `BinOpSide` for the full API including
/// `apply` and `into_inner`.
#[proc_macro_attribute]
pub fn userdata(attr: TokenStream, item: TokenStream) -> TokenStream {
    userdata::expand_impl(attr.into(), item.into()).into()
}

/// Attribute macro for `mod name { ... }` blocks.
///
/// Generates inside the module:
/// - `pub fn build_module_table(env: &GlobalEnv) -> Result<Table, VmError>`
/// - `pub fn register_global_module(env: &GlobalEnv) -> Result<(), VmError>`
/// - `pub fn register_preload(env: &GlobalEnv)` (stub until Step 3)
///
/// ## Item annotations
///
/// - `#[function]` / `#[function(rename = "x")]` /
///   `#[function(variadic)]` - exposes a free function.  Free
///   functions may declare a `CallContext` or `GlobalEnv` parameter
///   (the shingetsu analog of mlua's `&Lua`); these are auto-injected
///   from the active call site and are not visible to lua callers.
/// - `#[field]` / `#[field(rename = "x")]` - eager field: zero-argument
///   function called once at table construction time.
/// - `#[lazy_field]` / `#[lazy_field(rename = "x")]` - read-only accessor:
///   the function is called on every Lua read of the field.  Use this
///   spelling when there is no paired setter: the lua name defaults
///   to the unmodified fn ident.
/// - `#[getter("name")]` / `#[setter("name")]` - paired read/write
///   accessors.  When both are present for the same lua name, the
///   field is read-write; either may appear alone for read-only or
///   write-only.  A bare `#[getter]` or `#[setter]` strips the
///   `get_` / `set_` prefix from the function ident as the lua name
///   (matching the conventional `fn get_x` / `fn set_x` pattern).
///   The setter must accept exactly one argument.
///
/// `#[lazy_field]` and a solo `#[getter]` produce identical runtime
/// behavior; pick whichever spelling reads more clearly at the call
/// site (`#[lazy_field]` for value-shaped reads, `#[getter]` when
/// pairing with `#[setter]`).
///
/// ## Module options
///
/// `#[shingetsu::module(name = "lua_name")]` - override the Lua module name
/// (default: the `mod` identifier).
///
/// `#[shingetsu::module(strict)]` - TODO: generates `__index`/`__newindex`
/// guards that raise errors for unknown keys.
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module::expand(attr.into(), item.into()).into()
}

/// Derive `FromLua` for structs and enums.
///
/// See the shared [Attribute reference](#attribute-reference) below for
/// the full set of `#[lua(...)]` annotations accepted on the container,
/// fields, and variants.
///
/// ## Structs
///
/// Converts from a Lua table.  Each field is extracted via
/// `Table::get_field`.
///
/// ### Extra fields are ignored
///
/// Tables passed to the generated `FromLua` may contain fields beyond
/// those declared in the struct — they are silently ignored.  This
/// matches Luau's structural (width-subtyping) type system, where a
/// table with extra fields is a valid subtype of one with fewer
/// fields.  It also preserves common Lua idioms such as
/// `os.time(os.date("*t", ts))`, where `os.date` returns fields
/// (`wday`, `yday`, `isdst`) that `os.time` does not consume.  Set
/// `#[lua(deny_unknown_fields)]` on the container to opt out.
///
/// ## Enums
///
/// Two enum shapes are supported:
///
/// 1. **Unit-string enums** — every variant is a unit (data-less)
///    variant.  The Lua value is a string matching the variant name
///    (or `#[lua(rename = "...")]`, or the container's
///    `#[lua(rename_all = "...")]` casing).
/// 2. **Newtype enums** — every variant is a single-field tuple
///    variant.  The container `#[lua(...)]` attribute picks one of
///    three tagging modes:
///    - **Untagged** (default, or explicit `#[lua(untagged)]`): each
///      variant's inner `FromLua` is tried in discriminant-priority
///      order — narrower types first (e.g. `i64` before `f64`).
///      Variants with identical or ambiguously overlapping accepted
///      types produce a compile error.
///    - **Internally tagged** (`#[lua(tag = "kind")]`): the table
///      carries the variant name in the named field; remaining fields
///      come from the inner type's `FromLua`.  The inner type must
///      produce a Table from `IntoLua`.
///    - **Adjacently tagged** (`#[lua(tag = "kind", content = "data")]`):
///      the table is `{ kind = "VariantName", data = inner_value }`;
///      the inner type can be anything.
///
/// Use `derive(LuaTyped)` — or `derive(LuaRepr)` for everything at
/// once — to also generate type metadata for the type-checker.
///
/// # Attribute reference
///
/// Every annotation lives inside `#[lua(...)]`.  Multiple keys can be
/// combined in a single attribute (`#[lua(rename = "x", default)]`).
///
/// ## Container attributes (structs)
///
/// - `try_from = "T"` — decode the Lua value as `T`, then convert via
///   `Self::try_from(T)`.  Symmetric `IntoLua` uses `Into<T>`.
/// - `into = "T"` — `IntoLua` only: convert to `T` before emitting.
/// - `default` / `default = "path::to::fn"` — if the Lua value is
///   `nil`, build the whole struct from `Default::default()` (bare
///   flag) or from the named zero-argument function.
/// - `deny_unknown_fields` — reject tables containing keys that are
///   not declared on the struct.  Incompatible with container
///   `try_from` / `into`.
/// - `rename_all = "casing"` — default case-convert field names
///   that don't carry an explicit `#[lua(rename = "...")]`.  Accepts
///   the same values as the enum form (`"kebab-case"`,
///   `"snake_case"`, etc.).  Does not reach into `#[lua(flatten)]`
///   fields — the flattened type's own attributes still own its
///   keys.  Incompatible with container `try_from` / `into`.
///
/// ## Container attributes (enums)
///
/// - `untagged` — explicitly select the untagged newtype mode
///   (this is also the default when no `tag` is set).
/// - `tag = "kind"` — internally tagged: the variant name lives in
///   the named table field.
/// - `tag = "kind", content = "data"` — adjacently tagged: the
///   variant name lives in `tag` and the inner value in `content`.
///   `content` requires `tag`; `untagged` is incompatible with `tag`.
/// - `rename_all = "casing"` — default case-convert variant names.
///   Accepts the serde values: `"lowercase"`, `"UPPERCASE"`,
///   `"PascalCase"`, `"camelCase"`, `"snake_case"`,
///   `"SCREAMING_SNAKE_CASE"`, `"kebab-case"`,
///   `"SCREAMING-KEBAB-CASE"`.  Per-variant `#[lua(rename = "...")]`
///   overrides this default for that variant.
///
/// ## Field attributes (structs)
///
/// - `rename = "x"` — use `"x"` as the Lua table key (default: the
///   Rust field ident).
/// - `default` / `default = expr` — on `FromLua`, when the key is
///   nil/absent fall back to `T::default()` (bare flag) or to `expr`.
/// - `skip` — omit the field from `FromLua`, `IntoLua`, and
///   `LuaTyped`.  `FromLua` fills it with `T::default()`.
///   Incompatible with `flatten` / `try_from` / `into`.
/// - `flatten` — inline the inner struct's fields at this level (the
///   field's own type must itself be a struct with `FromLua` /
///   `IntoLua` / `LuaTyped`).  Incompatible with `rename`,
///   `try_from`, and `into`.
/// - `try_from = "T"` — read as `T` then convert via
///   `<FieldType as TryFrom<T>>::try_from`.  Symmetric `IntoLua`
///   uses `Into<T>`.
/// - `into = "T"` — `IntoLua` only: convert via `Into<T>` before
///   writing to the Lua table.
/// - `deprecated = "reason"` — record a deprecation reason in the
///   field metadata for the type-checker lint.
/// - `validate = "path::to::fn"` — after `FromLua` extraction, call
///   `fn(&T) -> Result<(), impl Display>` to validate the value.
///
/// ## Variant attributes (enums)
///
/// - `rename = "x"` — use `"x"` as the variant's Lua-facing name
///   (the string for unit-string enums, the tag value for tagged
///   newtype enums).  Overrides any container `rename_all`.
/// - `nil` — on a unit variant inside an otherwise newtype enum,
///   map this variant to/from Lua `nil`.  Only meaningful for the
///   `IntoLua` and `LuaTyped` derives.
#[proc_macro_derive(FromLua, attributes(lua))]
pub fn derive_from_lua(input: TokenStream) -> TokenStream {
    lua_struct::derive_from_lua(input.into()).into()
}

/// Derive `IntoLua` for structs and enums.
///
/// See [`FromLua`'s attribute reference](macro@FromLua#attribute-reference)
/// for the full set of `#[lua(...)]` annotations — they are shared
/// across all four derives in this family.
///
/// ## Structs
///
/// Converts to a Lua table.  Each field is inserted via
/// `Table::raw_set`.  `Option<T>` fields that are `None` are skipped
/// rather than inserted as `nil`.
///
/// ## Enums
///
/// - Unit-string enums emit the variant's Lua-facing name as a
///   string (respecting `#[lua(rename = "...")]` and the container
///   `#[lua(rename_all = "...")]`).
/// - Newtype enums delegate to the inner type's `IntoLua`, wrapping
///   in a tag table for `#[lua(tag = ...)]` and
///   `#[lua(tag, content)]` modes.
/// - A `#[lua(nil)]` unit variant emits Lua `nil`.
///
/// Use `derive(LuaTyped)` — or `derive(LuaRepr)` for everything at
/// once — to also generate type metadata for the type-checker.
#[proc_macro_derive(IntoLua, attributes(lua))]
pub fn derive_into_lua(input: TokenStream) -> TokenStream {
    lua_struct::derive_into_lua(input.into()).into()
}

/// Derive `LuaTyped` for structs and enums.
///
/// Produces the type description consumed by the type-checker and
/// docgen.  See
/// [`FromLua`'s attribute reference](macro@FromLua#attribute-reference)
/// for the shared `#[lua(...)]` annotations.
///
/// - **Structs** produce `LuaType::Table` with typed fields matching
///   the struct's named fields.  `Option<T>` and
///   `#[lua(default = ...)]` fields are wrapped in
///   `LuaType::Optional`.  `#[lua(flatten)]` fields are inlined.
///   `#[lua(skip)]` fields are omitted.
/// - **Unit-string enums** produce `LuaType::String`.
/// - **Newtype enums** produce a `LuaType::Union` of each variant's
///   inner type.  `#[lua(nil)]` unit variants contribute
///   `LuaType::Nil` to the union.  Tagged modes produce the
///   appropriate tagged-table shape.
///
/// This derive is included automatically by `derive(LuaRepr)`.
#[proc_macro_derive(LuaTyped, attributes(lua))]
pub fn derive_lua_typed(input: TokenStream) -> TokenStream {
    lua_struct::derive_lua_typed(input.into()).into()
}

/// Derive `FromLua`, `IntoLua`, and `LuaTyped` for structs and enums.
///
/// Convenience macro equivalent to
/// `#[derive(FromLua, IntoLua, LuaTyped)]`.  See
/// [`FromLua`'s attribute reference](macro@FromLua#attribute-reference)
/// for the full set of `#[lua(...)]` annotations.
#[proc_macro_derive(LuaRepr, attributes(lua))]
pub fn derive_lua_table(input: TokenStream) -> TokenStream {
    lua_struct::derive_lua_table(input.into()).into()
}

/// Derive `IntoLuaMulti` for enums with polymorphic multi-return shapes.
///
/// Each variant's fields are expanded positionally into a `Vec<Value>`
/// via `IntoLua::into_lua`.  Supports:
///
/// - **Unit variants** - produce `vec![Value::Nil]`.
/// - **Newtype variants** (single field) - produce `vec![field.into_lua()]`.
/// - **Tuple variants** (multiple fields) - each field is pushed via
///   `IntoLua::into_lua`.  If the last field is `Variadic`, it is
///   extended rather than pushed as a single element.
///
/// ```rust,ignore
/// #[derive(IntoLuaMulti)]
/// enum FindResult {
///     Match(i64, i64),
///     MatchCaptures(i64, i64, Variadic),
///     NotFound,  // → nil
/// }
/// ```
#[proc_macro_derive(IntoLuaMulti)]
pub fn derive_into_lua_multi(input: TokenStream) -> TokenStream {
    lua_enum::derive_enum_into_lua_multi(input.into()).into()
}

/// Derive `FromLuaMulti` for enums with overloaded argument arities.
///
/// Each variant's field count determines the accepted argument count.
/// Variants are tried from longest to shortest.  All field types must
/// implement `FromLua`.
///
/// ```rust,ignore
/// #[derive(FromLuaMulti)]
/// enum InsertArgs {
///     AtPos(Table, i64, Value),  // 3 args
///     Append(Table, Value),      // 2 args
/// }
/// ```
#[proc_macro_derive(FromLuaMulti)]
pub fn derive_from_lua_multi(input: TokenStream) -> TokenStream {
    lua_enum::derive_enum_from_lua_multi(input.into()).into()
}
