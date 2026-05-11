use std::ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Rem, Sub};
use std::sync::Arc;

use downcast_rs::DowncastSync;

use crate::call_context::CallContext;
use crate::error::VmError;
use crate::global_env::GlobalEnv;
use crate::types::LuaType;
use crate::value::{Value, ValueVec};

/// Re-materialisation closure produced by [`Userdata::snapshot`].
///
/// Allows a userdata value's logical content to be reconstructed in
/// a different [`GlobalEnv`] than the one that produced it.  Useful
/// for host-side caches that span VM instances — the cached value
/// can hold a `Snapshot` instead of a [`Value`], avoiding lifetime
/// dependencies on any specific VM.
///
/// The closure is `Send + Sync + 'static` so it can travel through
/// async caches and be invoked from any thread.
#[derive(Clone)]
pub struct Snapshot(pub Arc<dyn Fn(&GlobalEnv) -> Result<Value, VmError> + Send + Sync + 'static>);

impl Snapshot {
    /// Construct a [`Snapshot`] from a closure.
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&GlobalEnv) -> Result<Value, VmError> + Send + Sync + 'static,
    {
        Self(Arc::new(f))
    }

    /// Re-materialise the snapshot in the given env.
    pub fn rebuild(&self, env: &GlobalEnv) -> Result<Value, VmError> {
        (self.0)(env)
    }
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot").finish_non_exhaustive()
    }
}

/// Describes the position of the non-self operand in a binary metamethod.
///
/// When a userdata binary metamethod like `__add` or `__lt` is dispatched,
/// `self` is the userdata object and the other operand may be on either side
/// of the operator. This enum tells the method implementation which side
/// the other operand was on, enabling correct behavior for non-commutative
/// operations.
///
/// # Commutative operations
///
/// For commutative operations where operand order doesn't matter, use
/// [`into_inner`](BinOpSide::into_inner) or the trait-delegating convenience
/// method:
///
/// ```rust,ignore
/// #[lua_metamethod(Add)]
/// fn add_mm(&self, other: BinOpSide<i64>) -> i64 {
///     // Convenience: delegates to std::ops::Add in the correct order.
///     other.add(self.0)
///     // Or equivalently, since addition is commutative:
///     // self.0 + other.into_inner()
/// }
/// ```
///
/// # Non-commutative operations
///
/// For non-commutative operations like subtraction or comparison, use
/// [`apply`](BinOpSide::apply) or the trait-delegating convenience method.
/// Both ensure operands are placed in the correct order:
///
/// ```rust,ignore
/// #[lua_metamethod(Sub)]
/// fn sub_mm(&self, other: BinOpSide<i64>) -> i64 {
///     // Convenience: delegates to std::ops::Sub in the correct order.
///     other.impl_sub(self.0)
///     // Or with apply:
///     // other.apply(self.0, |lhs, rhs| lhs - rhs)
/// }
///
/// #[lua_metamethod(Lt)]
/// fn lt_mm(&self, other: BinOpSide<i64>) -> bool {
///     other.impl_lt(self.0)
/// }
/// ```
///
/// # Plain parameters
///
/// If a binary metamethod parameter is declared as a plain type instead of
/// `BinOpSide<T>`, the other operand is passed directly without position
/// information. This is convenient for commutative operations but incorrect
/// for non-commutative ones when `self` may appear on either side:
///
/// ```rust,ignore
/// #[lua_metamethod(Add)]
/// fn add_mm(&self, rhs: i64) -> i64 {
///     self.0 + rhs // Fine — addition is commutative.
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOpSide<T> {
    /// The other operand is to the left of the operator: `other OP self`.
    LeftOfOperator(T),
    /// The other operand is to the right of the operator: `self OP other`.
    RightOfOperator(T),
}

impl<T> BinOpSide<T> {
    /// Apply a binary operation with the correct operand ordering.
    ///
    /// The closure receives `(lhs, rhs)` matching the original Lua expression
    /// `lhs OP rhs`. The `self_val` argument is the userdata's value and is
    /// placed on the correct side automatically:
    ///
    /// - `RightOfOperator(val)`: expression was `self OP val`, closure
    ///   receives `(self_val, val)`.
    /// - `LeftOfOperator(val)`: expression was `val OP self`, closure
    ///   receives `(val, self_val)`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// #[lua_metamethod(Sub)]
    /// fn sub_mm(&self, other: BinOpSide<i64>) -> i64 {
    ///     // Closure always gets (lhs, rhs) of the original expression.
    ///     other.apply(self.0, |lhs, rhs| lhs - rhs)
    /// }
    /// ```
    /// When both operands are the same type `T`, use this form directly.
    /// For mixed types, match on the variants instead.
    pub fn apply<R>(self, self_val: T, f: impl FnOnce(T, T) -> R) -> R {
        match self {
            BinOpSide::RightOfOperator(other) => f(self_val, other),
            BinOpSide::LeftOfOperator(other) => f(other, self_val),
        }
    }

    /// Returns the contained operand regardless of position.
    ///
    /// Useful for commutative operations like addition or bitwise-or
    /// where operand order doesn't affect the result.
    pub fn into_inner(self) -> T {
        match self {
            BinOpSide::RightOfOperator(v) | BinOpSide::LeftOfOperator(v) => v,
        }
    }

    /// Delegates to [`std::ops::Add`] with correct operand ordering.
    pub fn add<S>(self, self_val: S) -> S::Output
    where
        S: Add<T>,
        T: Add<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val + other,
            BinOpSide::LeftOfOperator(other) => other + self_val,
        }
    }

    /// Implements subtraction with correct operand ordering via [`std::ops::Sub`].
    ///
    /// Named `impl_sub` so that `other.impl_sub(self.0)` reads as
    /// "implement subtraction against self" rather than "other - self".
    pub fn impl_sub<S>(self, self_val: S) -> S::Output
    where
        S: Sub<T>,
        T: Sub<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val - other,
            BinOpSide::LeftOfOperator(other) => other - self_val,
        }
    }

    /// Delegates to [`std::ops::Mul`] with correct operand ordering.
    pub fn mul<S>(self, self_val: S) -> S::Output
    where
        S: Mul<T>,
        T: Mul<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val * other,
            BinOpSide::LeftOfOperator(other) => other * self_val,
        }
    }

    /// Implements division with correct operand ordering via [`std::ops::Div`].
    ///
    /// Named `impl_div` so that `other.impl_div(self.0)` reads as
    /// "implement division against self" rather than "other / self".
    pub fn impl_div<S>(self, self_val: S) -> S::Output
    where
        S: Div<T>,
        T: Div<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val / other,
            BinOpSide::LeftOfOperator(other) => other / self_val,
        }
    }

    /// Implements remainder with correct operand ordering via [`std::ops::Rem`].
    ///
    /// Named `impl_rem` so that `other.impl_rem(self.0)` reads as
    /// "implement remainder against self" rather than "other % self".
    pub fn impl_rem<S>(self, self_val: S) -> S::Output
    where
        S: Rem<T>,
        T: Rem<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val % other,
            BinOpSide::LeftOfOperator(other) => other % self_val,
        }
    }

    /// Delegates to [`std::ops::BitAnd`] with correct operand ordering.
    pub fn bitand<S>(self, self_val: S) -> S::Output
    where
        S: BitAnd<T>,
        T: BitAnd<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val & other,
            BinOpSide::LeftOfOperator(other) => other & self_val,
        }
    }

    /// Delegates to [`std::ops::BitOr`] with correct operand ordering.
    pub fn bitor<S>(self, self_val: S) -> S::Output
    where
        S: BitOr<T>,
        T: BitOr<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val | other,
            BinOpSide::LeftOfOperator(other) => other | self_val,
        }
    }

    /// Delegates to [`std::ops::BitXor`] with correct operand ordering.
    pub fn bitxor<S>(self, self_val: S) -> S::Output
    where
        S: BitXor<T>,
        T: BitXor<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val ^ other,
            BinOpSide::LeftOfOperator(other) => other ^ self_val,
        }
    }

    /// Implements `<` with correct operand ordering via [`PartialOrd`].
    ///
    /// Named `impl_lt` rather than `lt` so that `other.impl_lt(self.0)` reads
    /// as "implement less-than against self" rather than "other < self".
    pub fn impl_lt<S>(self, self_val: S) -> bool
    where
        S: PartialOrd<T>,
        T: PartialOrd<S>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val < other,
            BinOpSide::LeftOfOperator(other) => other < self_val,
        }
    }

    /// Implements `<=` with correct operand ordering via [`PartialOrd`].
    ///
    /// Named `impl_le` rather than `le` so that `other.impl_le(self.0)` reads
    /// as "implement less-or-equal against self" rather than "other <= self".
    pub fn impl_le<S>(self, self_val: S) -> bool
    where
        S: PartialOrd<T>,
        T: PartialOrd<S>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val <= other,
            BinOpSide::LeftOfOperator(other) => other <= self_val,
        }
    }

    /// Implements `==` with correct operand ordering via [`PartialEq`].
    ///
    /// Named `impl_eq` rather than `eq` so that `other.impl_eq(self.0)` reads
    /// as "implement equality against self" rather than "other == self".
    pub fn impl_eq<S>(self, self_val: S) -> bool
    where
        S: PartialEq<T>,
        T: PartialEq<S>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val == other,
            BinOpSide::LeftOfOperator(other) => other == self_val,
        }
    }
}

/// Structured representation of a [`Userdata`]'s contents, returned
/// from [`Userdata::pretty_entries`] for use by debug renderers.
///
/// The iterator borrows from the userdata (no `Send + Sync` /
/// `'static` bounds) since renderers consume it within a single sync
/// call.
pub enum PrettyShape<'a> {
    /// Renders like a Lua map: `{ key = value, ... }`.
    Map(Box<dyn Iterator<Item = Result<(Value, Value), VmError>> + 'a>),
    /// Renders like a Lua array: `{ v1, v2, v3 }`.
    Vec(Box<dyn Iterator<Item = Result<Value, VmError>> + 'a>),
}

/// Trait implemented by host-provided Rust objects exposed to Lua.
///
/// All metamethod calls are async so that getters, setters, and metamethods
/// can dispatch to async operations (database reads, network calls, etc.)
/// without the VM needing to know whether the implementation is sync or async.
///
/// Arbitrary metamethod names are supported through the single `dispatch`
/// entry point.  Standard names (`__index`, `__add`, etc.) and any
/// host-defined custom names are handled uniformly.
///
/// The `Arc<Self>` receiver on `dispatch` ensures the produced future is
/// `'static` so it can be stored across yield points without lifetime
/// complications.
///
/// # Deriving
///
/// - `#[derive(shingetsu::UserData)]` generates a minimal no-method
///   implementation for types that only need a type name.
/// - `#[shingetsu::userdata]` on an `impl` block generates a full
///   implementation with `#[lua_method]`, `#[lua_field]`, and
///   `#[lua_metamethod]` annotations routing to typed Rust methods.
///
/// Implementors should also call `impl_downcast!(sync YourType)` (or use the
/// derive macros above, which do this automatically) to enable downcasting
/// from `Arc<dyn Userdata>` back to `Arc<YourType>`.
#[async_trait::async_trait]
pub trait Userdata: DowncastSync {
    /// The name shown in error messages and stack traces.
    fn type_name(&self) -> &'static str;

    /// Return the full structural type information for this userdata.
    ///
    /// The default returns an opaque `LuaType::Named(type_name)`.  The
    /// `#[shingetsu::userdata]` proc macro overrides this to return a
    /// `LuaType::Table` with the full method/field layout so the
    /// compiler can perform compile-time checks (e.g. dot-vs-colon
    /// call syntax validation).
    fn lua_type_info(&self) -> LuaType {
        LuaType::named(self.type_name())
    }

    /// Produce a re-materialisation closure that can rebuild this
    /// value in a different [`crate::GlobalEnv`].  The returned
    /// [`Snapshot`] decouples the value's lifetime from the VM that
    /// produced it, enabling host-side caches that span VM
    /// instances (and motivating the `__memoize` metamethod
    /// convention exposed by the `#[shingetsu::userdata]` macro).
    ///
    /// The default returns `None`, signalling that the type does
    /// not opt in to cross-VM snapshotting.  Implementors that are
    /// `Clone` and produce stable [`crate::Value`] output via
    /// [`crate::IntoLua`] can opt in by overriding this method (the
    /// macro provides `#[lua_snapshot]` / `#[lua(snapshot)]` for
    /// that purpose).
    fn snapshot(&self) -> Option<Snapshot> {
        None
    }

    /// Returns `true` if this userdata implements the named
    /// metamethod.  The default returns `false`; the
    /// `#[shingetsu::userdata]` proc macro overrides this to return
    /// `true` for every metamethod the type registers.  Used by the
    /// VM's `==` dispatch (which falls back to rawequal-false when
    /// `__eq` isn't implemented, per Lua 5.4 semantics) and is
    /// available for any other caller that needs to probe a
    /// metamethod's presence without actually invoking it.
    fn has_metamethod(&self, _name: &str) -> bool {
        false
    }

    /// Describe this userdata's contents as a sequence of values
    /// for structured renderers like `debug.pretty_print`.
    ///
    /// The default returns `None`, meaning "opaque" — renderers
    /// fall back to `"userdata: 0xADDR"`.  Types that wrap a
    /// table-like or array-like structure should override this so
    /// debugging output can show their contents.
    ///
    /// The returned iterator is lazy: renderers walk only as many
    /// entries as they need (e.g. `pretty_print`'s `max_entries`
    /// cap).  Each item is `Result<...>` because lazy materialization
    /// of nested values may itself fail.
    fn pretty_entries<'a>(&'a self, _env: &GlobalEnv) -> Option<Result<PrettyShape<'a>, VmError>> {
        None
    }

    /// Synchronous `__index` fast path.
    ///
    /// Called by the VM before the async `dispatch` path for `__index`.
    /// If this returns `Some(result)`, the VM uses it directly — no
    /// `CallContext`, no call-stack snapshot, no async yield.
    ///
    /// The default returns `None`, falling through to `dispatch`.
    fn index(&self, _key: &Value) -> Option<Result<ValueVec, VmError>> {
        None
    }

    /// Synchronous fused-method-call fast path.
    ///
    /// Called by the VM's `Invoke` opcode handler before the
    /// `index`-then-call path.  When implemented, dispatches the named
    /// method directly and returns the result `ValueVec`, bypassing the
    /// allocation of an intermediate `Function` value.
    ///
    /// `args[0]` is the receiver (this userdata wrapped in
    /// `Value::Userdata`); `args[1..]` are the explicit method arguments.
    ///
    /// Return values:
    /// - `Some(Ok(values))` — method handled successfully; `values` is
    ///   the result tuple.
    /// - `Some(Err(e))` — method dispatched but failed (e.g. argument
    ///   conversion error).  Propagated to the caller as-is.
    /// - `None` — this method is not handled by the fast path; the VM
    ///   falls through to `index` and then to `dispatch`.  Methods that
    ///   need a `CallContext` (e.g. to call back into the VM) should
    ///   return `None` here.
    ///
    /// The default returns `None`.
    fn invoke(&self, _method: &[u8], _args: &[Value]) -> Option<Result<ValueVec, VmError>> {
        None
    }

    /// Asynchronous fused-method-call fast path.
    ///
    /// Like [`invoke`](Self::invoke), but for `async fn` methods.
    /// Returns `Some((sig, fut))` when the method is handled — `sig` is
    /// used to populate the call-stack entry that the VM pushes before
    /// awaiting `fut`.  Returns `None` when the method is not handled
    /// here (the VM falls through to `index` and then to `dispatch`).
    fn invoke_async(
        self: Arc<Self>,
        _method: &[u8],
        _args: ValueVec,
    ) -> Option<(
        Arc<crate::types::FunctionSignature>,
        futures::future::BoxFuture<'static, Result<ValueVec, VmError>>,
    )> {
        None
    }

    /// Synchronous `__newindex` fast path.
    ///
    /// Like `index`, but for field assignment.  `key` and `value` are
    /// the second and third args from the `__newindex` metamethod.
    fn newindex(&self, _key: &Value, _value: &Value) -> Option<Result<ValueVec, VmError>> {
        None
    }

    /// Dispatch a metamethod call.
    ///
    /// `metamethod` is the full name, e.g. `"__index"`, `"__add"`, or any
    /// arbitrary host-defined name.
    ///
    /// `args` contains the arguments in standard Lua order:
    /// - `__index`:    `[receiver, key]`
    /// - `__newindex`: `[receiver, key, value]`
    /// - `__add`:      `[lhs, rhs]`
    /// - `__call`:     `[callee, arg1, arg2, …]`
    ///
    /// The default returns `VmError::HostError` indicating the metamethod is
    /// not implemented.
    async fn dispatch(
        self: Arc<Self>,
        context: CallContext,
        metamethod: &str,
        args: ValueVec,
    ) -> Result<ValueVec, VmError> {
        let _ = (context, args);
        Err(VmError::HostError {
            name: format!("{}:{}", self.type_name(), metamethod),
            source: format!(
                "metamethod '{}' not implemented for '{}'",
                metamethod,
                self.type_name()
            )
            .into(),
        })
    }
}

downcast_rs::impl_downcast!(sync Userdata);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binopside_apply_right_of_operator() {
        let side = BinOpSide::RightOfOperator(3);
        let result = side.apply(10, |lhs, rhs| lhs - rhs);
        k9::assert_equal!(result, 7);
    }

    #[test]
    fn binopside_apply_left_of_operator() {
        let side = BinOpSide::LeftOfOperator(3);
        let result = side.apply(10, |lhs, rhs| lhs - rhs);
        k9::assert_equal!(result, -7);
    }

    #[test]
    fn binopside_into_inner() {
        k9::assert_equal!(BinOpSide::RightOfOperator(42).into_inner(), 42);
        k9::assert_equal!(BinOpSide::LeftOfOperator(42).into_inner(), 42);
    }

    #[test]
    fn binopside_add() {
        k9::assert_equal!(BinOpSide::RightOfOperator(3).add(10), 13);
        k9::assert_equal!(BinOpSide::LeftOfOperator(3).add(10), 13);
    }

    #[test]
    fn binopside_sub() {
        k9::assert_equal!(BinOpSide::RightOfOperator(3).impl_sub(10), 7);
        k9::assert_equal!(BinOpSide::LeftOfOperator(3).impl_sub(10), -7);
    }

    #[test]
    fn binopside_mul() {
        k9::assert_equal!(BinOpSide::RightOfOperator(3).mul(10), 30);
        k9::assert_equal!(BinOpSide::LeftOfOperator(3).mul(10), 30);
    }

    #[test]
    fn binopside_div() {
        k9::assert_equal!(BinOpSide::RightOfOperator(3.0).impl_div(12.0), 4.0);
        k9::assert_equal!(BinOpSide::LeftOfOperator(12.0).impl_div(3.0), 4.0);
    }

    #[test]
    fn binopside_rem() {
        k9::assert_equal!(BinOpSide::RightOfOperator(3).impl_rem(10), 1);
        k9::assert_equal!(BinOpSide::LeftOfOperator(3).impl_rem(10), 3);
    }

    #[test]
    fn binopside_bitand() {
        k9::assert_equal!(BinOpSide::RightOfOperator(0b1010).bitand(0b1100), 0b1000);
        k9::assert_equal!(BinOpSide::LeftOfOperator(0b1010).bitand(0b1100), 0b1000);
    }

    #[test]
    fn binopside_bitor() {
        k9::assert_equal!(BinOpSide::RightOfOperator(0b1010).bitor(0b1100), 0b1110);
        k9::assert_equal!(BinOpSide::LeftOfOperator(0b1010).bitor(0b1100), 0b1110);
    }

    #[test]
    fn binopside_bitxor() {
        k9::assert_equal!(BinOpSide::RightOfOperator(0b1010).bitxor(0b1100), 0b0110);
        k9::assert_equal!(BinOpSide::LeftOfOperator(0b1010).bitxor(0b1100), 0b0110);
    }

    #[test]
    fn binopside_lt() {
        // self=5, other=10 RightOfOperator → 5 < 10 = true
        k9::assert_equal!(BinOpSide::RightOfOperator(10).impl_lt(5), true);
        // self=5, other=3 RightOfOperator → 5 < 3 = false
        k9::assert_equal!(BinOpSide::RightOfOperator(3).impl_lt(5), false);
        // self=5, other=3 LeftOfOperator → 3 < 5 = true
        k9::assert_equal!(BinOpSide::LeftOfOperator(3).impl_lt(5), true);
        // self=5, other=10 LeftOfOperator → 10 < 5 = false
        k9::assert_equal!(BinOpSide::LeftOfOperator(10).impl_lt(5), false);
    }

    #[test]
    fn binopside_le() {
        k9::assert_equal!(BinOpSide::RightOfOperator(5).impl_le(5), true);
        k9::assert_equal!(BinOpSide::RightOfOperator(3).impl_le(5), false);
        k9::assert_equal!(BinOpSide::LeftOfOperator(5).impl_le(5), true);
        k9::assert_equal!(BinOpSide::LeftOfOperator(10).impl_le(5), false);
    }

    #[test]
    fn binopside_eq_op() {
        k9::assert_equal!(BinOpSide::RightOfOperator(5).impl_eq(5), true);
        k9::assert_equal!(BinOpSide::RightOfOperator(3).impl_eq(5), false);
        k9::assert_equal!(BinOpSide::LeftOfOperator(5).impl_eq(5), true);
        k9::assert_equal!(BinOpSide::LeftOfOperator(3).impl_eq(5), false);
    }
}
