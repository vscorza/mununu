//! AST types for the supported SystemVerilog subset.
//!
//! Covers: module declarations, always_ff/always_comb blocks, case/if-else,
//! typedef enum, basic expressions, and inline `@mununu` property/annotation comments.

/// A SystemVerilog module.
#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    /// Module parameters from `#(parameter N = 4)`.
    pub parameters: Vec<Parameter>,
    pub ports: Vec<Port>,
    pub declarations: Vec<Declaration>,
    pub always_blocks: Vec<AlwaysBlock>,
    pub assigns: Vec<ContinuousAssign>,
    /// Properties extracted from `// @mununu` comments.
    pub mununu_properties: Vec<MununuProperty>,
    /// Domain annotations from `// @mununu domain` comments.
    pub domain_annotations: Vec<MununuDomainAnnotation>,
    /// Signals annotated as controllable via `// @mununu controllable`.
    pub controllable_signals: Vec<String>,
    /// Signals annotated as input (uncontrollable) via `// @mununu input`.
    pub input_signals: Vec<String>,
    /// Whether Kripke mode is forced via `// @mununu mode kripke`.
    pub force_kripke: bool,
    /// Module instantiations (sub-module instances with port bindings).
    /// Used by `sv init --multi` to derive connections from a top module.
    pub instantiations: Vec<ModuleInstantiation>,
}

/// A module instantiation: `module_type instance_name(.port(wire), ...);`
#[derive(Debug, Clone)]
pub struct ModuleInstantiation {
    /// The module type being instantiated (e.g., "axilite_master").
    pub module_type: String,
    /// The instance name (e.g., "master_inst").
    pub instance_name: String,
    /// Port connections: `.port_name(signal_name)`.
    pub port_connections: Vec<PortConnection>,
}

/// A named port connection in a module instantiation.
#[derive(Debug, Clone)]
pub struct PortConnection {
    /// Port name on the sub-module (e.g., "m_axi_awvalid").
    pub port_name: String,
    /// Signal/wire name in the parent module (e.g., "awvalid").
    pub signal_name: String,
}

/// A module parameter.
#[derive(Debug, Clone)]
pub struct Parameter {
    pub name: String,
    pub default_value: i64,
}

/// A module port.
#[derive(Debug, Clone)]
pub struct Port {
    pub name: String,
    pub direction: PortDirection,
    pub width: usize, // 1 = single bit
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortDirection {
    Input,
    Output,
    Inout,
}

/// A declaration inside a module.
#[derive(Debug, Clone)]
pub enum Declaration {
    Enum {
        name: String,
        variants: Vec<String>,
        var_name: Option<String>,
    },
    Logic {
        name: String,
        width: usize,
    },
}

/// An always block.
#[derive(Debug, Clone)]
pub enum AlwaysBlock {
    AlwaysFF {
        reset: Option<ResetInfo>,
        body: Statement,
    },
    AlwaysComb {
        body: Statement,
    },
}

/// Reset information extracted from `if (rst) ... else ...` pattern.
#[derive(Debug, Clone)]
pub struct ResetInfo {
    pub reset_signal: String,
    pub assignments: Vec<(String, String)>, // (target, value)
}

/// The left-hand side of an assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignTarget {
    /// A simple register name: `count <= value`
    Simple(String),
    /// A bit-slice of a register: `var[msb:lsb] <= value`
    /// Used for struct field writes resolved at parse time.
    BitSlice {
        base: String,
        msb: usize,
        lsb: usize,
    },
}

impl AssignTarget {
    /// Returns the base register name regardless of the target kind.
    pub fn name(&self) -> &str {
        match self {
            AssignTarget::Simple(name) => name,
            AssignTarget::BitSlice { base, .. } => base,
        }
    }
}

impl PartialEq<&str> for AssignTarget {
    fn eq(&self, other: &&str) -> bool {
        self.name() == *other
    }
}

/// A statement in an always block.
#[derive(Debug, Clone)]
pub enum Statement {
    If {
        cond: Expr,
        then_branch: Box<Statement>,
        else_branch: Option<Box<Statement>>,
    },
    Case {
        selector: String,
        branches: Vec<CaseBranch>,
        default: Option<Box<Statement>>,
    },
    Block(Vec<Statement>),
    NonblockingAssign {
        target: AssignTarget,
        value: Expr,
    },
    BlockingAssign {
        target: AssignTarget,
        value: Expr,
    },
}

/// A branch in a case statement.
#[derive(Debug, Clone)]
pub struct CaseBranch {
    pub label: String,
    pub body: Statement,
}

/// An expression in the supported SystemVerilog subset.
#[derive(Debug, Clone)]
pub enum Expr {
    Ident(String),
    Number(i64),
    Not(Box<Expr>),
    BinOp {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    /// Boolean literal or comparison used as a condition
    Bool(bool),
    /// Ternary: `cond ? then_expr : else_expr`
    Ternary {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    /// Single-bit select: `x[i]`
    BitSelect {
        base: Box<Expr>,
        index: Box<Expr>,
    },
    /// Bit-range slice: `x[msb:lsb]`
    BitSlice {
        base: Box<Expr>,
        msb: Box<Expr>,
        lsb: Box<Expr>,
    },
    /// Concatenation: `{a, b, c}`
    Concat(Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    And,
    Or,
    BitOr,
    BitAnd,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Shl,
    Shr,
}

/// Continuous assignment: `assign x = expr;`
#[derive(Debug, Clone)]
pub struct ContinuousAssign {
    pub target: AssignTarget,
    pub value: Expr,
}

/// A property annotation from `// @mununu` comments.
#[derive(Debug, Clone)]
pub struct MununuProperty {
    pub kind: MununuPropertyKind,
    pub name: String,
    pub formula: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MununuPropertyKind {
    Ltl,
    Assume,
    Guarantee,
}

/// A domain annotation from `// @mununu domain <register>: <kind>`.
#[derive(Debug, Clone)]
pub struct MununuDomainAnnotation {
    pub register_name: String,
    pub domain_kind: DomainAnnotationKind,
}

/// The domain kind specified in a `// @mununu domain` annotation.
#[derive(Debug, Clone)]
pub enum DomainAnnotationKind {
    /// 1-bit boolean.
    Boolean,
    /// Bounded counter with explicit range.
    BoundedCounter { lower: i64, upper: i64 },
    /// Named enum variants, optionally with concrete value mappings.
    ///
    /// Syntax: `enum {IDLE, BUSY, DONE}` or `enum {IDLE=0, START=3, STOP=255, OTHER}`
    ///
    /// When value mappings are present, the last variant without `=` acts as
    /// catch-all for unmapped values. The `value_map` stores `(variant_name, value)`.
    Enum {
        variants: Vec<String>,
        /// Concrete value → variant mapping. Empty if no `=` used.
        value_map: Vec<(String, i64)>,
    },
    /// Excluded from state space.
    Ignored,
}
