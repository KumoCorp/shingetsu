//! Lowered AST used by lint plugins.
//!
//! The lint IR is a stable, sugar-removed tree built from the full_moon
//! parse tree.  Plugin authors see it (via Lua userdata wrappers) and
//! depend on its shape, so its public surface is treated as a versioned
//! contract: additive changes bump a schema number; renames and removals
//! are breaking changes.
//!
//! Design points worth knowing before touching this file:
//!
//! - Method calls (`r:m(a, b)`) and dot-calls (`f(a, b)`) are distinct
//!   node kinds.  The receiver is not folded into the args list.
//! - Index access via brackets (`t[k]`) and via field (`t.k`) are
//!   separate kinds so lints can see which spelling the source used.
//! - Parentheses do not get a dedicated node.  The inner expression's
//!   [`Expr::was_parenthesized`] flag records the wrap.
//! - Statement-vs-expression is a strict split.  A call used as a
//!   statement becomes an [`StmtKind::ExprStatement`] wrapping a
//!   [`ExprKind::FunctionCall`] or [`ExprKind::MethodCall`].
//! - Spans carry both byte and line/column extents so plugins can
//!   reason about positioning without re-parsing.  The byte fields
//!   are authoritative; line/column are convenience.
//! - Type-bearing syntactic constructs (`expr :: T`, `type X = ...`,
//!   function return annotations, generics) are modelled as IR
//!   nodes; the type *body* itself stays as a source-string
//!   [`TypeAnnotation`] rather than a parallel type AST.  Plugins
//!   that need semantic type information go through `ctx.type_of`.

pub mod lower;
pub use lower::{lower as lower_ast, Lowered, UnsupportedNode};

use crate::error::SourceLocation;
use shingetsu_vm::Bytes;
use std::sync::Arc;

// Re-export so callers in shingetsu-vm tests can reach these without
// spelling out the full path.
pub use shingetsu_vm::Ud;

/// Source-range span for a lint-IR node.
///
/// Byte offsets are authoritative; line/column are convenience for
/// renderers and plugin authors who think in source coordinates.
///
/// Debug format is intentionally compact:
/// `Span(start_line:start_col..end_line:end_col start_byte..end_byte)`.
/// The verbose field-by-field form would dominate any node
/// snapshot, drowning out the structural shape the tests actually
/// care about.  The compact form fits each span on one line and
/// makes diffs immediately readable.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

impl std::fmt::Debug for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Span({}:{}..{}:{} {}..{})",
            self.start_line,
            self.start_col,
            self.end_line,
            self.end_col,
            self.start_byte,
            self.end_byte,
        )
    }
}

#[shingetsu_derive::userdata(crate = "shingetsu_vm", rename = "Span", index_fallback = "nil")]
impl Span {
    /// `true` if this span fully contains `other`.
    pub fn contains(&self, other: &Span) -> bool {
        self.start_byte <= other.start_byte && other.end_byte <= self.end_byte
    }

    /// Convert to a [`SourceLocation`] anchored at the span's start.
    /// The compiler's diagnostic pipeline accepts these, so plugin
    /// `ctx.warn(span, ...)` calls round-trip into the standard
    /// rendered output.
    pub fn to_source_location(&self, source_name: &Arc<String>) -> SourceLocation {
        SourceLocation {
            source_name: Arc::clone(source_name),
            line: self.start_line,
            column: self.start_col,
            byte_offset: self.start_byte,
            byte_len: self.end_byte.saturating_sub(self.start_byte),
        }
    }

    #[lua_field]
    fn start_line(&self) -> i64 {
        self.start_line as i64
    }
    #[lua_field]
    fn start_col(&self) -> i64 {
        self.start_col as i64
    }
    #[lua_field]
    fn end_line(&self) -> i64 {
        self.end_line as i64
    }
    #[lua_field]
    fn end_col(&self) -> i64 {
        self.end_col as i64
    }
    #[lua_field]
    fn start_byte(&self) -> i64 {
        self.start_byte as i64
    }
    #[lua_field]
    fn end_byte(&self) -> i64 {
        self.end_byte as i64
    }
}

/// Top-level chunk.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub block: Block,
    pub span: Span,
}

/// A sequence of statements.  Used for chunk bodies, loop bodies, and
/// `if`/`elseif`/`else` branch bodies.
#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

/// Binary operator kinds.  Each variant carries no operands; the
/// surrounding [`ExprKind::BinOp`] holds the lhs/rhs and the
/// operator's own span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    Pow,
    Concat,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

impl BinOp {
    pub fn as_str(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::FloorDiv => "//",
            BinOp::Mod => "%",
            BinOp::Pow => "^",
            BinOp::Concat => "..",
            BinOp::Eq => "==",
            BinOp::NotEq => "~=",
            BinOp::Lt => "<",
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "~",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        }
    }
}

/// Unary operator kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Len,
    BitNot,
}

impl UnOp {
    pub fn as_str(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "not",
            UnOp::Len => "#",
            UnOp::BitNot => "~",
        }
    }
}

/// An expression.  Carries its source span and a parenthesization
/// flag.  Parentheses themselves are not nodes -- the inner
/// expression's [`Self::was_parenthesized`] records the wrap, which
/// is sufficient for the lints that care (parenthese_conditions and
/// similar).
#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
    pub was_parenthesized: bool,
}

#[shingetsu_derive::userdata(crate = "shingetsu_vm", rename = "Expr", index_fallback = "nil")]
impl Expr {
    /// The expression kind as a snake_case discriminant string.
    /// Plugins switch on this rather than introspecting the
    /// underlying enum.
    #[lua_field]
    fn kind(&self) -> Bytes {
        match &self.kind {
            ExprKind::StringLiteral { .. } => "string_literal",
            ExprKind::InterpString { .. } => "interp_string",
            ExprKind::NumberLiteral { .. } => "number_literal",
            ExprKind::BoolLiteral(_) => "bool_literal",
            ExprKind::Nil => "nil",
            ExprKind::Vararg => "vararg",
            ExprKind::Name { .. } => "name",
            ExprKind::BinOp { .. } => "binop",
            ExprKind::UnOp { .. } => "unop",
            ExprKind::FunctionCall(_) => "function_call",
            ExprKind::MethodCall(_) => "method_call",
            ExprKind::Index { .. } => "index",
            ExprKind::Field { .. } => "field",
            ExprKind::TableConstructor { .. } => "table_constructor",
            ExprKind::FunctionExpr { .. } => "function_expr",
            ExprKind::TypeAssertion { .. } => "type_assertion",
            ExprKind::IfExpression { .. } => "if_expression",
        }
        .into()
    }

    #[lua_field]
    fn span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.span))
    }

    #[lua_field]
    fn was_parenthesized(&self) -> bool {
        self.was_parenthesized
    }

    /// For `name` kind: the identifier text.
    #[lua_field]
    fn name(&self) -> Option<Bytes> {
        match &self.kind {
            ExprKind::Name { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// For `name` kind: `true` when this is a global reference.
    #[lua_field]
    fn is_global(&self) -> bool {
        matches!(
            &self.kind,
            ExprKind::Name {
                is_global: true,
                ..
            }
        )
    }

    /// For `name` kind: `true` when this is a local reference.
    #[lua_field]
    fn is_local(&self) -> bool {
        matches!(&self.kind, ExprKind::Name { is_local: true, .. })
    }

    /// For `name` kind: the compiler-assigned binding id for
    /// resolved locals; `nil` for globals.
    #[lua_field]
    fn binding_id(&self) -> Option<i64> {
        match &self.kind {
            ExprKind::Name { binding_id, .. } => binding_id.map(|id| id as i64),
            _ => None,
        }
    }

    /// For `string_literal` kind: the post-escape byte sequence.
    #[lua_field]
    fn string_value(&self) -> Option<Bytes> {
        match &self.kind {
            ExprKind::StringLiteral { value, .. } => Some(value.clone()),
            _ => None,
        }
    }

    /// For `string_literal` / `number_literal` kinds: the literal
    /// source spelling (before escape processing / normalisation).
    #[lua_field]
    fn raw(&self) -> Option<Bytes> {
        match &self.kind {
            ExprKind::StringLiteral { raw, .. } => Some(raw.clone()),
            ExprKind::NumberLiteral { raw, .. } => Some(raw.as_str().into()),
            _ => None,
        }
    }

    /// For `number_literal` kind: the parsed numeric value.
    #[lua_field]
    fn number_value(&self) -> Option<f64> {
        match &self.kind {
            ExprKind::NumberLiteral { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// For `bool_literal` kind: the boolean value.
    #[lua_field]
    fn bool_value(&self) -> Option<bool> {
        match &self.kind {
            ExprKind::BoolLiteral(b) => Some(*b),
            _ => None,
        }
    }

    /// For `binop` / `unop` kinds: the operator as a source string
    /// (`"+"`, `"and"`, `"not"`, etc.).
    #[lua_field]
    fn op(&self) -> Option<Bytes> {
        match &self.kind {
            ExprKind::BinOp { op, .. } => Some(op.as_str().into()),
            ExprKind::UnOp { op, .. } => Some(op.as_str().into()),
            _ => None,
        }
    }

    /// For `binop` / `unop` kinds: span covering just the operator
    /// token, for tight diagnostic anchoring.
    #[lua_field]
    fn op_span(&self) -> Option<shingetsu_vm::Ud<Span>> {
        match &self.kind {
            ExprKind::BinOp { op_span, .. } | ExprKind::UnOp { op_span, .. } => {
                Some(shingetsu_vm::Ud(Arc::new(*op_span)))
            }
            _ => None,
        }
    }

    /// For `binop` kind: the left-hand operand.
    #[lua_field]
    fn lhs(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            ExprKind::BinOp { lhs, .. } => Some(shingetsu_vm::Ud(Arc::new(*lhs.clone()))),
            _ => None,
        }
    }

    /// For `binop` kind: the right-hand operand.
    #[lua_field]
    fn rhs(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            ExprKind::BinOp { rhs, .. } => Some(shingetsu_vm::Ud(Arc::new(*rhs.clone()))),
            _ => None,
        }
    }

    /// For `unop` kind: the single operand.
    #[lua_field]
    fn operand(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            ExprKind::UnOp { operand, .. } => Some(shingetsu_vm::Ud(Arc::new(*operand.clone()))),
            _ => None,
        }
    }

    /// For `function_call` kind: the callee expression.
    #[lua_field]
    fn callee(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            ExprKind::FunctionCall(fc) => Some(shingetsu_vm::Ud(Arc::new(*fc.callee.clone()))),
            _ => None,
        }
    }

    /// For `function_call` / `method_call` kinds: the argument list.
    #[lua_field]
    fn args(&self) -> Vec<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            ExprKind::FunctionCall(fc) => fc
                .args
                .iter()
                .cloned()
                .map(|e| shingetsu_vm::Ud(Arc::new(e)))
                .collect(),
            ExprKind::MethodCall(mc) => mc
                .args
                .iter()
                .cloned()
                .map(|e| shingetsu_vm::Ud(Arc::new(e)))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// For `method_call` kind: the receiver expression.
    #[lua_field]
    fn receiver(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            ExprKind::MethodCall(mc) => Some(shingetsu_vm::Ud(Arc::new(*mc.receiver.clone()))),
            _ => None,
        }
    }

    /// For `method_call` kind: the method name.
    #[lua_field]
    fn method(&self) -> Option<Bytes> {
        match &self.kind {
            ExprKind::MethodCall(mc) => Some(mc.method.clone()),
            _ => None,
        }
    }

    /// For `method_call` kind: span covering just the method name
    /// token.
    #[lua_field]
    fn method_span(&self) -> Option<shingetsu_vm::Ud<Span>> {
        match &self.kind {
            ExprKind::MethodCall(mc) => Some(shingetsu_vm::Ud(Arc::new(mc.method_span))),
            _ => None,
        }
    }

    /// For `index` / `field` kinds: the base expression being
    /// indexed.
    #[lua_field]
    fn target(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            ExprKind::Index { target, .. } | ExprKind::Field { target, .. } => {
                Some(shingetsu_vm::Ud(Arc::new(*target.clone())))
            }
            _ => None,
        }
    }

    /// For `index` kind: the bracket key expression.
    #[lua_field]
    fn key(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            ExprKind::Index { key, .. } => Some(shingetsu_vm::Ud(Arc::new(*key.clone()))),
            _ => None,
        }
    }

    /// For `field` kind: the identifier on the right of the dot.
    #[lua_field]
    fn field_name(&self) -> Option<Bytes> {
        match &self.kind {
            ExprKind::Field { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// For `field` kind: span covering just the field identifier.
    #[lua_field]
    fn field_span(&self) -> Option<shingetsu_vm::Ud<Span>> {
        match &self.kind {
            ExprKind::Field { name_span, .. } => Some(shingetsu_vm::Ud(Arc::new(*name_span))),
            _ => None,
        }
    }

    /// For `function_expr` kind: the parameter list.
    #[lua_field]
    fn params(&self) -> Vec<shingetsu_vm::Ud<Param>> {
        match &self.kind {
            ExprKind::FunctionExpr { params, .. } => params
                .iter()
                .cloned()
                .map(|p| shingetsu_vm::Ud(Arc::new(p)))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// For `function_expr` kind: `true` when the function accepts
    /// a variadic argument (`...`).
    #[lua_field]
    fn is_variadic(&self) -> bool {
        matches!(
            &self.kind,
            ExprKind::FunctionExpr {
                is_variadic: true,
                ..
            }
        )
    }

    /// For `table_constructor` kind: the entries.
    #[lua_field]
    fn entries(&self) -> Vec<shingetsu_vm::Ud<TableEntry>> {
        match &self.kind {
            ExprKind::TableConstructor { entries } => entries
                .iter()
                .cloned()
                .map(|e| shingetsu_vm::Ud(Arc::new(e)))
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    /// String literal -- `"hello"`, `'x'`, `[[long]]`, or
    /// `[=[deep]=]`.  `raw` is the content between delimiters with
    /// escape sequences left intact (`a\\n`); `value` is the
    /// post-escape byte sequence (`a` then a real newline).  For
    /// long-bracket strings the two are identical (no escape
    /// processing).  `long_depth` is `None` for quoted forms and
    /// `Some(d)` for long-bracket forms with `d` equals-signs:
    /// `[[...]]` is `Some(0)`, `[=[...]=]` is `Some(1)`, and so on.
    StringLiteral {
        raw: Bytes,
        value: Bytes,
        long_depth: Option<u32>,
    },
    /// Luau-style interpolated string: `` `hello {name}` ``.  Parts
    /// alternate between literal text and inline expressions.
    InterpString {
        parts: Vec<InterpPart>,
    },
    /// Numeric literal.  `raw` preserves the source spelling
    /// (`0x1p4`, `1_000`, etc.) so lints can warn on style without
    /// re-parsing.
    NumberLiteral {
        value: f64,
        raw: String,
    },
    BoolLiteral(bool),
    Nil,
    /// The variadic placeholder `...`.
    Vararg,
    /// A name reference.  `is_global` / `is_local` are mutually
    /// exclusive; `binding_id` is `Some` for resolved locals and
    /// `None` for globals (the lint visitor sees this rather than
    /// resolving names itself).
    Name {
        name: Bytes,
        is_global: bool,
        is_local: bool,
        binding_id: Option<u32>,
    },
    BinOp {
        op: BinOp,
        op_span: Span,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    UnOp {
        op: UnOp,
        op_span: Span,
        operand: Box<Expr>,
    },
    /// `callee(args)`.
    FunctionCall(FunctionCall),
    /// `receiver:method(args)`.
    MethodCall(MethodCall),
    /// `target[key]`.
    Index {
        target: Box<Expr>,
        key: Box<Expr>,
    },
    /// `target.name`.  `name_span` covers just the identifier so
    /// diagnostics anchor cleanly on the field.
    Field {
        target: Box<Expr>,
        name: Bytes,
        name_span: Span,
    },
    /// `{ ... }`.
    TableConstructor {
        entries: Vec<TableEntry>,
    },
    /// `function(...) ... end` as an expression.
    FunctionExpr {
        params: Vec<Param>,
        is_variadic: bool,
        generics: Vec<TypeParam>,
        return_type: Option<TypeAnnotation>,
        body: Block,
    },
    /// Luau-style type assertion: `expr :: T`.  The annotation is
    /// the literal source spelling of the right-hand type; semantic
    /// type checks go through `ctx.type_of(expr)`.
    TypeAssertion {
        expr: Box<Expr>,
        annotation: TypeAnnotation,
    },
    /// Luau if-expression: `if cond then a elseif c2 then b else c`.
    /// Distinct from [`StmtKind::If`]: this form yields a value and
    /// is mandatorily total (the `else` branch is always present in
    /// the source).  `else_expr` is therefore not optional.
    IfExpression {
        branches: Vec<ExprBranch>,
        else_expr: Box<Expr>,
    },
}

/// One `if`/`elseif` branch of an if-*expression*.  Mirrors
/// [`Branch`] but holds an expression body instead of a block --
/// the Luau if-expression syntax is single-expression per arm.
#[derive(Debug, Clone)]
pub struct ExprBranch {
    pub cond: Expr,
    pub value: Expr,
}

/// `callee(args)`.  Payload for [`ExprKind::FunctionCall`].
#[derive(Debug, Clone)]
pub struct FunctionCall {
    pub callee: Box<Expr>,
    pub args: Vec<Expr>,
    /// `true` when the last arg is itself a call or `...`, so the
    /// runtime will pass through its full multi-value result
    /// rather than truncating to one value.  Lets lints that care
    /// about multret semantics skip re-inspecting the last
    /// element.
    pub has_trailing_multret: bool,
    pub span: Span,
    /// Doc-comment text inherited from the closest enclosing
    /// statement with a `---` block.  Lowering leaves this `None`
    /// (no enclosing-statement context available at expr lowering
    /// time); the dispatcher fills it in when firing the event.
    pub doc_comment: Option<String>,
}

#[shingetsu_derive::userdata(
    crate = "shingetsu_vm",
    rename = "FunctionCall",
    index_fallback = "nil"
)]
impl FunctionCall {
    /// Discriminant tag matching the event name.
    #[lua_field]
    fn kind(&self) -> Bytes {
        "function_call".into()
    }
    #[lua_field]
    fn span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.span))
    }
    #[lua_field]
    fn has_trailing_multret(&self) -> bool {
        self.has_trailing_multret
    }
    #[lua_field]
    fn doc_comment(&self) -> Option<Bytes> {
        self.doc_comment.as_ref().map(|s| s.as_str().into())
    }
    #[lua_field]
    fn args(&self) -> Vec<shingetsu_vm::Ud<Expr>> {
        self.args
            .iter()
            .cloned()
            .map(|e| shingetsu_vm::Ud(Arc::new(e)))
            .collect()
    }
}

/// `receiver:method(args)`.  Payload for [`ExprKind::MethodCall`].
#[derive(Debug, Clone)]
pub struct MethodCall {
    pub receiver: Box<Expr>,
    pub method: Bytes,
    /// Span covering just the method-name token -- so diagnostics
    /// can anchor on the method rather than the whole call.
    pub method_span: Span,
    pub args: Vec<Expr>,
    pub has_trailing_multret: bool,
    pub span: Span,
    /// Doc-comment text inherited from the closest enclosing
    /// statement with a `---` block.  See [`FunctionCall::doc_comment`].
    pub doc_comment: Option<String>,
}

#[shingetsu_derive::userdata(crate = "shingetsu_vm", rename = "MethodCall", index_fallback = "nil")]
impl MethodCall {
    #[lua_field]
    fn kind(&self) -> Bytes {
        "method_call".into()
    }
    #[lua_field]
    fn method(&self) -> Bytes {
        self.method.clone()
    }
    #[lua_field]
    fn method_span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.method_span))
    }
    #[lua_field]
    fn span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.span))
    }
    #[lua_field]
    fn has_trailing_multret(&self) -> bool {
        self.has_trailing_multret
    }
    #[lua_field]
    fn doc_comment(&self) -> Option<Bytes> {
        self.doc_comment.as_ref().map(|s| s.as_str().into())
    }
    #[lua_field]
    fn args(&self) -> Vec<shingetsu_vm::Ud<Expr>> {
        self.args
            .iter()
            .cloned()
            .map(|e| shingetsu_vm::Ud(Arc::new(e)))
            .collect()
    }
}

/// `a, b = x, y`.  Payload for [`StmtKind::Assign`].
#[derive(Debug, Clone)]
pub struct Assign {
    pub targets: Vec<Expr>,
    pub values: Vec<Expr>,
    pub span: Span,
    /// Doc-comment text on this statement (a `---` block
    /// immediately preceding).  See [`FunctionCall::doc_comment`].
    pub doc_comment: Option<String>,
}

#[shingetsu_derive::userdata(crate = "shingetsu_vm", rename = "Assign", index_fallback = "nil")]
impl Assign {
    #[lua_field]
    fn kind(&self) -> Bytes {
        "assign".into()
    }
    #[lua_field]
    fn span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.span))
    }
    #[lua_field]
    fn doc_comment(&self) -> Option<Bytes> {
        self.doc_comment.as_ref().map(|s| s.as_str().into())
    }
}

/// A piece of an interpolated string: either literal text or an
/// embedded expression.
#[derive(Debug, Clone)]
pub enum InterpPart {
    Literal(Bytes),
    Expr(Expr),
}

/// One entry in a table-constructor literal.
#[derive(Debug, Clone)]
pub struct TableEntry {
    pub span: Span,
    pub kind: TableEntryKind,
}

#[shingetsu_derive::userdata(crate = "shingetsu_vm", rename = "TableEntry", index_fallback = "nil")]
impl TableEntry {
    /// Discriminant: `"array"`, `"named"`, or `"hash"`.
    #[lua_field]
    fn kind(&self) -> Bytes {
        match &self.kind {
            TableEntryKind::Array { .. } => "array",
            TableEntryKind::Named { .. } => "named",
            TableEntryKind::Hash { .. } => "hash",
        }
        .into()
    }

    #[lua_field]
    fn span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.span))
    }

    /// For `"named"` entries (`{ name = value }`), the key as
    /// written.  `nil` for `"array"` / `"hash"`.
    #[lua_field]
    fn name(&self) -> Option<Bytes> {
        match &self.kind {
            TableEntryKind::Named { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// The value expression.  For every entry kind.
    #[lua_field]
    fn value(&self) -> shingetsu_vm::Ud<Expr> {
        let v = match &self.kind {
            TableEntryKind::Array { value }
            | TableEntryKind::Named { value, .. }
            | TableEntryKind::Hash { value, .. } => value.clone(),
        };
        shingetsu_vm::Ud(Arc::new(v))
    }

    /// For `"hash"` entries (`{ [k] = v }`), the key expression.
    /// `nil` for `"array"` / `"named"`.
    #[lua_field]
    fn key(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            TableEntryKind::Hash { key, .. } => Some(shingetsu_vm::Ud(Arc::new(key.clone()))),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TableEntryKind {
    /// Positional entry: `{ value, ... }`.
    Array { value: Expr },
    /// Named-key entry: `{ name = value }`.  `name_span` covers the
    /// identifier so diagnostics can anchor on it.
    Named {
        name: Bytes,
        name_span: Span,
        value: Expr,
    },
    /// Computed-key entry: `{ [expr] = value }`.
    Hash { key: Expr, value: Expr },
}

/// Variable attribute (`<const>` / `<close>` on a `local` binding).
/// Per-name in the IR rather than a flat list on the statement so the
/// model correctly reflects which name owns which attribute (e.g.
/// `local x, y <close> = ...` puts the attribute on `y` only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribute {
    Const,
    Close,
}

/// A function parameter or local-binding name.  Carries an optional
/// Luau-style type annotation, an optional default expression
/// (Luau-only), and an optional variable attribute (Lua 5.4+).
#[derive(Debug, Clone)]
pub struct Param {
    pub name: Bytes,
    pub name_span: Span,
    /// Inline type annotation (`x: number`).  `None` when absent.
    /// Plugins generally prefer `ctx.type_of` over reading this.
    pub type_annotation: Option<TypeAnnotation>,
    /// Default value (`x: number? = 0`).  `None` when absent.
    pub default: Option<Expr>,
    /// `<const>` / `<close>` attribute on a `local` binding.  `None`
    /// for function parameters and unannotated locals.
    pub attribute: Option<Attribute>,
}

#[shingetsu_derive::userdata(crate = "shingetsu_vm", rename = "Param", index_fallback = "nil")]
impl Param {
    #[lua_field]
    fn name(&self) -> Bytes {
        self.name.clone()
    }

    #[lua_field]
    fn name_span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.name_span))
    }

    /// The raw source spelling of the type annotation, e.g.
    /// `"number"`, `"string?"`.  `nil` when no annotation was
    /// written.  Plugins that need resolved type information should
    /// use `ctx.type_of` instead.
    #[lua_field]
    fn type_annotation(&self) -> Option<Bytes> {
        self.type_annotation
            .as_ref()
            .map(|a| a.source.as_str().into())
    }

    /// Default-value expression for Luau optional params
    /// (`x: T? = default`).  `nil` when absent.
    #[lua_field]
    fn default_value(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        self.default
            .as_ref()
            .map(|e| shingetsu_vm::Ud(Arc::new(e.clone())))
    }

    /// For `<const>` / `<close>` bindings: the attribute name as a
    /// string.  `nil` for plain locals and function parameters.
    #[lua_field]
    fn attribute(&self) -> Option<Bytes> {
        self.attribute.map(|a| match a {
            Attribute::Const => Bytes::from("const"),
            Attribute::Close => Bytes::from("close"),
        })
    }
}

/// A generic type parameter declared on a function or type alias,
/// e.g. `function f<T>(...)` or `type Map<K, V> = ...`.
#[derive(Debug, Clone)]
pub struct TypeParam {
    pub name: Bytes,
    pub name_span: Span,
    /// Optional default in Luau: `type Map<K, V = string> = ...`.
    pub default: Option<TypeAnnotation>,
}

/// A type-annotation reference.  Carries the literal source
/// spelling and its span; modelling the full Luau type grammar
/// (unions, intersections, function types, generic instantiations)
/// is out of scope for the lint IR.  Plugins that need semantic
/// type information query `ctx.type_of(node)` instead.
#[derive(Debug, Clone)]
pub struct TypeAnnotation {
    pub source: String,
    pub span: Span,
}

/// A statement.  `doc_comment` carries the raw text of any `---`
/// block immediately preceding this statement (joined with `\n`,
/// comment markers stripped).  Harvested for `local_assign`,
/// `local_function`, `function_decl`, and multi-target `assign`;
/// other statement kinds have `None`.
#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
    pub doc_comment: Option<String>,
}

// The lint IR is short-lived and built once per statement during linting, so
// the size spread between variants is not worth the indirection of boxing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum StmtKind {
    /// Multi-target assignment: `a, b = x, y`.
    Assign(Assign),
    /// `local a, b = x, y`.  Each name's attribute (if any) is
    /// recorded on the [`Param`] itself.
    LocalAssign {
        names: Vec<Param>,
        values: Vec<Expr>,
    },
    /// Luau compound assignment: `a += b`, `t.x *= 2`, `s ..= "!"`.
    /// Kept distinct from [`Self::Assign`] so style lints can flag
    /// the compound form without re-inspecting the operator.  The
    /// target is a single lvalue (Luau forbids multi-target
    /// compound assignment); `op_span` covers the compound
    /// operator token (e.g. `+=`) for diagnostic anchoring.
    CompoundAssign {
        target: Expr,
        op: BinOp,
        op_span: Span,
        value: Expr,
    },
    /// Luau `const x = expr`.  Semantically equivalent to
    /// `local x <const> = expr` (so [`Param::attribute`] is
    /// `Some(Attribute::Const)`) but syntactically distinct, kept
    /// as its own kind so style lints can prefer one spelling.
    ConstAssign {
        name: Param,
        value: Expr,
    },
    /// Luau `const function f(...) ... end`.  Like
    /// [`Self::LocalFunction`] but the binding is implicitly const.
    ConstFunction {
        name: Bytes,
        name_span: Span,
        params: Vec<Param>,
        is_variadic: bool,
        generics: Vec<TypeParam>,
        return_type: Option<TypeAnnotation>,
        body: Block,
    },
    /// `local function f(...) ... end`.
    LocalFunction {
        name: Bytes,
        name_span: Span,
        params: Vec<Param>,
        is_variadic: bool,
        generics: Vec<TypeParam>,
        return_type: Option<TypeAnnotation>,
        body: Block,
    },
    /// `function foo.bar:baz(...) ... end`.  `is_method` is `true`
    /// when the colon-syntax `:` was used (the implicit `self` is
    /// not listed in `params`).
    FunctionDecl {
        target: Expr,
        is_method: bool,
        params: Vec<Param>,
        is_variadic: bool,
        generics: Vec<TypeParam>,
        return_type: Option<TypeAnnotation>,
        body: Block,
    },
    /// Lua 5.5 `global` declaration.  Each name may carry an
    /// optional type annotation when declared as `global x: T`.
    GlobalDecl {
        names: Vec<Bytes>,
        name_spans: Vec<Span>,
        type_annotations: Vec<Option<TypeAnnotation>>,
    },
    /// Luau `type X = Y` / `export type X = Y` declaration.
    TypeAlias {
        name: Bytes,
        name_span: Span,
        generics: Vec<TypeParam>,
        body: TypeAnnotation,
        exported: bool,
    },
    If {
        branches: Vec<Branch>,
        else_block: Option<Block>,
    },
    While {
        cond: Expr,
        block: Block,
    },
    Repeat {
        block: Block,
        cond: Expr,
    },
    NumericFor {
        var: Param,
        start: Expr,
        stop: Expr,
        step: Option<Expr>,
        block: Block,
    },
    GenericFor {
        vars: Vec<Param>,
        exprs: Vec<Expr>,
        block: Block,
    },
    DoBlock {
        block: Block,
    },
    /// `return ...` (chunk- and function-tail return).
    Return {
        values: Vec<Expr>,
    },
    Break,
    Continue,
    Goto {
        label: Bytes,
        label_span: Span,
    },
    Label {
        name: Bytes,
    },
    /// A call expression used as a statement.  The wrapped expression
    /// is always a [`ExprKind::FunctionCall`] or
    /// [`ExprKind::MethodCall`].
    ExprStatement {
        expr: Expr,
    },
}

/// One `if`/`elseif` branch.  An `else` clause is represented by
/// the surrounding statement's [`StmtKind::If::else_block`].
#[derive(Debug, Clone)]
pub struct Branch {
    pub cond: Expr,
    pub block: Block,
}

#[shingetsu_derive::userdata(crate = "shingetsu_vm", rename = "Branch", index_fallback = "nil")]
impl Branch {
    /// The branch condition expression.
    #[lua_field]
    fn cond(&self) -> shingetsu_vm::Ud<Expr> {
        shingetsu_vm::Ud(Arc::new(self.cond.clone()))
    }

    /// Span covering the condition expression (convenience alias for
    /// `branch.cond.span`).
    #[lua_field]
    fn span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.cond.span))
    }
}

#[shingetsu_derive::userdata(crate = "shingetsu_vm", rename = "Stmt", index_fallback = "nil")]
impl Stmt {
    /// The statement kind as a snake_case discriminant string.
    /// Plugins switch on this, not on the underlying enum.
    #[lua_field]
    fn kind(&self) -> Bytes {
        match &self.kind {
            StmtKind::Assign(_) => "assign",
            StmtKind::LocalAssign { .. } => "local_assign",
            StmtKind::CompoundAssign { .. } => "compound_assign",
            StmtKind::ConstAssign { .. } => "const_assign",
            StmtKind::ConstFunction { .. } => "const_function",
            StmtKind::LocalFunction { .. } => "local_function",
            StmtKind::FunctionDecl { .. } => "function_decl",
            StmtKind::GlobalDecl { .. } => "global_decl",
            StmtKind::TypeAlias { .. } => "type_alias",
            StmtKind::If { .. } => "if",
            StmtKind::While { .. } => "while",
            StmtKind::Repeat { .. } => "repeat",
            StmtKind::NumericFor { .. } => "numeric_for",
            StmtKind::GenericFor { .. } => "generic_for",
            StmtKind::DoBlock { .. } => "do_block",
            StmtKind::Return { .. } => "return",
            StmtKind::Break => "break",
            StmtKind::Continue => "continue",
            StmtKind::Goto { .. } => "goto",
            StmtKind::Label { .. } => "label",
            StmtKind::ExprStatement { .. } => "expr_statement",
        }
        .into()
    }

    #[lua_field]
    fn span(&self) -> shingetsu_vm::Ud<Span> {
        shingetsu_vm::Ud(Arc::new(self.span))
    }

    #[lua_field]
    fn doc_comment(&self) -> Option<Bytes> {
        self.doc_comment.as_ref().map(|s| s.as_str().into())
    }

    // ---- local_assign ----

    /// For `local_assign` / `local_function` / `function_decl` /
    /// `const_function` / `numeric_for` kinds: the bound parameter
    /// list.  Empty for other kinds.
    #[lua_field]
    fn params(&self) -> Vec<shingetsu_vm::Ud<Param>> {
        let slice: &[Param] = match &self.kind {
            StmtKind::LocalAssign { names, .. } => names,
            StmtKind::LocalFunction { params, .. }
            | StmtKind::ConstFunction { params, .. }
            | StmtKind::FunctionDecl { params, .. } => params,
            StmtKind::NumericFor { var, .. } => std::slice::from_ref(var),
            StmtKind::GenericFor { vars, .. } => vars,
            _ => &[],
        };
        slice
            .iter()
            .cloned()
            .map(|p| shingetsu_vm::Ud(Arc::new(p)))
            .collect()
    }

    /// For `local_assign` / `return` / `generic_for` kinds: the
    /// right-hand-side expression list.  Empty for other kinds.
    #[lua_field]
    fn values(&self) -> Vec<shingetsu_vm::Ud<Expr>> {
        let slice: &[Expr] = match &self.kind {
            StmtKind::LocalAssign { values, .. } | StmtKind::Return { values } => values,
            StmtKind::GenericFor { exprs, .. } => exprs,
            _ => &[],
        };
        slice
            .iter()
            .cloned()
            .map(|e| shingetsu_vm::Ud(Arc::new(e)))
            .collect()
    }

    // ---- global_decl ----

    /// For `global_decl` kind: the declared names as plain strings.
    /// Use `stmt.params` for `local_assign` (which carries type
    /// info, defaults, and attributes).
    #[lua_field]
    fn names(&self) -> Vec<Bytes> {
        match &self.kind {
            StmtKind::GlobalDecl { names, .. } => names.clone(),
            _ => Vec::new(),
        }
    }

    /// For `global_decl` kind: spans for each declared name.
    #[lua_field]
    fn name_spans(&self) -> Vec<shingetsu_vm::Ud<Span>> {
        match &self.kind {
            StmtKind::GlobalDecl { name_spans, .. } => name_spans
                .iter()
                .map(|s| shingetsu_vm::Ud(Arc::new(*s)))
                .collect(),
            _ => Vec::new(),
        }
    }

    // ---- local_function / const_function / function_decl ----

    /// For `local_function` / `const_function` / `goto` / `label`
    /// kinds: the name as written in source.
    #[lua_field]
    fn name(&self) -> Option<Bytes> {
        match &self.kind {
            StmtKind::LocalFunction { name, .. } | StmtKind::ConstFunction { name, .. } => {
                Some(name.clone())
            }
            StmtKind::Goto { label, .. } => Some(label.clone()),
            StmtKind::Label { name } => Some(name.clone()),
            _ => None,
        }
    }

    /// For `local_function` / `const_function` kinds: span covering
    /// just the function name token.
    #[lua_field]
    fn name_span(&self) -> Option<shingetsu_vm::Ud<Span>> {
        match &self.kind {
            StmtKind::LocalFunction { name_span, .. }
            | StmtKind::ConstFunction { name_span, .. } => {
                Some(shingetsu_vm::Ud(Arc::new(*name_span)))
            }
            _ => None,
        }
    }

    /// For `local_function` / `const_function` / `function_decl`
    /// kinds: `true` when the function declares `...`.
    #[lua_field]
    fn is_variadic(&self) -> bool {
        matches!(
            &self.kind,
            StmtKind::LocalFunction {
                is_variadic: true,
                ..
            } | StmtKind::ConstFunction {
                is_variadic: true,
                ..
            } | StmtKind::FunctionDecl {
                is_variadic: true,
                ..
            }
        )
    }

    /// For `local_function` / `const_function` / `function_decl`
    /// kinds: the raw source spelling of the return type annotation.
    /// `nil` when absent or wrong kind.
    #[lua_field]
    fn return_type(&self) -> Option<Bytes> {
        let rt = match &self.kind {
            StmtKind::LocalFunction { return_type, .. }
            | StmtKind::ConstFunction { return_type, .. }
            | StmtKind::FunctionDecl { return_type, .. } => return_type.as_ref(),
            _ => None,
        };
        rt.map(|a| a.source.as_str().into())
    }

    // ---- function_decl ----

    /// For `function_decl` kind: the target expression (e.g.
    /// `mod.foo` or `T.new`).
    #[lua_field]
    fn target(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            StmtKind::FunctionDecl { target, .. } => {
                Some(shingetsu_vm::Ud(Arc::new(target.clone())))
            }
            _ => None,
        }
    }

    /// For `function_decl` kind: `true` when the colon syntax was
    /// used (`function T:method()`).
    #[lua_field]
    fn is_method(&self) -> bool {
        matches!(
            &self.kind,
            StmtKind::FunctionDecl {
                is_method: true,
                ..
            }
        )
    }

    // ---- if ----

    /// For `if` kind: the `if`/`elseif` branches.
    #[lua_field]
    fn branches(&self) -> Vec<shingetsu_vm::Ud<Branch>> {
        match &self.kind {
            StmtKind::If { branches, .. } => branches
                .iter()
                .cloned()
                .map(|b| shingetsu_vm::Ud(Arc::new(b)))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Convenience: for `if` / `while` / `repeat` kinds, the
    /// primary condition expression (for `if`, the first branch's
    /// condition; `repeat` exposes its trailing condition here
    /// too).
    #[lua_field]
    fn cond(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            StmtKind::If { branches, .. } => branches
                .first()
                .map(|b| shingetsu_vm::Ud(Arc::new(b.cond.clone()))),
            StmtKind::While { cond, .. } | StmtKind::Repeat { cond, .. } => {
                Some(shingetsu_vm::Ud(Arc::new(cond.clone())))
            }
            _ => None,
        }
    }

    // ---- numeric_for ----

    /// For `numeric_for` kind: the loop-variable binding.
    #[lua_field]
    fn var(&self) -> Option<shingetsu_vm::Ud<Param>> {
        match &self.kind {
            StmtKind::NumericFor { var, .. } => Some(shingetsu_vm::Ud(Arc::new(var.clone()))),
            _ => None,
        }
    }

    /// For `numeric_for` kind: the start expression.
    #[lua_field]
    fn start(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            StmtKind::NumericFor { start, .. } => Some(shingetsu_vm::Ud(Arc::new(start.clone()))),
            _ => None,
        }
    }

    /// For `numeric_for` kind: the stop expression.
    #[lua_field]
    fn stop(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            StmtKind::NumericFor { stop, .. } => Some(shingetsu_vm::Ud(Arc::new(stop.clone()))),
            _ => None,
        }
    }

    /// For `numeric_for` kind: the optional step expression.
    #[lua_field]
    fn step(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            StmtKind::NumericFor { step, .. } => {
                step.as_ref().map(|e| shingetsu_vm::Ud(Arc::new(e.clone())))
            }
            _ => None,
        }
    }

    // ---- expr_statement ----

    /// For `expr_statement` kind: the call expression.
    #[lua_field]
    fn expr(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            StmtKind::ExprStatement { expr } => Some(shingetsu_vm::Ud(Arc::new(expr.clone()))),
            _ => None,
        }
    }

    // ---- compound_assign ----

    /// For `compound_assign` kind: the assignment target.
    #[lua_field]
    fn compound_target(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            StmtKind::CompoundAssign { target, .. } => {
                Some(shingetsu_vm::Ud(Arc::new(target.clone())))
            }
            _ => None,
        }
    }

    /// For `compound_assign` kind: the operator as a source string
    /// (e.g. `"+"` for `+=`).
    #[lua_field]
    fn compound_op(&self) -> Option<Bytes> {
        match &self.kind {
            StmtKind::CompoundAssign { op, .. } => Some(op.as_str().into()),
            _ => None,
        }
    }

    /// For `compound_assign` kind: span covering the operator token.
    #[lua_field]
    fn compound_op_span(&self) -> Option<shingetsu_vm::Ud<Span>> {
        match &self.kind {
            StmtKind::CompoundAssign { op_span, .. } => Some(shingetsu_vm::Ud(Arc::new(*op_span))),
            _ => None,
        }
    }

    /// For `compound_assign` / `const_assign` kinds: the right-hand
    /// expression.
    #[lua_field]
    fn value(&self) -> Option<shingetsu_vm::Ud<Expr>> {
        match &self.kind {
            StmtKind::CompoundAssign { value, .. } | StmtKind::ConstAssign { value, .. } => {
                Some(shingetsu_vm::Ud(Arc::new(value.clone())))
            }
            _ => None,
        }
    }

    // ---- goto ----

    /// For `goto` kind: span of the target label.
    #[lua_field]
    fn label_span(&self) -> Option<shingetsu_vm::Ud<Span>> {
        match &self.kind {
            StmtKind::Goto { label_span, .. } => Some(shingetsu_vm::Ud(Arc::new(*label_span))),
            _ => None,
        }
    }
}
