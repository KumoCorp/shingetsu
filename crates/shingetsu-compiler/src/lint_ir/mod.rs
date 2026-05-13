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

/// Unary operator kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    Len,
    BitNot,
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
    /// `callee(args)`.  `has_trailing_multret` is `true` when the
    /// last arg is itself a call or `...`, so lints that care about
    /// multret semantics don't need to re-inspect the last element.
    FunctionCall {
        callee: Box<Expr>,
        args: Vec<Expr>,
        has_trailing_multret: bool,
    },
    /// `receiver:method(args)`.  `method_span` covers just the
    /// method-name token so diagnostics can anchor on it.
    MethodCall {
        receiver: Box<Expr>,
        method: Bytes,
        method_span: Span,
        args: Vec<Expr>,
        has_trailing_multret: bool,
    },
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

#[derive(Debug, Clone)]
pub enum StmtKind {
    /// Multi-target assignment: `a, b = x, y`.
    Assign {
        targets: Vec<Expr>,
        values: Vec<Expr>,
    },
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
