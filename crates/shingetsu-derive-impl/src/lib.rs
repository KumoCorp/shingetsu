//! Shared codegen library for shingetsu's derive macros.  The
//! proc-macro entry points in `shingetsu-derive` are thin wrappers
//! over the functions exported here; `shingetsu-migrate-derive`
//! reuses the same functions to emit the shingetsu side of its
//! facade derives.
//!
//! Each function takes and returns a `proc_macro2::TokenStream`,
//! intentionally avoiding `proc_macro::TokenStream` so the crate is
//! reachable from non-proc-macro callers.

pub mod facade;
pub mod lua_enum;
pub mod lua_enum_mlua;
pub mod lua_struct;
pub mod lua_struct_mlua;
pub mod module;
pub mod userdata;
pub mod util;
