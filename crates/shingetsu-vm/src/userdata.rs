use std::ops::{Add, BitAnd, BitOr, BitXor, Div, Mul, Rem, Shl, Shr, Sub};
use std::sync::Arc;

use crate::byte_string::Bytes;
use downcast_rs::DowncastSync;

use crate::call_context::CallContext;
use crate::error::VmError;
use crate::types::LuaType;
use crate::value::{Value, ValueVec};

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

    /// Implements left shift with correct operand ordering via [`std::ops::Shl`].
    ///
    /// Named `impl_shl` so that `other.impl_shl(self.0)` reads as
    /// "implement shift-left against self" rather than "other << self".
    pub fn impl_shl<S>(self, self_val: S) -> S::Output
    where
        S: Shl<T>,
        T: Shl<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val << other,
            BinOpSide::LeftOfOperator(other) => other << self_val,
        }
    }

    /// Implements right shift with correct operand ordering via [`std::ops::Shr`].
    ///
    /// Named `impl_shr` so that `other.impl_shr(self.0)` reads as
    /// "implement shift-right against self" rather than "other >> self".
    pub fn impl_shr<S>(self, self_val: S) -> S::Output
    where
        S: Shr<T>,
        T: Shr<S, Output = S::Output>,
    {
        match self {
            BinOpSide::RightOfOperator(other) => self_val >> other,
            BinOpSide::LeftOfOperator(other) => other >> self_val,
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
/// Implementors should also call `impl_downcast!(sync YourType)` (or use the
/// `#[derive(UserData)]` macro which does this automatically) to enable
/// downcasting from `Arc<dyn Userdata>` back to `Arc<YourType>`.
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
        LuaType::Named(Bytes::from(self.type_name().as_bytes()))
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
    fn binopside_shl() {
        // self=1, other=2 RightOfOperator → 1 << 2 = 4
        k9::assert_equal!(BinOpSide::RightOfOperator(2).impl_shl(1), 4);
        // self=1, other=2 LeftOfOperator → 2 << 1 = 4
        k9::assert_equal!(BinOpSide::LeftOfOperator(2).impl_shl(1), 4);
        // self=1, other=3 RightOfOperator → 1 << 3 = 8
        k9::assert_equal!(BinOpSide::RightOfOperator(3).impl_shl(1), 8);
        // self=3, other=1 LeftOfOperator → 1 << 3 = 8
        k9::assert_equal!(BinOpSide::LeftOfOperator(1).impl_shl(3), 8);
    }

    #[test]
    fn binopside_shr() {
        k9::assert_equal!(BinOpSide::RightOfOperator(1).impl_shr(8), 4);
        k9::assert_equal!(BinOpSide::LeftOfOperator(1).impl_shr(8), 0);
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
