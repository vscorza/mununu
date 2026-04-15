//! AST types for the supported SystemVerilog subset.
//!
//! Covers: module declarations, always_ff/always_comb blocks, case/if-else,
//! typedef enum, basic expressions, and inline `@mununu` property comments.

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
        target: String,
        value: Expr,
    },
    BlockingAssign {
        target: String,
        value: Expr,
    },
}

/// A branch in a case statement.
#[derive(Debug, Clone)]
pub struct CaseBranch {
    pub label: String,
    pub body: Statement,
}

/// A simple expression.
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Eq,
    Ne,
    And,
    Or,
    BitOr,
    Add,
    Sub,
}

/// Continuous assignment: `assign x = expr;`
#[derive(Debug, Clone)]
pub struct ContinuousAssign {
    pub target: String,
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
