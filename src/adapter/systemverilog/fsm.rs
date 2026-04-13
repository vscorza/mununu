//! FSM extraction from parsed SystemVerilog always_ff blocks.
//!
//! Identifies the FSM state variable (from typedef enum + case selector),
//! extracts states and transitions, and determines the initial state from
//! the reset branch.

use super::ast::*;

/// An extracted FSM with named states and transitions.
#[derive(Debug, Clone)]
pub struct ExtractedFsm {
    pub state_var: String,
    pub states: Vec<FsmState>,
    pub transitions: Vec<FsmTransition>,
}

#[derive(Debug, Clone)]
pub struct FsmState {
    pub name: String,
    pub is_initial: bool,
}

#[derive(Debug, Clone)]
pub struct FsmTransition {
    pub source: String,
    pub target: String,
    /// The event label for this transition (derived from guard or state change context).
    pub label: String,
    pub guard: Option<String>,
}

/// Extract an FSM from the module's declarations and always_ff blocks.
pub fn extract_fsm(module: &Module) -> Option<ExtractedFsm> {
    // Step 1: Find the enum declaration (FSM states)
    let (enum_variants, state_var) = find_enum_state_var(module)?;

    // Step 2: Find the always_ff block that drives the state variable
    let (reset, case_stmt) = find_state_driving_always_ff(module, &state_var)?;

    // Step 3: Determine initial state from reset
    let initial_state = reset
        .and_then(|r| {
            r.assignments
                .iter()
                .find(|(t, _)| t == &state_var)
                .map(|(_, v)| v.clone())
        })
        .unwrap_or_else(|| enum_variants.first().cloned().unwrap_or_default());

    // Step 4: Build states
    let states: Vec<FsmState> = enum_variants
        .iter()
        .map(|name| FsmState {
            name: name.clone(),
            is_initial: *name == initial_state,
        })
        .collect();

    // Step 5: Extract transitions from case branches
    let transitions = extract_transitions_from_case(case_stmt, &state_var);

    Some(ExtractedFsm {
        state_var,
        states,
        transitions,
    })
}

/// Find the enum declaration that defines FSM states and its variable.
fn find_enum_state_var(module: &Module) -> Option<(Vec<String>, String)> {
    for decl in &module.declarations {
        if let Declaration::Enum {
            variants,
            var_name: Some(var),
            ..
        } = decl
        {
            return Some((variants.clone(), var.clone()));
        }
    }
    None
}

/// Find the always_ff block that drives the state variable via a case statement.
/// Returns the reset info and the case statement from the else branch.
fn find_state_driving_always_ff<'a>(
    module: &'a Module,
    _state_var: &str,
) -> Option<(Option<&'a ResetInfo>, &'a Statement)> {
    for block in &module.always_blocks {
        if let AlwaysBlock::AlwaysFF { reset, body } = block {
            // Look for case statement in the body (possibly inside if/else reset pattern)
            if let Some(case_stmt) = find_case_in_statement(body) {
                return Some((reset.as_ref(), case_stmt));
            }
        }
    }
    None
}

/// Recursively find a Case statement inside a statement tree.
fn find_case_in_statement(stmt: &Statement) -> Option<&Statement> {
    match stmt {
        Statement::Case { .. } => Some(stmt),
        Statement::If {
            else_branch: Some(else_br),
            ..
        } => find_case_in_statement(else_br),
        Statement::Block(stmts) => stmts.iter().find_map(find_case_in_statement),
        _ => None,
    }
}

/// Extract transitions from a case statement's branches.
fn extract_transitions_from_case(case_stmt: &Statement, state_var: &str) -> Vec<FsmTransition> {
    let mut transitions = Vec::new();

    if let Statement::Case { branches, .. } = case_stmt {
        for branch in branches {
            let source = &branch.label;
            extract_transitions_from_body(&branch.body, source, state_var, None, &mut transitions);
        }
    }

    transitions
}

/// Recursively extract transitions from a statement body within a case branch.
fn extract_transitions_from_body(
    stmt: &Statement,
    source: &str,
    state_var: &str,
    guard: Option<&str>,
    transitions: &mut Vec<FsmTransition>,
) {
    match stmt {
        Statement::NonblockingAssign { target, value } if target == state_var => {
            let target_state = expr_to_state_name(value);
            let label = if let Some(g) = guard {
                format!("{source}_to_{target_state}_when_{g}")
            } else {
                format!("{source}_to_{target_state}")
            };
            transitions.push(FsmTransition {
                source: source.to_string(),
                target: target_state,
                label,
                guard: guard.map(String::from),
            });
        }
        Statement::If {
            cond,
            then_branch,
            else_branch,
        } => {
            let guard_str = expr_to_guard_name(cond);
            extract_transitions_from_body(
                then_branch,
                source,
                state_var,
                Some(&guard_str),
                transitions,
            );
            if let Some(else_br) = else_branch {
                let neg_guard = format!("not_{guard_str}");
                extract_transitions_from_body(
                    else_br,
                    source,
                    state_var,
                    Some(&neg_guard),
                    transitions,
                );
            }
        }
        Statement::Block(stmts) => {
            for s in stmts {
                extract_transitions_from_body(s, source, state_var, guard, transitions);
            }
        }
        _ => {}
    }
}

/// Convert an expression to a state name (for the transition target).
fn expr_to_state_name(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::Number(n) => format!("s{n}"),
        _ => "unknown".to_string(),
    }
}

/// Convert a condition expression to a guard name for labeling.
fn expr_to_guard_name(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::Not(inner) => format!("not_{}", expr_to_guard_name(inner)),
        Expr::BinOp {
            op: BinOp::Eq,
            left,
            right,
        } => format!(
            "{}_eq_{}",
            expr_to_guard_name(left),
            expr_to_guard_name(right)
        ),
        Expr::BinOp { left, .. } => expr_to_guard_name(left),
        Expr::Number(n) => format!("{n}"),
        Expr::Bool(b) => format!("{b}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::systemverilog::parser;

    #[test]
    fn extract_three_state_fsm() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                typedef enum logic [1:0] {IDLE, WAIT, DONE} state_t;
                state_t state;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) state <= IDLE;
                    else case (state)
                        IDLE: state <= WAIT;
                        WAIT: state <= DONE;
                        DONE: state <= IDLE;
                    endcase
                end
            endmodule"#,
        )
        .unwrap();

        let fsm = extract_fsm(&module).expect("should extract FSM");
        assert_eq!(fsm.state_var, "state");
        assert_eq!(fsm.states.len(), 3);
        assert!(fsm.states.iter().any(|s| s.name == "IDLE" && s.is_initial));
        assert_eq!(fsm.transitions.len(), 3);
    }

    #[test]
    fn extract_fsm_with_guarded_transitions() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst, input logic req);
                typedef enum logic [1:0] {IDLE, WAIT, ACTIVE, DONE} state_t;
                state_t state;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) state <= IDLE;
                    else case (state)
                        IDLE: if (req) state <= WAIT;
                        WAIT: state <= ACTIVE;
                        ACTIVE: if (!req) state <= DONE;
                        DONE: state <= IDLE;
                    endcase
                end
            endmodule"#,
        )
        .unwrap();

        let fsm = extract_fsm(&module).expect("should extract FSM");
        assert_eq!(fsm.states.len(), 4);
        // IDLE has a guarded transition (if req)
        let idle_transitions: Vec<_> = fsm
            .transitions
            .iter()
            .filter(|t| t.source == "IDLE")
            .collect();
        assert_eq!(idle_transitions.len(), 1);
        assert!(idle_transitions[0].guard.is_some());
        assert_eq!(idle_transitions[0].target, "WAIT");
    }

    #[test]
    fn extract_initial_from_reset() {
        let module = parser::parse(
            r#"module test(input logic clk, input logic rst);
                typedef enum logic {A, B} state_t;
                state_t state;
                always_ff @(posedge clk or posedge rst) begin
                    if (rst) state <= B;
                    else case (state)
                        A: state <= B;
                        B: state <= A;
                    endcase
                end
            endmodule"#,
        )
        .unwrap();

        let fsm = extract_fsm(&module).expect("should extract FSM");
        // Initial state should be B (from reset)
        assert!(fsm.states.iter().any(|s| s.name == "B" && s.is_initial));
        assert!(fsm.states.iter().any(|s| s.name == "A" && !s.is_initial));
    }
}
