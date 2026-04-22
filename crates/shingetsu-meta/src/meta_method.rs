use std::str::FromStr;

/// All standard Lua/LuaU metamethod names.
///
/// `fn name()` returns the canonical `__xx` string.  `FromStr` parses either
/// the canonical form (`"__index"`) or the short variant name (`"Index"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetaMethod {
    Index,
    NewIndex,
    Call,
    Len,
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
    Unm,
    BAnd,
    BOr,
    BXor,
    BNot,
    Shl,
    Shr,
    Eq,
    Lt,
    Le,
    Concat,
    ToString,
    Gc,
    Close,
    Pairs,
    IPairs,
}

impl MetaMethod {
    /// Whether this is a binary metamethod where the userdata may appear
    /// as either operand (arithmetic, bitwise, comparison, concat).
    pub fn is_binary_op(self) -> bool {
        matches!(
            self,
            Self::Add
                | Self::Sub
                | Self::Mul
                | Self::Div
                | Self::IDiv
                | Self::Mod
                | Self::Pow
                | Self::BAnd
                | Self::BOr
                | Self::BXor
                | Self::Shl
                | Self::Shr
                | Self::Eq
                | Self::Lt
                | Self::Le
                | Self::Concat
        )
    }

    /// Returns the canonical `__xx` metamethod name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Index => "__index",
            Self::NewIndex => "__newindex",
            Self::Call => "__call",
            Self::Len => "__len",
            Self::Add => "__add",
            Self::Sub => "__sub",
            Self::Mul => "__mul",
            Self::Div => "__div",
            Self::IDiv => "__idiv",
            Self::Mod => "__mod",
            Self::Pow => "__pow",
            Self::Unm => "__unm",
            Self::BAnd => "__band",
            Self::BOr => "__bor",
            Self::BXor => "__bxor",
            Self::BNot => "__bnot",
            Self::Shl => "__shl",
            Self::Shr => "__shr",
            Self::Eq => "__eq",
            Self::Lt => "__lt",
            Self::Le => "__le",
            Self::Concat => "__concat",
            Self::ToString => "__tostring",
            Self::Gc => "__gc",
            Self::Close => "__close",
            Self::Pairs => "__pairs",
            Self::IPairs => "__ipairs",
        }
    }
}

impl FromStr for MetaMethod {
    type Err = UnknownMetaMethod;

    /// Parses `"__index"` (canonical) or `"Index"` (short variant name).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "__index" | "Index" => Ok(Self::Index),
            "__newindex" | "NewIndex" => Ok(Self::NewIndex),
            "__call" | "Call" => Ok(Self::Call),
            "__len" | "Len" => Ok(Self::Len),
            "__add" | "Add" => Ok(Self::Add),
            "__sub" | "Sub" => Ok(Self::Sub),
            "__mul" | "Mul" => Ok(Self::Mul),
            "__div" | "Div" => Ok(Self::Div),
            "__idiv" | "IDiv" => Ok(Self::IDiv),
            "__mod" | "Mod" => Ok(Self::Mod),
            "__pow" | "Pow" => Ok(Self::Pow),
            "__unm" | "Unm" => Ok(Self::Unm),
            "__band" | "BAnd" => Ok(Self::BAnd),
            "__bor" | "BOr" => Ok(Self::BOr),
            "__bxor" | "BXor" => Ok(Self::BXor),
            "__bnot" | "BNot" => Ok(Self::BNot),
            "__shl" | "Shl" => Ok(Self::Shl),
            "__shr" | "Shr" => Ok(Self::Shr),
            "__eq" | "Eq" => Ok(Self::Eq),
            "__lt" | "Lt" => Ok(Self::Lt),
            "__le" | "Le" => Ok(Self::Le),
            "__concat" | "Concat" => Ok(Self::Concat),
            "__tostring" | "ToString" => Ok(Self::ToString),
            "__gc" | "Gc" => Ok(Self::Gc),
            "__close" | "Close" => Ok(Self::Close),
            "__pairs" | "Pairs" => Ok(Self::Pairs),
            "__ipairs" | "IPairs" => Ok(Self::IPairs),
            _ => Err(UnknownMetaMethod(s.to_owned())),
        }
    }
}

/// Error returned when parsing an unknown metamethod name.
#[derive(Debug, thiserror::Error)]
#[error("unknown metamethod: {0}")]
pub struct UnknownMetaMethod(pub String);
