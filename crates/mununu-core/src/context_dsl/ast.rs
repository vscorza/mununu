use crate::context_dsl::token::Span;
use crate::ltl::LtlFormula;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub struct ContextDoc {
    pub name: Ident,
    pub alphabet: Vec<AlphabetEntry>,
    pub constants: Vec<ConstantEntry>,
    pub ranges: Vec<RangeEntry>,
    pub enums: Vec<EnumDecl>,
    pub automata: Vec<Automaton>,
    pub compositions: Vec<Composition>,
    pub controllers: Vec<Controller>,
    pub mu_formulas: Vec<MuFormula>,
    pub span: Span,
    /// Side-channel structured state valuations from adapter cross-product enumeration.
    /// Keyed by `automaton_name → state_name → { variable → display_value }`.
    /// Not encoded in CTXDSL text — injected after parsing by adapter callers.
    pub state_valuations: HashMap<String, HashMap<String, BTreeMap<String, String>>>,
}

#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: Ident,
    pub variants: Vec<Ident>,
}

#[derive(Debug, Clone, Default)]
pub struct ControllerOptions {
    pub minimize: Option<bool>,
    pub diagnostics: Option<DiagnosticsConfig>,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticsConfig {
    pub counterexample: Option<bool>,
    pub deadlock_traces: Option<bool>,
    pub max_counter_traces: Option<u32>,
    pub proof_obligations: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

impl Ident {
    pub fn new(name: String, span: Span) -> Self {
        Self { name, span }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub id: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlphabetEntry {
    pub name: Ident,
    pub display: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConstantEntry {
    pub name: Ident,
    pub value: i64,
}

#[derive(Debug, Clone)]
pub struct RangeEntry {
    pub name: Ident,
    pub lower: Expr,
    pub upper: Expr,
}

#[derive(Debug, Clone)]
pub struct Automaton {
    pub name: Ident,
    pub meta: Meta,
    pub parameters: Vec<Parameter>,
    pub alphabet: Vec<AlphabetRef>,
    pub controllable: Vec<AlphabetRef>,
    pub internal: Vec<AlphabetRef>,
    pub controllable_declared: bool,
    pub internal_declared: bool,
    pub variables: Vec<VariableDecl>,
    pub state_groups: Vec<StateGroup>,
    pub states: Vec<StateDecl>,
    pub transitions: Vec<TransitionDecl>,
    pub predicates: Vec<PredicateDecl>,
}

#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: Ident,
    pub spec: RangeSpec,
}

#[derive(Debug, Clone)]
pub enum RangeSpec {
    Named(Ident),
    Bounds { lower: Expr, upper: Expr },
}

#[derive(Debug, Clone)]
pub struct AlphabetRef {
    pub name: Ident,
    pub index: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct VariableDecl {
    pub name: Ident,
    pub index: Option<Expr>,
    pub ty: TypeName,
    pub init: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeName {
    Bool,
    I64,
    Enum(String),
}

#[derive(Debug, Clone)]
pub struct StateDecl {
    pub name: Ident,
    pub index: Option<StateIndexSpec>,
    pub is_initial: bool,
    pub overrides: Vec<Assignment>,
}

#[derive(Debug, Clone)]
pub struct StateGroup {
    pub name: Ident,
    pub members: Vec<StateSelector>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StateIndexSpec {
    Range { symbol: Ident, range: Ident },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct TransitionDecl {
    pub source: StateSelector,
    pub target: StateSelector,
    pub label: TransitionLabel,
    pub additional_labels: Vec<TransitionLabel>,
    pub guard: Option<Expr>,
    pub effects: Vec<Assignment>,
}

#[derive(Debug, Clone)]
pub struct PredicateDecl {
    pub name: Ident,
    pub target: PredicateTarget,
}

#[derive(Debug, Clone)]
pub enum PredicateTarget {
    State(StateRef),
}

#[derive(Debug, Clone)]
pub enum StateSelector {
    Named(StateRef),
    Group(Ident),
    Wildcard(WildcardPattern),
}

#[derive(Debug, Clone)]
pub struct WildcardPattern {
    pub pattern: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StateRef {
    Simple(Ident),
    Indexed { name: Ident, indices: Vec<Expr> },
}

#[derive(Debug, Clone)]
pub enum TransitionLabel {
    Named { name: Ident, index: Option<Expr> },
    Epsilon(Span),
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub target: Ident,
    pub expr: Expr,
}

#[derive(Debug, Clone)]
pub struct MemberRef {
    pub name: Ident,
    pub index: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct Composition {
    pub name: Ident,
    pub meta: Meta,
    pub kind: CompositionKind,
    pub members: Vec<MemberRef>,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionKind {
    Synchronous,
    Asynchronous,
    Superset,
}

#[derive(Debug, Clone)]
pub struct Controller {
    pub name: Ident,
    pub meta: Meta,
    pub source: Ident,
    pub formula: Ident,
    pub export: Option<String>,
    pub options: ControllerOptions,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct MuFormula {
    pub name: Ident,
    pub meta: Meta,
    pub targets: FormulaTargets,
    pub body: FormulaExpr,
}

#[derive(Debug, Clone)]
pub enum FormulaTargets {
    All(Span),
    Named(Vec<Ident>),
}

/// Formula expression that can be either μ-calculus or LTL.
#[derive(Debug, Clone)]
pub enum FormulaExpr {
    /// Existing μ-calculus syntax
    MuCalculus(MuExpr),
    /// New LTL syntax
    Ltl(LtlExpr),
}

/// μ-calculus expression (raw string representation).
#[derive(Debug, Clone)]
pub struct MuExpr {
    pub raw: String,
    pub span: Span,
}

/// LTL expression with parsed formula and source span.
#[derive(Debug, Clone)]
pub struct LtlExpr {
    pub formula: LtlFormula,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Integer(i64),
    Ident(Ident),
    Index {
        target: Ident,
        expr: Box<Expr>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
    },
    Group(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}
