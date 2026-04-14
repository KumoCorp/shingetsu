//! Trait and impls for converting typed Rust closures into [`Function`] values.
//!
//! This provides a runtime equivalent of what the `#[function]` proc macro does
//! at compile time: automatic `FromLua` extraction for parameters, `IntoLuaMulti`
//! conversion for return values, and position-tagged error messages.
//!
//! # Usage
//!
//! ```rust,ignore
//! // Simple typed closure — parameters extracted via FromLua
//! let f = Function::wrap("add", |a: i64, b: i64| Ok(a + b));
//!
//! // With CallContext access
//! let f = Function::wrap("my_func", |ctx: CallContext, s: Bytes| {
//!     // ctx available for nested calls, error context, etc.
//!     Ok(s)
//! });
//! ```

use std::marker::PhantomData;
use std::sync::Arc;

use bytes::Bytes;

use crate::call_context::CallContext;
use crate::convert::{FromLua, IntoLuaMulti, Variadic};
use crate::error::VmError;
use crate::function::{Function, NativeFunction};
use crate::types::FunctionSignature;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Marker types for trait dispatch
// ---------------------------------------------------------------------------

/// Marker for closures that do not receive a [`CallContext`].
pub struct Plain<Args>(PhantomData<Args>);

/// Marker for closures whose first parameter is a [`CallContext`].
pub struct WithCtx<Args>(PhantomData<Args>);

/// Marker for closures with typed params followed by a trailing [`Variadic`].
pub struct PlainVarargs<Args>(PhantomData<Args>);

/// Marker for closures with [`CallContext`], typed params, and trailing [`Variadic`].
pub struct WithCtxVarargs<Args>(PhantomData<Args>);

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// Trait for Rust callables that can be converted into a [`NativeFunction`].
///
/// Implemented for `Fn` closures with typed parameters (`FromLua`) and
/// return values (`IntoLuaMulti`).  The `Marker` type parameter is inferred
/// by the compiler and prevents coherence conflicts between the plain and
/// context-carrying variants.
pub trait IntoNativeFunction<Marker>: Send + Sync + 'static {
    fn into_native_function(self, name: &'static str) -> NativeFunction;
}

impl Function {
    /// Wrap a typed Rust closure as a [`Function`], with automatic
    /// `FromLua` extraction for parameters and `IntoLuaMulti` for returns.
    ///
    /// The closure's parameter types determine how Lua arguments are
    /// extracted.  If the first parameter is [`CallContext`], it receives
    /// the call context and remaining parameters are extracted from Lua
    /// arguments.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// // No context
    /// let add = Function::wrap("add", |a: i64, b: i64| Ok(a + b));
    ///
    /// // With context
    /// let f = Function::wrap("f", |ctx: CallContext, t: Table| {
    ///     Ok(())
    /// });
    /// ```
    pub fn wrap<Marker>(name: &'static str, f: impl IntoNativeFunction<Marker>) -> Function {
        Function::native(f.into_native_function(name))
    }
}

// ---------------------------------------------------------------------------
// Helper: build a FunctionSignature from a static name
// ---------------------------------------------------------------------------

fn make_signature(name: &'static str) -> Arc<FunctionSignature> {
    Arc::new(FunctionSignature {
        name: Bytes::from_static(name.as_bytes()),
        type_params: Vec::new(),
        params: Vec::new(),
        variadic: false,
        arg_offset: 0,
        returns: None,
        lua_returns: None,
    })
}

// ---------------------------------------------------------------------------
// Arity impls — generated via macro
// ---------------------------------------------------------------------------

/// Helper to extract one `FromLua` argument with a position-tagged error.
#[inline]
fn extract_arg<T: FromLua>(
    args: &mut impl Iterator<Item = Value>,
    position: usize,
    name: &'static str,
) -> Result<T, VmError> {
    let v = args.next().unwrap_or(Value::Nil);
    T::from_lua(v).map_err(|e| match e {
        VmError::BadArgument { expected, got, .. } => VmError::BadArgument {
            position,
            function: name.to_owned(),
            expected,
            got,
        },
        other => other,
    })
}

macro_rules! impl_into_native_fn {
    // Base case: zero args
    () => {
        // No context, no args
        impl<R, Func> IntoNativeFunction<Plain<()>> for Func
        where
            R: IntoLuaMulti + Send + 'static,
            Func: Fn() -> Result<R, VmError> + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name);
                NativeFunction {
                    signature: sig,
                    call: Arc::new(move |_ctx, _args| {
                        let result = self();
                        Box::pin(async move {
                            result.map(|r| r.into_lua_multi())
                        })
                    }),
                }
            }
        }

        // With context, no args
        impl<R, Func> IntoNativeFunction<WithCtx<()>> for Func
        where
            R: IntoLuaMulti + Send + 'static,
            Func: Fn(CallContext) -> Result<R, VmError> + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name);
                NativeFunction {
                    signature: sig,
                    call: Arc::new(move |ctx, _args| {
                        let result = self(ctx);
                        Box::pin(async move {
                            result.map(|r| r.into_lua_multi())
                        })
                    }),
                }
            }
        }

        // No context, variadic only
        impl<R, Func> IntoNativeFunction<PlainVarargs<()>> for Func
        where
            R: IntoLuaMulti + Send + 'static,
            Func: Fn(Variadic) -> Result<R, VmError> + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let mut sig = make_signature(name);
                Arc::get_mut(&mut sig).expect("freshly created").variadic = true;
                NativeFunction {
                    signature: sig,
                    call: Arc::new(move |_ctx, args| {
                        let result = self(Variadic(args));
                        Box::pin(async move { result.map(|r| r.into_lua_multi()) })
                    }),
                }
            }
        }

        // With context, variadic only
        impl<R, Func> IntoNativeFunction<WithCtxVarargs<()>> for Func
        where
            R: IntoLuaMulti + Send + 'static,
            Func: Fn(CallContext, Variadic) -> Result<R, VmError> + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let mut sig = make_signature(name);
                Arc::get_mut(&mut sig).expect("freshly created").variadic = true;
                NativeFunction {
                    signature: sig,
                    call: Arc::new(move |ctx, args| {
                        let result = self(ctx, Variadic(args));
                        Box::pin(async move { result.map(|r| r.into_lua_multi()) })
                    }),
                }
            }
        }
    };

    // Recursive case: one or more typed args
    ($($T:ident),+) => {
        // No context
        impl<$($T,)* R, Func> IntoNativeFunction<Plain<($($T,)*)>> for Func
        where
            $($T: FromLua,)*
            R: IntoLuaMulti + Send + 'static,
            Func: Fn($($T,)*) -> Result<R, VmError> + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name);
                NativeFunction {
                    signature: sig,
                    call: Arc::new(move |_ctx, args| {
                        let mut __iter = args.into_iter();
                        let mut __pos: usize = 0;
                        let result = (|| -> Result<R, VmError> {
                            $(
                                __pos += 1;
                                let $T = extract_arg::<$T>(&mut __iter, __pos, name)?;
                            )*
                            self($($T,)*)
                        })();
                        Box::pin(async move {
                            result.map(|r| r.into_lua_multi())
                        })
                    }),
                }
            }
        }

        // With context
        impl<$($T,)* R, Func> IntoNativeFunction<WithCtx<($($T,)*)>> for Func
        where
            $($T: FromLua,)*
            R: IntoLuaMulti + Send + 'static,
            Func: Fn(CallContext, $($T,)*) -> Result<R, VmError> + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name);
                NativeFunction {
                    signature: sig,
                    call: Arc::new(move |ctx, args| {
                        let mut __iter = args.into_iter();
                        let mut __pos: usize = 0;
                        let result = (|| -> Result<R, VmError> {
                            $(
                                __pos += 1;
                                let $T = extract_arg::<$T>(&mut __iter, __pos, name)?;
                            )*
                            self(ctx, $($T,)*)
                        })();
                        Box::pin(async move {
                            result.map(|r| r.into_lua_multi())
                        })
                    }),
                }
            }
        }

        // Typed args + trailing Variadic, no context
        impl<$($T,)* R, Func> IntoNativeFunction<PlainVarargs<($($T,)*)>> for Func
        where
            $($T: FromLua,)*
            R: IntoLuaMulti + Send + 'static,
            Func: Fn($($T,)* Variadic) -> Result<R, VmError> + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let mut sig = make_signature(name);
                Arc::get_mut(&mut sig).expect("freshly created").variadic = true;
                NativeFunction {
                    signature: sig,
                    call: Arc::new(move |_ctx, args| {
                        let mut __iter = args.into_iter();
                        let mut __pos: usize = 0;
                        let result = (|| -> Result<R, VmError> {
                            $(
                                __pos += 1;
                                let $T = extract_arg::<$T>(&mut __iter, __pos, name)?;
                            )*
                            let __variadic = Variadic(__iter.collect());
                            self($($T,)* __variadic)
                        })();
                        Box::pin(async move {
                            result.map(|r| r.into_lua_multi())
                        })
                    }),
                }
            }
        }

        // Typed args + trailing Variadic, with context
        impl<$($T,)* R, Func> IntoNativeFunction<WithCtxVarargs<($($T,)*)>> for Func
        where
            $($T: FromLua,)*
            R: IntoLuaMulti + Send + 'static,
            Func: Fn(CallContext, $($T,)* Variadic) -> Result<R, VmError> + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let mut sig = make_signature(name);
                Arc::get_mut(&mut sig).expect("freshly created").variadic = true;
                NativeFunction {
                    signature: sig,
                    call: Arc::new(move |ctx, args| {
                        let mut __iter = args.into_iter();
                        let mut __pos: usize = 0;
                        let result = (|| -> Result<R, VmError> {
                            $(
                                __pos += 1;
                                let $T = extract_arg::<$T>(&mut __iter, __pos, name)?;
                            )*
                            let __variadic = Variadic(__iter.collect());
                            self(ctx, $($T,)* __variadic)
                        })();
                        Box::pin(async move {
                            result.map(|r| r.into_lua_multi())
                        })
                    }),
                }
            }
        }
    };
}

impl_into_native_fn!();
impl_into_native_fn!(A);
impl_into_native_fn!(A, B);
impl_into_native_fn!(A, B, C);
impl_into_native_fn!(A, B, C, D);
impl_into_native_fn!(A, B, C, D, E);
impl_into_native_fn!(A, B, C, D, E, F);
impl_into_native_fn!(A, B, C, D, E, F, G);
impl_into_native_fn!(A, B, C, D, E, F, G, H);
impl_into_native_fn!(A, B, C, D, E, F, G, H, I);
impl_into_native_fn!(A, B, C, D, E, F, G, H, I, J);
impl_into_native_fn!(A, B, C, D, E, F, G, H, I, J, K);
impl_into_native_fn!(A, B, C, D, E, F, G, H, I, J, K, L);
impl_into_native_fn!(A, B, C, D, E, F, G, H, I, J, K, L, M);
impl_into_native_fn!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
impl_into_native_fn!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
impl_into_native_fn!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);
