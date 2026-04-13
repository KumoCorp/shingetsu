// Allow proc-macro generated code (which references `::shingetsu::*`) to
// resolve within this crate.
extern crate self as shingetsu;

// Re-export the VM public API so embedders only need to depend on `shingetsu`.
pub use shingetsu_vm::*;

pub mod builtins;
pub mod string_lib;

// Re-export the compiler under a sub-module for advanced users.
pub use shingetsu_compiler as compiler;

// Re-export downcast_rs so proc-macro generated code can reference
// `::shingetsu::downcast_rs::impl_downcast!` without the embedder having
// to add a direct dependency on downcast-rs.
pub use downcast_rs;

// Re-export proc macros so users only need `shingetsu` as a dependency.
pub use shingetsu_derive::{module, userdata, UserData};

// Re-export external crates referenced in proc-macro generated code so that
// generated code can use `::shingetsu::bytes` / `::shingetsu::async_trait`
// without the embedder needing direct dependencies on those crates.
pub use async_trait;
pub use bytes;
