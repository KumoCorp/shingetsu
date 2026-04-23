//! Trait and impls for converting typed Rust closures into [`Function`] values.
//!
//! This provides a runtime equivalent of what the `#[function]` proc macro does
//! at compile time: automatic `FromLua` extraction for parameters, `IntoLuaMulti`
//! conversion for return values, and position-tagged error messages.
//!
//! # Usage
//!
//! ```
//! use crate::byte_string::Bytes;
//! use shingetsu_vm::{CallContext, Function, VmError};
//!
//! // Simple typed closure — parameters extracted via FromLua.
//! let _add = Function::wrap("add", |a: i64, b: i64| -> Result<i64, VmError> {
//!     Ok(a + b)
//! });
//!
//! // With CallContext access.
//! let _echo = Function::wrap(
//!     "echo",
//!     |_ctx: CallContext, s: Bytes| -> Result<Bytes, VmError> { Ok(s) },
//! );
//!
//! // Async closures.
//! let _fetch = Function::wrap("fetch", async |url: Bytes| -> Result<Bytes, VmError> {
//!     Ok(url)
//! });
//! ```

use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::byte_string::Bytes;

use crate::call_context::CallContext;
use crate::convert::{FromLua, IntoLuaMulti, LuaTyped, LuaTypedMulti, Variadic};
use crate::error::VmError;
use crate::function::{Function, NativeCall, NativeFunction};
use crate::types::{FunctionSignature, LuaType, ParamSpec};
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

/// Marker for async closures that do not receive a [`CallContext`].
pub struct AsyncPlain<Args>(PhantomData<Args>);

/// Marker for async closures whose first parameter is a [`CallContext`].
pub struct AsyncWithCtx<Args>(PhantomData<Args>);

/// Marker for async closures with typed params followed by a trailing [`Variadic`].
pub struct AsyncPlainVarargs<Args>(PhantomData<Args>);

/// Marker for async closures with [`CallContext`], typed params, and trailing [`Variadic`].
pub struct AsyncWithCtxVarargs<Args>(PhantomData<Args>);

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
    /// ```
    /// use shingetsu_vm::{CallContext, Function, Table, VmError};
    ///
    /// // No context.
    /// let _add = Function::wrap("add", |a: i64, b: i64| -> Result<i64, VmError> {
    ///     Ok(a + b)
    /// });
    ///
    /// // With context.
    /// let _f = Function::wrap(
    ///     "f",
    ///     |_ctx: CallContext, _t: Table| -> Result<(), VmError> { Ok(()) },
    /// );
    /// ```
    pub fn wrap<Marker>(name: &'static str, f: impl IntoNativeFunction<Marker>) -> Function {
        Function::native(f.into_native_function(name))
    }

    /// Wrap a Rust [`Iterator`] as a stateful Lua iterator function.
    ///
    /// The returned `Function` ignores its arguments and calls
    /// `iter.next()` on each invocation.  Items are converted via
    /// [`IntoIterResult`] (supports both `T: IntoLuaMulti` and
    /// `Result<T, VmError>`).  Returns `nil` when the iterator is
    /// exhausted.
    ///
    /// # Examples
    ///
    /// ```
    /// use shingetsu_vm::Function;
    ///
    /// let iter = vec![1i64, 2, 3].into_iter();
    /// let _f = Function::from_iter("count", iter);
    /// // In Lua: for v in f do print(v) end
    /// ```
    pub fn from_iter<I>(name: &'static str, iter: I) -> Function
    where
        I: Iterator + Send + 'static,
        I::Item: IntoIterResult + Send + 'static,
    {
        let iter = parking_lot::Mutex::new(iter);
        Function::native(NativeFunction {
            signature: make_signature(name, vec![], false, None),
            call: NativeCall::SyncPlain(Arc::new(move |_args| match iter.lock().next() {
                Some(item) => item.into_iter_result(),
                None => Ok(vec![Value::Nil]),
            })),
        })
    }

    /// Wrap an async [`Stream`](futures::stream::Stream) as a stateful
    /// Lua iterator function.
    ///
    /// Same semantics as [`from_iter`](Function::from_iter) but polls
    /// the stream asynchronously.
    ///
    /// # Examples
    ///
    /// ```
    /// use shingetsu_vm::Function;
    ///
    /// let stream = futures::stream::iter(vec![1i64, 2, 3]);
    /// let _f = Function::from_stream("count", stream);
    /// ```
    pub fn from_stream<S>(name: &'static str, stream: S) -> Function
    where
        S: futures::stream::Stream + Send + Unpin + 'static,
        S::Item: IntoIterResult + Send + 'static,
    {
        use futures::stream::StreamExt;
        let stream = Arc::new(futures::lock::Mutex::new(stream));
        Function::native(NativeFunction {
            signature: make_signature(name, vec![], false, None),
            call: NativeCall::Async(Arc::new(move |_ctx, _args| {
                let stream = Arc::clone(&stream);
                Box::pin(async move {
                    let result = stream.lock().await.next().await;
                    match result {
                        Some(item) => item.into_iter_result(),
                        None => Ok(vec![Value::Nil]),
                    }
                })
            })),
        })
    }

    /// Package this function with state and control values into the
    /// `(iterator_fn, state, control)` triple that Lua's generic `for`
    /// expects.
    ///
    /// Returns a [`Variadic`] ready to be returned from a `#[function]`.
    ///
    /// # Examples
    ///
    /// ```
    /// use shingetsu_vm::{Function, Table, Value, VmError};
    ///
    /// let step = Function::wrap(
    ///     "iter",
    ///     |_t: Table, idx: i64| -> Result<Option<i64>, VmError> { Ok(Some(idx + 1)) },
    /// );
    /// let t = Table::new();
    /// let _triple = step.generic_for(Value::Table(t), Value::Integer(0));
    /// ```
    pub fn generic_for(self, state: Value, control: Value) -> Variadic {
        Variadic(vec![Value::Function(self), state, control])
    }
}

// ---------------------------------------------------------------------------
// IntoIterResult — convert iterator items to Lua multi-values
// ---------------------------------------------------------------------------

/// Trait for converting iterator items into Lua return values.
///
/// Implemented for both infallible (`T: IntoLuaMulti`) and fallible
/// (`Result<T, VmError>`) item types.
pub trait IntoIterResult {
    fn into_iter_result(self) -> Result<Vec<Value>, VmError>;
}

impl<T: IntoLuaMulti> IntoIterResult for T {
    fn into_iter_result(self) -> Result<Vec<Value>, VmError> {
        Ok(self.into_lua_multi())
    }
}

impl<T: IntoLuaMulti> IntoIterResult for Result<T, VmError> {
    fn into_iter_result(self) -> Result<Vec<Value>, VmError> {
        self.map(|v| v.into_lua_multi())
    }
}

// ---------------------------------------------------------------------------
// Helper: build a FunctionSignature from a static name
// ---------------------------------------------------------------------------

fn make_signature(
    name: &'static str,
    params: Vec<ParamSpec>,
    variadic: bool,
    lua_returns: Option<Vec<LuaType>>,
) -> Arc<FunctionSignature> {
    Arc::new(FunctionSignature {
        name: Bytes::from(name.as_bytes()),
        source: Bytes::default(),
        type_params: Vec::new(),
        params,
        variadic,
        arg_offset: 0,
        returns: None,
        lua_returns,
        line_defined: 0,
        last_line_defined: 0,
        num_upvalues: 0,
    })
}

/// Build a [`ParamSpec`] from the compile-time type information of a `LuaTyped` type.
#[inline]
fn param_spec<T: LuaTyped>() -> ParamSpec {
    ParamSpec {
        name: None,
        runtime_type: T::value_type(),
        lua_type: Some(T::lua_type()),
    }
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

/// Extract one `FromLua` argument from a slice by cloning the value at the
/// given index.  Used by the sync native call path to avoid allocating a
/// `Vec<Value>` for the argument list.
#[inline]
fn extract_arg_from_slice<T: FromLua>(
    args: &[Value],
    index: usize,
    position: usize,
    name: &'static str,
) -> Result<T, VmError> {
    let v = args.get(index).cloned().unwrap_or(Value::Nil);
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
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Func: Fn() -> Result<R, VmError> + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![], false, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::SyncPlain(Arc::new(move |_args| {
                        self().map(|r| r.into_lua_multi())
                    })),
                }
            }
        }

        // With context, no args
        impl<R, Func> IntoNativeFunction<WithCtx<()>> for Func
        where
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Func: Fn(CallContext) -> Result<R, VmError> + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![], false, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::SyncWithCtx(Arc::new(move |ctx, _args| {
                        self(ctx).map(|r| r.into_lua_multi())
                    })),
                }
            }
        }

        // No context, variadic only
        impl<R, Func> IntoNativeFunction<PlainVarargs<()>> for Func
        where
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Func: Fn(Variadic) -> Result<R, VmError> + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![], true, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::SyncPlain(Arc::new(move |args| {
                        self(Variadic(args.to_vec())).map(|r| r.into_lua_multi())
                    })),
                }
            }
        }

        // With context, variadic only
        impl<R, Func> IntoNativeFunction<WithCtxVarargs<()>> for Func
        where
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Func: Fn(CallContext, Variadic) -> Result<R, VmError> + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![], true, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::SyncWithCtx(Arc::new(move |ctx, args| {
                        self(ctx, Variadic(args.to_vec())).map(|r| r.into_lua_multi())
                    })),
                }
            }
        }

        // Async: no context, no args
        impl<Fut, R, Func> IntoNativeFunction<AsyncPlain<()>> for Func
        where
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Fut: Future<Output = Result<R, VmError>> + Send + 'static,
            Func: Fn() -> Fut + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![], false, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::Async(Arc::new(move |_ctx, _args| {
                        let fut = self();
                        Box::pin(async move {
                            fut.await.map(|r| r.into_lua_multi())
                        })
                    })),
                }
            }
        }

        // Async: with context, no args
        impl<Fut, R, Func> IntoNativeFunction<AsyncWithCtx<()>> for Func
        where
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Fut: Future<Output = Result<R, VmError>> + Send + 'static,
            Func: Fn(CallContext) -> Fut + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![], false, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::Async(Arc::new(move |ctx, _args| {
                        let fut = self(ctx);
                        Box::pin(async move {
                            fut.await.map(|r| r.into_lua_multi())
                        })
                    })),
                }
            }
        }

        // Async: no context, variadic only
        impl<Fut, R, Func> IntoNativeFunction<AsyncPlainVarargs<()>> for Func
        where
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Fut: Future<Output = Result<R, VmError>> + Send + 'static,
            Func: Fn(Variadic) -> Fut + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![], true, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::Async(Arc::new(move |_ctx, args| {
                        let fut = self(Variadic(args));
                        Box::pin(async move { fut.await.map(|r| r.into_lua_multi()) })
                    })),
                }
            }
        }

        // Async: with context, variadic only
        impl<Fut, R, Func> IntoNativeFunction<AsyncWithCtxVarargs<()>> for Func
        where
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Fut: Future<Output = Result<R, VmError>> + Send + 'static,
            Func: Fn(CallContext, Variadic) -> Fut + Send + Sync + 'static,
        {
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![], true, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::Async(Arc::new(move |ctx, args| {
                        let fut = self(ctx, Variadic(args));
                        Box::pin(async move { fut.await.map(|r| r.into_lua_multi()) })
                    })),
                }
            }
        }
    };

    // Recursive case: one or more typed args
    ($($T:ident),+) => {
        // No context
        impl<$($T,)* R, Func> IntoNativeFunction<Plain<($($T,)*)>> for Func
        where
            $($T: FromLua + LuaTyped,)*
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Func: Fn($($T,)*) -> Result<R, VmError> + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![$(param_spec::<$T>(),)*], false, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::SyncPlain(Arc::new(move |args| {
                        let mut __idx: usize = 0;
                        let mut __pos: usize = 0;
                        $(
                            __pos += 1;
                            let $T = extract_arg_from_slice::<$T>(args, __idx, __pos, name)?;
                            __idx += 1;
                        )*
                        self($($T,)*).map(|r| r.into_lua_multi())
                    })),
                }
            }
        }

        // With context
        impl<$($T,)* R, Func> IntoNativeFunction<WithCtx<($($T,)*)>> for Func
        where
            $($T: FromLua + LuaTyped,)*
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Func: Fn(CallContext, $($T,)*) -> Result<R, VmError> + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![$(param_spec::<$T>(),)*], false, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::SyncWithCtx(Arc::new(move |ctx, args| {
                        let mut __idx: usize = 0;
                        let mut __pos: usize = 0;
                        $(
                            __pos += 1;
                            let $T = extract_arg_from_slice::<$T>(args, __idx, __pos, name)?;
                            __idx += 1;
                        )*
                        self(ctx, $($T,)*).map(|r| r.into_lua_multi())
                    })),
                }
            }
        }

        // Typed args + trailing Variadic, no context
        impl<$($T,)* R, Func> IntoNativeFunction<PlainVarargs<($($T,)*)>> for Func
        where
            $($T: FromLua + LuaTyped,)*
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Func: Fn($($T,)* Variadic) -> Result<R, VmError> + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![$(param_spec::<$T>(),)*], true, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::SyncPlain(Arc::new(move |args| {
                        let mut __idx: usize = 0;
                        let mut __pos: usize = 0;
                        $(
                            __pos += 1;
                            let $T = extract_arg_from_slice::<$T>(args, __idx, __pos, name)?;
                            __idx += 1;
                        )*
                        let __variadic = Variadic(args[__idx..].to_vec());
                        self($($T,)* __variadic).map(|r| r.into_lua_multi())
                    })),
                }
            }
        }

        // Typed args + trailing Variadic, with context
        impl<$($T,)* R, Func> IntoNativeFunction<WithCtxVarargs<($($T,)*)>> for Func
        where
            $($T: FromLua + LuaTyped,)*
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Func: Fn(CallContext, $($T,)* Variadic) -> Result<R, VmError> + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![$(param_spec::<$T>(),)*], true, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::SyncWithCtx(Arc::new(move |ctx, args| {
                        let mut __idx: usize = 0;
                        let mut __pos: usize = 0;
                        $(
                            __pos += 1;
                            let $T = extract_arg_from_slice::<$T>(args, __idx, __pos, name)?;
                            __idx += 1;
                        )*
                        let __variadic = Variadic(args[__idx..].to_vec());
                        self(ctx, $($T,)* __variadic).map(|r| r.into_lua_multi())
                    })),
                }
            }
        }

        // Async: no context
        impl<$($T,)* Fut, R, Func> IntoNativeFunction<AsyncPlain<($($T,)*)>> for Func
        where
            $($T: FromLua + LuaTyped + Send + 'static,)*
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Fut: Future<Output = Result<R, VmError>> + Send + 'static,
            Func: Fn($($T,)*) -> Fut + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![$(param_spec::<$T>(),)*], false, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::Async(Arc::new(move |_ctx, args| {
                        let mut __iter = args.into_iter();
                        let mut __pos: usize = 0;
                        let extraction: Result<($($T,)*), VmError> = (|| {
                            $(
                                __pos += 1;
                                let $T = extract_arg::<$T>(&mut __iter, __pos, name)?;
                            )*
                            Ok(($($T,)*))
                        })();
                        match extraction {
                            Err(e) => Box::pin(async move { Err(e) }),
                            Ok(($($T,)*)) => {
                                let fut = self($($T,)*);
                                Box::pin(async move {
                                    fut.await.map(|r| r.into_lua_multi())
                                })
                            }
                        }
                    })),
                }
            }
        }

        // Async: with context
        impl<$($T,)* Fut, R, Func> IntoNativeFunction<AsyncWithCtx<($($T,)*)>> for Func
        where
            $($T: FromLua + LuaTyped + Send + 'static,)*
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Fut: Future<Output = Result<R, VmError>> + Send + 'static,
            Func: Fn(CallContext, $($T,)*) -> Fut + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![$(param_spec::<$T>(),)*], false, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::Async(Arc::new(move |ctx, args| {
                        let mut __iter = args.into_iter();
                        let mut __pos: usize = 0;
                        let extraction: Result<($($T,)*), VmError> = (|| {
                            $(
                                __pos += 1;
                                let $T = extract_arg::<$T>(&mut __iter, __pos, name)?;
                            )*
                            Ok(($($T,)*))
                        })();
                        match extraction {
                            Err(e) => Box::pin(async move { Err(e) }),
                            Ok(($($T,)*)) => {
                                let fut = self(ctx, $($T,)*);
                                Box::pin(async move {
                                    fut.await.map(|r| r.into_lua_multi())
                                })
                            }
                        }
                    })),
                }
            }
        }

        // Async: typed args + trailing Variadic, no context
        impl<$($T,)* Fut, R, Func> IntoNativeFunction<AsyncPlainVarargs<($($T,)*)>> for Func
        where
            $($T: FromLua + LuaTyped + Send + 'static,)*
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Fut: Future<Output = Result<R, VmError>> + Send + 'static,
            Func: Fn($($T,)* Variadic) -> Fut + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![$(param_spec::<$T>(),)*], true, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::Async(Arc::new(move |_ctx, args| {
                        let mut __iter = args.into_iter();
                        let mut __pos: usize = 0;
                        let extraction: Result<($($T,)*), VmError> = (|| {
                            $(
                                __pos += 1;
                                let $T = extract_arg::<$T>(&mut __iter, __pos, name)?;
                            )*
                            Ok(($($T,)*))
                        })();
                        match extraction {
                            Err(e) => Box::pin(async move { Err(e) }),
                            Ok(($($T,)*)) => {
                                let __variadic = Variadic(__iter.collect());
                                let fut = self($($T,)* __variadic);
                                Box::pin(async move { fut.await.map(|r| r.into_lua_multi()) })
                            }
                        }
                    })),
                }
            }
        }

        // Async: typed args + trailing Variadic, with context
        impl<$($T,)* Fut, R, Func> IntoNativeFunction<AsyncWithCtxVarargs<($($T,)*)>> for Func
        where
            $($T: FromLua + LuaTyped + Send + 'static,)*
            R: IntoLuaMulti + LuaTypedMulti + Send + 'static,
            Fut: Future<Output = Result<R, VmError>> + Send + 'static,
            Func: Fn(CallContext, $($T,)* Variadic) -> Fut + Send + Sync + 'static,
        {
            #[allow(non_snake_case, unused_mut, unused_variables)]
            fn into_native_function(self, name: &'static str) -> NativeFunction {
                let sig = make_signature(name, vec![$(param_spec::<$T>(),)*], true, Some(R::lua_types()));
                NativeFunction {
                    signature: sig,
                    call: NativeCall::Async(Arc::new(move |ctx, args| {
                        let mut __iter = args.into_iter();
                        let mut __pos: usize = 0;
                        let extraction: Result<($($T,)*), VmError> = (|| {
                            $(
                                __pos += 1;
                                let $T = extract_arg::<$T>(&mut __iter, __pos, name)?;
                            )*
                            Ok(($($T,)*))
                        })();
                        match extraction {
                            Err(e) => Box::pin(async move { Err(e) }),
                            Ok(($($T,)*)) => {
                                let __variadic = Variadic(__iter.collect());
                                let fut = self(ctx, $($T,)* __variadic);
                                Box::pin(async move { fut.await.map(|r| r.into_lua_multi()) })
                            }
                        }
                    })),
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

#[cfg(test)]
mod tests {
    use crate::byte_string::Bytes;

    use super::*;
    use crate::table::Table;
    use crate::types::{FunctionLuaType, LuaType, TableLuaType, ValueType};

    /// Extract the NativeFunction from a Function, panicking if it's a Lua closure.
    fn native(f: &Function) -> &NativeFunction {
        match f.state() {
            crate::function::FunctionState::Native(n) => n,
            _ => panic!("expected native function"),
        }
    }

    // -----------------------------------------------------------------
    // Plain (no context, no variadic)
    // -----------------------------------------------------------------

    #[test]
    fn wrap_zero_args() {
        let f = Function::wrap("zero", || Ok(42i64));
        let n = native(&f);
        k9::assert_equal!(&*n.signature.name, b"zero");
        k9::assert_equal!(n.signature.params, vec![]);
        k9::assert_equal!(n.signature.variadic, false);
    }

    #[test]
    fn wrap_one_arg() {
        let f = Function::wrap("one", |a: i64| Ok(a + 1));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: Some(ValueType::Number),
                lua_type: Some(LuaType::Number)
            }]
        );
        k9::assert_equal!(n.signature.variadic, false);
    }

    #[test]
    fn wrap_two_args() {
        let f = Function::wrap("two", |a: i64, b: i64| Ok(a + b));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                },
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                }
            ]
        );
    }

    #[test]
    fn wrap_mixed_types() {
        let f = Function::wrap("mixed", |_s: Bytes, _n: i64, _flag: bool| Ok(()));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::String),
                    lua_type: Some(LuaType::String)
                },
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                },
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Boolean),
                    lua_type: Some(LuaType::Boolean)
                }
            ]
        );
    }

    #[test]
    fn wrap_optional_args() {
        let f = Function::wrap("opt", |_a: i64, _b: Option<i64>| Ok(()));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                },
                ParamSpec {
                    name: None,
                    runtime_type: None,
                    lua_type: Some(LuaType::Optional(Box::new(LuaType::Number)))
                }
            ]
        );
    }

    #[test]
    fn wrap_returns_unit() {
        let f = Function::wrap("unit", || Ok(()));
        let n = native(&f);
        k9::assert_equal!(n.signature.params, vec![]);
    }

    #[test]
    fn wrap_returns_tuple() {
        let f = Function::wrap("tuple", |a: i64, b: i64| Ok((a, b)));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                },
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                }
            ]
        );
    }

    #[test]
    fn wrap_value_arg_unconstrained() {
        let f = Function::wrap("val", |_v: Value| Ok(()));
        let n = native(&f);
        // Value is unconstrained — no runtime_type
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: None,
                lua_type: Some(LuaType::Any)
            }]
        );
    }

    #[test]
    fn wrap_table_arg() {
        let f = Function::wrap("tab", |_t: Table| Ok(()));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: Some(ValueType::Table),
                lua_type: Some(LuaType::Table(Box::new(TableLuaType {
                    fields: vec![],
                    indexer: None
                })))
            }]
        );
    }

    #[test]
    fn wrap_function_arg() {
        let f = Function::wrap("fn_arg", |_func: Function| Ok(()));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: Some(ValueType::Function),
                lua_type: Some(LuaType::Function(Box::new(FunctionLuaType {
                    type_params: vec![],
                    params: vec![],
                    variadic: Some(Box::new(LuaType::Any)),
                    returns: vec![],
                    is_method: false,
                    inferred_unannotated: false
                })))
            }]
        );
    }

    #[test]
    fn wrap_f64_arg_is_number() {
        let f = Function::wrap("num", |_x: f64| Ok(()));
        let n = native(&f);
        // f64 maps to Number (accepts both Integer and Float)
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: Some(ValueType::Number),
                lua_type: Some(LuaType::Number)
            }]
        );
    }

    #[test]
    fn wrap_string_arg() {
        let f = Function::wrap("str", |_s: String| Ok(()));
        let n = native(&f);
        k9::assert_equal!(n.signature.params[0].runtime_type, Some(ValueType::String));
        k9::assert_equal!(n.signature.params[0].lua_type, Some(LuaType::String));
    }

    // -----------------------------------------------------------------
    // WithCtx (context, no variadic)
    // -----------------------------------------------------------------

    #[test]
    fn wrap_ctx_zero_args() {
        let f = Function::wrap("ctx0", |_ctx: CallContext| Ok(()));
        let n = native(&f);
        // CallContext is not a Lua parameter
        k9::assert_equal!(n.signature.params, vec![]);
        k9::assert_equal!(n.signature.variadic, false);
    }

    #[test]
    fn wrap_ctx_one_arg() {
        let f = Function::wrap("ctx1", |_ctx: CallContext, _s: Bytes| Ok(()));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: Some(ValueType::String),
                lua_type: Some(LuaType::String)
            }]
        );
    }

    #[test]
    fn wrap_ctx_two_args() {
        let f = Function::wrap("ctx2", |_ctx: CallContext, _a: i64, _b: i64| Ok(()));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                },
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                }
            ]
        );
    }

    #[test]
    fn wrap_ctx_optional_args() {
        let f = Function::wrap("ctx_opt", |_ctx: CallContext, _a: i64, _b: Option<i64>| {
            Ok(())
        });
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                },
                ParamSpec {
                    name: None,
                    runtime_type: None,
                    lua_type: Some(LuaType::Optional(Box::new(LuaType::Number)))
                }
            ]
        );
    }

    // -----------------------------------------------------------------
    // PlainVarargs (no context, trailing variadic)
    // -----------------------------------------------------------------

    #[test]
    fn wrap_variadic_only() {
        let f = Function::wrap("var", |_args: Variadic| Ok(()));
        let n = native(&f);
        k9::assert_equal!(n.signature.params, vec![]);
        k9::assert_equal!(n.signature.variadic, true);
    }

    #[test]
    fn wrap_one_arg_then_variadic() {
        let f = Function::wrap("one_var", |_t: Table, _rest: Variadic| Ok(()));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: Some(ValueType::Table),
                lua_type: Some(LuaType::Table(Box::new(TableLuaType {
                    fields: vec![],
                    indexer: None
                })))
            }]
        );
        k9::assert_equal!(n.signature.variadic, true);
    }

    #[test]
    fn wrap_two_args_then_variadic() {
        let f = Function::wrap("two_var", |_a: i64, _b: i64, _rest: Variadic| Ok(()));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                },
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                }
            ]
        );
        k9::assert_equal!(n.signature.variadic, true);
    }

    // -----------------------------------------------------------------
    // WithCtxVarargs (context + trailing variadic)
    // -----------------------------------------------------------------

    #[test]
    fn wrap_ctx_variadic_only() {
        let f = Function::wrap("ctx_var", |_ctx: CallContext, _args: Variadic| Ok(()));
        let n = native(&f);
        k9::assert_equal!(n.signature.params, vec![]);
        k9::assert_equal!(n.signature.variadic, true);
    }

    #[test]
    fn wrap_ctx_one_arg_then_variadic() {
        let f = Function::wrap(
            "ctx_one_var",
            |_ctx: CallContext, _n: i64, _rest: Variadic| Ok(()),
        );
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: Some(ValueType::Number),
                lua_type: Some(LuaType::Number)
            }]
        );
        k9::assert_equal!(n.signature.variadic, true);
    }

    // -----------------------------------------------------------------
    // Closures that capture state
    // -----------------------------------------------------------------

    #[test]
    fn wrap_capturing_closure() {
        let offset = 100i64;
        let f = Function::wrap("capture", move |n: i64| Ok(n + offset));
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![ParamSpec {
                name: None,
                runtime_type: Some(ValueType::Number),
                lua_type: Some(LuaType::Number)
            }]
        );
    }

    #[test]
    fn wrap_capturing_arc() {
        use std::sync::Arc;
        let shared = Arc::new(42i64);
        let f = Function::wrap("arc", move || Ok(*shared));
        let n = native(&f);
        k9::assert_equal!(n.signature.params, vec![]);
    }

    // -----------------------------------------------------------------
    // Runtime behavior — actually invoke the wrapped functions
    // -----------------------------------------------------------------

    /// Helper: invoke a NativeFunction with the given args and block on the result.
    fn call(f: &Function, args: Vec<Value>) -> Result<Vec<Value>, VmError> {
        let n = native(f);
        let ctx = CallContext {
            global: crate::global_env::GlobalEnv::new(),
            call_stack: Arc::new(vec![]),
            native_name: Some(n.signature.name.clone()),
        };
        match &n.call {
            NativeCall::SyncPlain(call) => call(&args),
            NativeCall::SyncWithCtx(call) => call(ctx, &args),
            NativeCall::Async(call) => futures::executor::block_on(call(ctx, args)),
        }
    }

    #[test]
    fn call_zero_args_returns_value() {
        let f = Function::wrap("forty_two", || Ok(42i64));
        let result = call(&f, vec![]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(42)]);
    }

    #[test]
    fn call_one_arg_integer() {
        let f = Function::wrap("inc", |a: i64| Ok(a + 1));
        let result = call(&f, vec![Value::Integer(10)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(11)]);
    }

    #[test]
    fn call_two_args() {
        let f = Function::wrap("add", |a: i64, b: i64| Ok(a + b));
        let result = call(&f, vec![Value::Integer(3), Value::Integer(4)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(7)]);
    }

    #[test]
    fn call_returns_tuple() {
        let f = Function::wrap("swap", |a: i64, b: i64| Ok((b, a)));
        let result = call(&f, vec![Value::Integer(1), Value::Integer(2)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(2), Value::Integer(1)]);
    }

    #[test]
    fn call_returns_unit() {
        let f = Function::wrap("noop", || Ok(()));
        let result = call(&f, vec![]).unwrap();
        k9::assert_equal!(result, vec![]);
    }

    #[test]
    fn call_optional_arg_present() {
        let f = Function::wrap("opt", |a: i64, b: Option<i64>| Ok(a + b.unwrap_or(0)));
        let result = call(&f, vec![Value::Integer(5), Value::Integer(3)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(8)]);
    }

    #[test]
    fn call_optional_arg_missing() {
        let f = Function::wrap("opt", |a: i64, b: Option<i64>| Ok(a + b.unwrap_or(0)));
        let result = call(&f, vec![Value::Integer(5)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(5)]);
    }

    #[test]
    fn call_optional_arg_nil() {
        let f = Function::wrap("opt", |a: i64, b: Option<i64>| Ok(a + b.unwrap_or(0)));
        let result = call(&f, vec![Value::Integer(5), Value::Nil]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(5)]);
    }

    #[test]
    fn call_extra_args_ignored() {
        let f = Function::wrap("one", |a: i64| Ok(a));
        let result = call(
            &f,
            vec![Value::Integer(1), Value::Integer(99), Value::Integer(100)],
        )
        .unwrap();
        k9::assert_equal!(result, vec![Value::Integer(1)]);
    }

    #[test]
    fn call_missing_required_arg_gets_nil() {
        let f = Function::wrap("need_int", |_a: i64| Ok(()));
        let err = call(&f, vec![]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #1 to 'need_int' (number expected, got nil)"
        );
    }

    #[test]
    fn call_wrong_type_error() {
        let f = Function::wrap("need_int", |_a: i64| Ok(()));
        let err = call(&f, vec![Value::Boolean(true)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #1 to 'need_int' (number expected, got boolean)"
        );
    }

    #[test]
    fn call_wrong_type_second_arg() {
        let f = Function::wrap("two", |_a: i64, _b: Bytes| Ok(()));
        let err = call(&f, vec![Value::Integer(1), Value::Boolean(false)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #2 to 'two' (string expected, got boolean)"
        );
    }

    #[test]
    fn call_closure_error_propagated() {
        let f = Function::wrap("fail", || -> Result<(), VmError> {
            Err(VmError::LuaError {
                display: "custom error".to_string(),
                value: Value::string("custom error"),
            })
        });
        let err = call(&f, vec![]).unwrap_err();
        k9::assert_equal!(err.to_string(), "custom error");
    }

    #[test]
    fn call_f64_accepts_integer() {
        let f = Function::wrap("half", |x: f64| Ok(x / 2.0));
        let result = call(&f, vec![Value::Integer(10)]).unwrap();
        k9::assert_equal!(result, vec![Value::Float(5.0)]);
    }

    #[test]
    fn call_f64_accepts_float() {
        let f = Function::wrap("half", |x: f64| Ok(x / 2.0));
        let result = call(&f, vec![Value::Float(7.0)]).unwrap();
        k9::assert_equal!(result, vec![Value::Float(3.5)]);
    }

    #[test]
    fn call_with_context() {
        let f = Function::wrap("ctx_fn", |ctx: CallContext, a: i64| {
            // Verify context has the function name
            k9::assert_equal!(ctx.native_name.as_deref(), Some(b"ctx_fn".as_slice()));
            Ok(a * 2)
        });
        let result = call(&f, vec![Value::Integer(5)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(10)]);
    }

    #[test]
    fn call_variadic_collects_all() {
        let f = Function::wrap("count", |args: Variadic| {
            Ok(Value::Integer(args.0.len() as i64))
        });
        let result = call(
            &f,
            vec![Value::Integer(1), Value::Integer(2), Value::Integer(3)],
        )
        .unwrap();
        k9::assert_equal!(result, vec![Value::Integer(3)]);
    }

    #[test]
    fn call_variadic_empty() {
        let f = Function::wrap("count", |args: Variadic| {
            Ok(Value::Integer(args.0.len() as i64))
        });
        let result = call(&f, vec![]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(0)]);
    }

    #[test]
    fn call_typed_then_variadic() {
        let f = Function::wrap("first_rest", |first: i64, rest: Variadic| {
            Ok((first, Value::Integer(rest.0.len() as i64)))
        });
        let result = call(
            &f,
            vec![Value::Integer(42), Value::Boolean(true), Value::Nil],
        )
        .unwrap();
        k9::assert_equal!(result, vec![Value::Integer(42), Value::Integer(2)]);
    }

    #[test]
    fn call_typed_then_variadic_no_extras() {
        let f = Function::wrap("first_rest", |first: i64, rest: Variadic| {
            Ok((first, Value::Integer(rest.0.len() as i64)))
        });
        let result = call(&f, vec![Value::Integer(42)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(42), Value::Integer(0)]);
    }

    #[test]
    fn call_ctx_variadic() {
        let f = Function::wrap("ctx_var", |_ctx: CallContext, args: Variadic| {
            Ok(Value::Integer(args.0.len() as i64))
        });
        let result = call(&f, vec![Value::Integer(1), Value::Integer(2)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(2)]);
    }

    #[test]
    fn call_ctx_typed_then_variadic() {
        let f = Function::wrap("ctx_t_var", |_ctx: CallContext, n: i64, rest: Variadic| {
            Ok((n, Value::Integer(rest.0.len() as i64)))
        });
        let result = call(&f, vec![Value::Integer(10), Value::Nil, Value::Nil]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(10), Value::Integer(2)]);
    }

    #[test]
    fn call_bytes_arg() {
        let f = Function::wrap("echo", |s: Bytes| Ok(s));
        let result = call(&f, vec![Value::string("hello")]).unwrap();
        k9::assert_equal!(result, vec![Value::string("hello")]);
    }

    #[test]
    fn call_bool_arg() {
        let f = Function::wrap("not", |b: bool| Ok(!b));
        let result = call(&f, vec![Value::Boolean(true)]).unwrap();
        k9::assert_equal!(result, vec![Value::Boolean(false)]);
    }

    #[test]
    fn call_table_arg() {
        let f = Function::wrap("len", |t: Table| Ok(t.raw_len()));
        let t = Table::new();
        t.raw_insert(1, Value::Integer(10)).unwrap();
        t.raw_insert(2, Value::Integer(20)).unwrap();
        let result = call(&f, vec![Value::Table(t)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(2)]);
    }

    // -----------------------------------------------------------------
    // Async closures
    // -----------------------------------------------------------------

    #[test]
    fn async_zero_args() {
        let f = Function::wrap("async0", || async { Ok(42i64) });
        let result = call(&f, vec![]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(42)]);
    }

    #[test]
    fn async_one_arg() {
        let f = Function::wrap("async1", |a: i64| async move { Ok(a * 3) });
        let result = call(&f, vec![Value::Integer(7)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(21)]);
    }

    #[test]
    fn async_two_args() {
        let f = Function::wrap("async2", |a: i64, b: i64| async move { Ok(a + b) });
        let result = call(&f, vec![Value::Integer(3), Value::Integer(4)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(7)]);
    }

    #[test]
    fn async_with_context() {
        let f = Function::wrap("async_ctx", |ctx: CallContext, a: i64| async move {
            k9::assert_equal!(ctx.native_name.as_deref(), Some(b"async_ctx".as_slice()));
            Ok(a * 2)
        });
        let result = call(&f, vec![Value::Integer(5)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(10)]);
    }

    #[test]
    fn async_variadic() {
        let f = Function::wrap("async_var", |args: Variadic| async move {
            Ok(Value::Integer(args.0.len() as i64))
        });
        let result = call(&f, vec![Value::Integer(1), Value::Integer(2)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(2)]);
    }

    #[test]
    fn async_typed_then_variadic() {
        let f = Function::wrap("async_tv", |first: i64, rest: Variadic| async move {
            Ok((first, Value::Integer(rest.0.len() as i64)))
        });
        let result = call(
            &f,
            vec![Value::Integer(10), Value::Boolean(true), Value::Nil],
        )
        .unwrap();
        k9::assert_equal!(result, vec![Value::Integer(10), Value::Integer(2)]);
    }

    #[test]
    fn async_ctx_variadic() {
        let f = Function::wrap("async_cv", |_ctx: CallContext, args: Variadic| async move {
            Ok(Value::Integer(args.0.len() as i64))
        });
        let result = call(&f, vec![Value::Integer(1)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(1)]);
    }

    #[test]
    fn async_ctx_typed_then_variadic() {
        let f = Function::wrap(
            "async_ctv",
            |_ctx: CallContext, n: i64, rest: Variadic| async move {
                Ok((n, Value::Integer(rest.0.len() as i64)))
            },
        );
        let result = call(&f, vec![Value::Integer(5), Value::Nil]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(5), Value::Integer(1)]);
    }

    #[test]
    fn async_type_error_before_await() {
        let f = Function::wrap("async_err", |_a: i64| async move { Ok(()) });
        let err = call(&f, vec![Value::Boolean(true)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #1 to 'async_err' (number expected, got boolean)"
        );
    }

    #[test]
    fn async_error_from_future() {
        let f = Function::wrap("async_fail", || async {
            Err::<(), _>(VmError::LuaError {
                display: "async boom".to_string(),
                value: Value::string("async boom"),
            })
        });
        let err = call(&f, vec![]).unwrap_err();
        k9::assert_equal!(err.to_string(), "async boom");
    }

    #[test]
    fn async_closure_syntax() {
        // Native `async ||` closure syntax (stable since Rust 1.85)
        let f = Function::wrap("async_native", async |a: i64, b: i64| Ok(a + b));
        let result = call(&f, vec![Value::Integer(10), Value::Integer(20)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(30)]);
    }

    #[test]
    fn async_signature_metadata() {
        let f = Function::wrap("async_sig", |_a: i64, _b: Bytes| async { Ok(()) });
        let n = native(&f);
        k9::assert_equal!(
            n.signature.params,
            vec![
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::Number),
                    lua_type: Some(LuaType::Number)
                },
                ParamSpec {
                    name: None,
                    runtime_type: Some(ValueType::String),
                    lua_type: Some(LuaType::String)
                }
            ]
        );
        k9::assert_equal!(n.signature.variadic, false);
    }

    #[test]
    fn async_capturing_closure() {
        let offset = 100i64;
        let f = Function::wrap("async_cap", move |n: i64| async move { Ok(n + offset) });
        let result = call(&f, vec![Value::Integer(5)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(105)]);
    }

    #[test]
    fn async_capturing_arc() {
        let shared = Arc::new(42i64);
        let f = Function::wrap("async_arc", move || {
            let val = *shared;
            async move { Ok(val) }
        });
        let result = call(&f, vec![]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(42)]);
    }

    #[test]
    fn async_optional_arg_present() {
        let f = Function::wrap("async_opt", |a: i64, b: Option<i64>| async move {
            Ok(a + b.unwrap_or(0))
        });
        let result = call(&f, vec![Value::Integer(5), Value::Integer(3)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(8)]);
    }

    #[test]
    fn async_optional_arg_missing() {
        let f = Function::wrap("async_opt", |a: i64, b: Option<i64>| async move {
            Ok(a + b.unwrap_or(0))
        });
        let result = call(&f, vec![Value::Integer(5)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(5)]);
    }

    #[test]
    fn async_optional_arg_nil() {
        let f = Function::wrap("async_opt", |a: i64, b: Option<i64>| async move {
            Ok(a + b.unwrap_or(0))
        });
        let result = call(&f, vec![Value::Integer(5), Value::Nil]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(5)]);
    }

    #[test]
    fn async_returns_tuple() {
        let f = Function::wrap("async_tup", |a: i64, b: i64| async move { Ok((b, a)) });
        let result = call(&f, vec![Value::Integer(1), Value::Integer(2)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(2), Value::Integer(1)]);
    }

    #[test]
    fn async_extra_args_ignored() {
        let f = Function::wrap("async_extra", |a: i64| async move { Ok(a) });
        let result = call(
            &f,
            vec![Value::Integer(1), Value::Integer(99), Value::Integer(100)],
        )
        .unwrap();
        k9::assert_equal!(result, vec![Value::Integer(1)]);
    }

    #[test]
    fn call_variadic_type_error_on_typed_arg() {
        let f = Function::wrap("tv_err", |_a: i64, _rest: Variadic| Ok(()));
        let err = call(&f, vec![Value::Boolean(true), Value::Integer(1)]).unwrap_err();
        k9::assert_equal!(
            err.to_string(),
            "bad argument #1 to 'tv_err' (number expected, got boolean)"
        );
    }

    // -----------------------------------------------------------------
    // Function::from_iter
    // -----------------------------------------------------------------

    #[test]
    fn from_iter_basic() {
        let f = Function::from_iter("count", vec![1i64, 2, 3].into_iter());
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(1)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(2)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(3)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Nil]);
    }

    #[test]
    fn from_iter_exhausted_stays_nil() {
        let f = Function::from_iter("one", std::iter::once(42i64));
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(42)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Nil]);
        // Calling again after exhaustion still returns nil
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Nil]);
    }

    #[test]
    fn from_iter_ignores_args() {
        let f = Function::from_iter("seq", vec![10i64].into_iter());
        // Extra arguments are passed by generic-for but should be ignored
        let result = call(&f, vec![Value::Integer(99), Value::Integer(0)]).unwrap();
        k9::assert_equal!(result, vec![Value::Integer(10)]);
    }

    #[test]
    fn from_iter_tuple_items() {
        let items = vec![(1i64, Bytes::from("a")), (2, Bytes::from("b"))];
        let f = Function::from_iter("kv", items.into_iter());
        k9::assert_equal!(
            call(&f, vec![]).unwrap(),
            vec![Value::Integer(1), Value::string("a")]
        );
        k9::assert_equal!(
            call(&f, vec![]).unwrap(),
            vec![Value::Integer(2), Value::string("b")]
        );
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Nil]);
    }

    #[test]
    fn from_iter_fallible_ok() {
        let items = vec![Ok(1i64), Ok(2)];
        let f = Function::from_iter("ok", items.into_iter());
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(1)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(2)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Nil]);
    }

    #[test]
    fn from_iter_fallible_err() {
        let items: Vec<Result<i64, VmError>> = vec![
            Ok(1),
            Err(VmError::LuaError {
                display: "iter error".to_string(),
                value: Value::string("iter error"),
            }),
        ];
        let f = Function::from_iter("fail", items.into_iter());
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(1)]);
        let err = call(&f, vec![]).unwrap_err();
        k9::assert_equal!(err.to_string(), "iter error");
    }

    #[test]
    fn from_iter_empty() {
        let f = Function::from_iter("empty", std::iter::empty::<i64>());
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Nil]);
    }

    #[test]
    fn from_iter_signature() {
        let f = Function::from_iter("sig", vec![1i64].into_iter());
        let n = native(&f);
        k9::assert_equal!(&*n.signature.name, b"sig");
        k9::assert_equal!(n.signature.params, vec![]);
        k9::assert_equal!(n.signature.variadic, false);
    }

    // -----------------------------------------------------------------
    // Function::from_stream
    // -----------------------------------------------------------------

    #[test]
    fn from_stream_basic() {
        let stream = futures::stream::iter(vec![1i64, 2, 3]);
        let f = Function::from_stream("stream", stream);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(1)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(2)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(3)]);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Nil]);
    }

    #[test]
    fn from_stream_fallible() {
        let items: Vec<Result<i64, VmError>> = vec![
            Ok(10),
            Err(VmError::LuaError {
                display: "stream fail".to_string(),
                value: Value::string("stream fail"),
            }),
        ];
        let stream = futures::stream::iter(items);
        let f = Function::from_stream("sfail", stream);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Integer(10)]);
        let err = call(&f, vec![]).unwrap_err();
        k9::assert_equal!(err.to_string(), "stream fail");
    }

    #[test]
    fn from_stream_empty() {
        let stream = futures::stream::iter(Vec::<i64>::new());
        let f = Function::from_stream("empty_s", stream);
        k9::assert_equal!(call(&f, vec![]).unwrap(), vec![Value::Nil]);
    }

    // -----------------------------------------------------------------
    // Function::generic_for
    // -----------------------------------------------------------------

    #[test]
    fn generic_for_triple() {
        let step = Function::wrap("step", |_t: Table, idx: i64| Ok(idx + 1));
        let t = Table::new();
        let triple = step.generic_for(Value::Table(t.clone()), Value::Integer(0));
        match triple.0.as_slice() {
            [Value::Function(_), state, control] => {
                k9::assert_equal!(state, &Value::Table(t));
                k9::assert_equal!(control, &Value::Integer(0));
            }
            other => panic!("expected [Function, Table, Integer], got {other:?}"),
        }
    }

    #[test]
    fn generic_for_nil_state() {
        let f = Function::from_iter("it", vec![1i64].into_iter());
        let triple = f.generic_for(Value::Nil, Value::Nil);
        match triple.0.as_slice() {
            [Value::Function(_), state, control] => {
                k9::assert_equal!(state, &Value::Nil);
                k9::assert_equal!(control, &Value::Nil);
            }
            other => panic!("expected [Function, Nil, Nil], got {other:?}"),
        }
    }
}
