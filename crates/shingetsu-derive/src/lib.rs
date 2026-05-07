use proc_macro::TokenStream;

mod lua_enum;
mod lua_struct;
mod module;
mod userdata;
mod util;

/// Derive macro for simple `Userdata` types with no annotated methods.
///
/// Generates:
/// - `impl Userdata for T` with the default (error-returning) dispatch and a
///   `type_name` derived from the struct name.
/// - `impl_downcast!(sync T)` so `Arc<T>` is recoverable from `Arc<dyn Userdata>`.
/// - `impl LuaTyped for T` returning `LuaType::Named`.
#[proc_macro_derive(UserData)]
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
/// ## Structs
///
/// Converts from a Lua table.  Each field is extracted via `Table::get_field`.
///
/// ### Extra fields are ignored
///
/// Tables passed to the generated `FromLua` may contain fields beyond those
/// declared in the struct — they are silently ignored.  This matches LuaU’s
/// structural (width-subtyping) type system, where a table with extra fields
/// is a valid subtype of one with fewer fields.  It also preserves common Lua
/// idioms such as `os.time(os.date("*t", ts))`, where `os.date` returns
/// fields (`wday`, `yday`, `isdst`) that `os.time` does not consume.
///
/// ### Field attributes
///
/// - `#[lua(rename = "x")]` — use `"x"` as the Lua table key.
/// - `#[lua(default = expr)]` — use `expr` when the field is nil/absent.
///
/// ## Enums
///
/// Each variant must be a newtype (single unnamed field).
///
/// **Tagging modes** (set via container `#[lua(...)]` attribute):
///
/// - **Untagged** (default, or explicit `#[lua(untagged)]`): the
///   generated `FromLua` tries each variant's inner `FromLua` in
///   discriminant-priority order — narrower types are tried first
///   (e.g. `i64` before `f64`).  Variants with identical or
///   ambiguously overlapping accepted types produce a compile error.
/// - **Internally tagged** (`#[lua(tag = "kind")]`): the lua table
///   carries the variant name in the named field; remaining fields
///   come from the inner type's `FromLua`/`IntoLua`.  The inner type
///   must produce a Table from `IntoLua`.
/// - **Adjacently tagged** (`#[lua(tag = "kind", content = "data")]`):
///   the lua table is `{ kind = "VariantName", data = inner_value }`;
///   the inner type can be anything.
///
/// Variant names default to the Rust ident; override with
/// `#[lua(rename = "...")]` on individual variants.
///
/// Use `derive(LuaTyped)` (or `derive(LuaTable)` for structs) to also
/// generate type metadata.
#[proc_macro_derive(FromLua, attributes(lua))]
pub fn derive_from_lua(input: TokenStream) -> TokenStream {
    lua_struct::derive_from_lua(input.into()).into()
}

/// Derive `IntoLua` for structs and enums.
///
/// For structs: converts to a Lua table.  Each field is inserted via
/// `Table::raw_set`.  `Option<T>` fields that are `None` are skipped
/// (not inserted as nil).
///
/// For enums: each variant must be a newtype (single unnamed field).
/// Delegates to the inner type's `IntoLua`.
///
/// Use `derive(LuaTyped)` (or `derive(LuaTable)` for structs) to also
/// generate type metadata.
#[proc_macro_derive(IntoLua, attributes(lua))]
pub fn derive_into_lua(input: TokenStream) -> TokenStream {
    lua_struct::derive_into_lua(input.into()).into()
}

/// Derive `LuaTyped` for structs and enums.
///
/// For structs: produces `LuaType::Table` with typed fields matching the
/// struct's named fields.  `Option<T>` and `#[lua(default = ...)]` fields
/// are wrapped in `LuaType::Optional`.
///
/// For enums: produces `LuaType::Union` of each variant's inner type.
///
/// This derive is included automatically by `derive(LuaTable)`.
#[proc_macro_derive(LuaTyped, attributes(lua))]
pub fn derive_lua_typed(input: TokenStream) -> TokenStream {
    lua_struct::derive_lua_typed(input.into()).into()
}

/// Derive `FromLua`, `IntoLua`, and `LuaTyped` for structs.
///
/// Convenience macro equivalent to `#[derive(FromLua, IntoLua, LuaTyped)]`.
#[proc_macro_derive(LuaTable, attributes(lua))]
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
