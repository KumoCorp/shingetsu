use proc_macro::TokenStream;

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

/// Attribute macro for `impl T { … }` blocks with `#[lua_method]`,
/// `#[lua_field]`, and `#[lua_metamethod]` annotations.
///
/// Generates:
/// - The original `impl T { … }` (annotations stripped).
/// - `impl Userdata for T` with a `dispatch` that routes `__index`,
///   `__newindex`, and named metamethods to the annotated Rust methods.
/// - `impl_downcast!(sync T)`.
/// - `impl LuaTyped for T`.
///
/// ## Annotations
///
/// - `#[lua_method]` / `#[lua_method(rename = "x")]` — exposes the function as
///   a Lua method.  The first Lua argument (the object) is skipped; remaining
///   arguments are extracted via `FromLua`.  Returns a `NativeFunction` from
///   `__index`.
/// - `#[lua_field]` — getter when the function name does **not** start with
///   `set_`; setter otherwise.  Also `#[lua_field(setter)]` or
///   `#[lua_field(rename = "x")]`.
/// - `#[lua_metamethod(Name)]` or `#[lua_metamethod("__name")]` — dispatched
///   when the metamethod matches exactly.
#[proc_macro_attribute]
pub fn userdata(attr: TokenStream, item: TokenStream) -> TokenStream {
    userdata::expand_impl(attr.into(), item.into()).into()
}

/// Attribute macro for `mod name { … }` blocks.
///
/// Generates inside the module:
/// - `pub fn build_module_table(env: &GlobalEnv) -> Result<Table, VmError>`
/// - `pub fn register_global_module(env: &GlobalEnv) -> Result<(), VmError>`
/// - `pub fn register_preload(env: &GlobalEnv)` (stub until Step 3)
///
/// ## Item annotations
///
/// - `#[function]` / `#[function(rename = "x")]` — exposes a free function.
/// - `#[field]` / `#[field(rename = "x")]` — eager field: zero-argument
///   function called once at table construction time.
///
/// ## Module options
///
/// `#[shingetsu::module(name = "lua_name")]` — override the Lua module name
/// (default: the `mod` identifier).
///
/// `#[shingetsu::module(strict)]` — TODO: generates `__index`/`__newindex`
/// guards that raise errors for unknown keys.
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module::expand(attr.into(), item.into()).into()
}

/// Derive `FromLua` for a struct with named fields, converting from a Lua
/// table.  Each field is extracted via `Table::get_field`.
///
/// Also generates `LuaTyped`, returning a `LuaType::Table` with typed fields.
///
/// ## Field attributes
///
/// - `#[lua(rename = "x")]` — use `"x"` as the Lua table key.
/// - `#[lua(default = expr)]` — use `expr` when the field is nil/absent.
#[proc_macro_derive(FromLua, attributes(lua))]
pub fn derive_from_lua(input: TokenStream) -> TokenStream {
    lua_struct::derive_from_lua(input.into()).into()
}

/// Derive `IntoLua` for a struct with named fields, converting to a Lua
/// table.  Each field is inserted via `Table::raw_set`.
///
/// `Option<T>` fields that are `None` are skipped (not inserted as nil).
#[proc_macro_derive(IntoLua, attributes(lua))]
pub fn derive_into_lua(input: TokenStream) -> TokenStream {
    lua_struct::derive_into_lua(input.into()).into()
}
