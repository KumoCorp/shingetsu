//! Runtime engine wrapper.
//!
//! [`Engine`] is the migration facade's typed handle to whichever
//! backend the host instantiated.  It is a pure wrapper: it owns
//! a fully-configured `shingetsu::GlobalEnv` or `mlua::Lua` and
//! exposes accessors so engine-coupled host code can reach the
//! underlying state directly.  The cross-engine surface (event
//! dispatch, callback registration) lives in sibling modules and
//! takes `&Engine` by reference.
//!
//! Real hosts (kumomta, wezterm) register a non-trivial set of
//! modules, callbacks, and globals on their backend before any
//! script is loaded.  `Engine` therefore takes a fully-configured
//! engine instance from the caller -- it does *not* construct one
//! for you, nor does it expose a built-in `eval`.  Hosts that need
//! to load user scripts use the engine's native API via
//! `as_shingetsu()` / `as_mlua()`, which preserves access to
//! `CompileOptions` (source_name, type_check), compile-time
//! diagnostics (`Bytecode.diagnostics`, `lint_directives`), and
//! the `Lua::load(...).set_name(...)` chain.  Typical use:
//!
//! ```ignore
//! let env = shingetsu::GlobalEnv::new();
//! shingetsu::builtins::register(&env)?;
//! my_host::register_modules(&env)?;
//! let engine = Engine::from_shingetsu(env);
//!
//! // Later, to load a user script with full diagnostic control:
//! let env = engine.as_shingetsu().expect("shingetsu engine");
//! let opts = CompileOptions { type_check: true, source_name: ..., .. };
//! let bc = Compiler::new(opts, env.global_type_map())
//!     .compile(src).await?;
//! // Surface bc.diagnostics to the user, then run.
//! ```

#![cfg(any(feature = "shingetsu-backend", feature = "mlua-backend"))]

#[cfg(feature = "shingetsu-backend")]
use shingetsu::GlobalEnv;

/// Either-engine wrapper used by hosts that need to choose a
/// backend at runtime during the migration.  Each variant owns the
/// engine state for that backend; engine-coupled host code
/// reaches in via `as_shingetsu()` / `as_mlua()`, while the
/// cross-engine surface (callback dispatch, registered events)
/// takes `&Engine` and dispatches through the variant.
#[non_exhaustive]
pub enum Engine {
    #[cfg(feature = "shingetsu-backend")]
    Shingetsu(GlobalEnv),
    #[cfg(feature = "mlua-backend")]
    Mlua(mlua::Lua),
}

impl Engine {
    /// Wrap a fully-configured `shingetsu::GlobalEnv`.  The caller
    /// is responsible for registering whatever builtins, host
    /// modules, and globals they need before constructing the
    /// `Engine`.
    #[cfg(feature = "shingetsu-backend")]
    pub fn from_shingetsu(env: GlobalEnv) -> Self {
        Self::Shingetsu(env)
    }

    /// Wrap a fully-configured `mlua::Lua`.  The caller is
    /// responsible for registering whatever stdlib subset, host
    /// modules, and globals they need before constructing the
    /// `Engine`.
    #[cfg(feature = "mlua-backend")]
    pub fn from_mlua(lua: mlua::Lua) -> Self {
        Self::Mlua(lua)
    }

    /// Borrow the underlying shingetsu env, if this is a shingetsu
    /// engine.  Returns `None` for the mlua variant.
    #[cfg(feature = "shingetsu-backend")]
    pub fn as_shingetsu(&self) -> Option<&GlobalEnv> {
        match self {
            Self::Shingetsu(env) => Some(env),
            #[cfg(feature = "mlua-backend")]
            _ => None,
        }
    }

    /// Borrow the underlying mlua state, if this is an mlua engine.
    /// Returns `None` for the shingetsu variant.
    #[cfg(feature = "mlua-backend")]
    pub fn as_mlua(&self) -> Option<&mlua::Lua> {
        match self {
            Self::Mlua(lua) => Some(lua),
            #[cfg(feature = "shingetsu-backend")]
            _ => None,
        }
    }

    /// Stable label naming the active backend.  Useful in logging
    /// and test assertions.
    pub fn backend_name(&self) -> &'static str {
        match self {
            #[cfg(feature = "shingetsu-backend")]
            Self::Shingetsu(_) => "shingetsu",
            #[cfg(feature = "mlua-backend")]
            Self::Mlua(_) => "mlua",
        }
    }
}
