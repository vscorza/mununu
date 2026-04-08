//! CFG (Control Flow Graph) extraction from Promela AST.
//!
//! Converts each process body into a labeled transition system where
//! nodes are program locations and edges carry guards, labels, and effects.

use std::collections::HashMap;

use super::ast::*;

/// A control-flow graph for a single process.
#[derive(Debug, Clone)]
pub struct Cfg {
    /// Process name.
    pub name: String,
    /// Locations (program counter values).
    pub locations: Vec<Location>,
    /// Initial location index.
    pub initial: usize,
    /// Edges.
    pub edges: Vec<CfgEdge>,
}

/// A program location in the CFG.
#[derive(Debug, Clone)]
pub struct Location {
    pub id: usize,
    pub label: String,
}

/// A CFG edge representing a statement execution.
#[derive(Debug, Clone)]
pub struct CfgEdge {
    pub src: usize,
    pub dst: usize,
    /// Guard expression (None = always enabled).
    pub guard: Option<Expr>,
    /// Action labels for synchronization with variable/channel automata.
    pub labels: Vec<String>,
    /// Variable effects (assignments).
    pub effects: Vec<(String, Expr)>,
}

/// Extract a CFG from a process body.
pub fn extract_cfg(proc_name: &str, body: &[Step]) -> Cfg {
    extract_cfg_with_options(proc_name, body, false)
}

/// Extract a CFG from a process body with deterministic option.
/// When `deterministic` is true (d_proctype), if/fi options get priority encoding:
/// option i is guarded by guard_i && !guard_0 && ... && !guard_{i-1}.
pub fn extract_cfg_with_options(proc_name: &str, body: &[Step], deterministic: bool) -> Cfg {
    let mut builder = CfgBuilder::new(proc_name, deterministic);
    let entry = builder.new_location("entry");
    let exit = builder.new_location("exit");

    // For a do-loop process body, the exit loops back to entry
    // But first, build the sequence normally
    builder.build_sequence(body, entry, exit, None);

    // If body is a single do-loop wrapping everything, connect exit back to entry
    if body.len() == 1 && matches!(&body[0], Step::Statement(Statement::Do { .. })) {
        // The do-loop already handles looping internally
    }

    builder.finish()
}

struct CfgBuilder {
    name: String,
    locations: Vec<Location>,
    edges: Vec<CfgEdge>,
    loc_counter: usize,
    /// Map from label name to location index (for goto resolution).
    label_map: HashMap<String, usize>,
    /// Whether this is a deterministic process (d_proctype).
    deterministic: bool,
}

impl CfgBuilder {
    fn new(name: &str, deterministic: bool) -> Self {
        Self {
            name: name.to_string(),
            locations: Vec::new(),
            edges: Vec::new(),
            loc_counter: 0,
            label_map: HashMap::new(),
            deterministic,
        }
    }

    fn new_location(&mut self, label: &str) -> usize {
        let id = self.loc_counter;
        self.loc_counter += 1;
        self.locations.push(Location {
            id,
            label: format!("{}_{}", self.name, label),
        });
        id
    }

    fn add_edge(
        &mut self,
        src: usize,
        dst: usize,
        guard: Option<Expr>,
        labels: Vec<String>,
        effects: Vec<(String, Expr)>,
    ) {
        self.edges.push(CfgEdge {
            src,
            dst,
            guard,
            labels,
            effects,
        });
    }

    fn build_sequence(
        &mut self,
        steps: &[Step],
        entry: usize,
        exit: usize,
        break_target: Option<usize>,
    ) {
        if steps.is_empty() {
            // Epsilon edge
            self.add_edge(entry, exit, None, vec![], vec![]);
            return;
        }

        let mut current = entry;
        for (i, step) in steps.iter().enumerate() {
            let next = if i == steps.len() - 1 {
                exit
            } else {
                self.new_location(&format!("loc{}", self.loc_counter))
            };
            self.build_step(step, current, next, break_target);
            current = next;
        }
    }

    fn build_step(&mut self, step: &Step, entry: usize, exit: usize, break_target: Option<usize>) {
        match step {
            Step::Statement(stmt) => self.build_statement(stmt, entry, exit, break_target),
            Step::Decl(_) => {
                // Local declaration: skip (variables are handled globally)
                self.add_edge(entry, exit, None, vec![], vec![]);
            }
        }
    }

    fn build_statement(
        &mut self,
        stmt: &Statement,
        entry: usize,
        exit: usize,
        break_target: Option<usize>,
    ) {
        match stmt {
            Statement::Assign { target, value } => {
                let label = format!("set_{}_{}", target.name, expr_summary(value));
                self.add_edge(
                    entry,
                    exit,
                    None,
                    vec![label],
                    vec![(target.name.clone(), value.clone())],
                );
            }
            Statement::Skip => {
                self.add_edge(
                    entry,
                    exit,
                    None,
                    vec![format!("{}_skip", self.name)],
                    vec![],
                );
            }
            Statement::ExprStmt { expr } => {
                // Guard expression: edge is enabled when expr is true
                let guard_label = format!("test_{}", expr_summary(expr));
                self.add_edge(entry, exit, Some(expr.clone()), vec![guard_label], vec![]);
            }
            Statement::If { options } => {
                // Each option is a guarded sequence.
                // For deterministic processes (d_proctype), add priority encoding:
                // option i gets negations of all earlier guards prepended.
                // For regular processes, all options are non-deterministic.
                if self.deterministic {
                    // Collect first-step guards for priority encoding
                    let guards: Vec<Option<Expr>> = options
                        .iter()
                        .map(|opt| {
                            // Extract guard from first step if it's an ExprStmt
                            if let Some(Step::Statement(Statement::ExprStmt { expr })) = opt.first()
                            {
                                Some(expr.clone())
                            } else {
                                None
                            }
                        })
                        .collect();

                    for (i, option) in options.iter().enumerate() {
                        let opt_entry = self.new_location(&format!("if_opt{i}"));
                        // Build negations of earlier guards as priority labels
                        let priority_labels: Vec<String> = guards
                            .iter()
                            .take(i)
                            .flatten()
                            .map(|g| format!("test_not_{}", expr_summary(g)))
                            .collect();
                        self.add_edge(entry, opt_entry, None, priority_labels, vec![]);
                        self.build_sequence(option, opt_entry, exit, break_target);
                    }
                } else {
                    for (i, option) in options.iter().enumerate() {
                        let opt_entry = self.new_location(&format!("if_opt{i}"));
                        self.add_edge(entry, opt_entry, None, vec![], vec![]);
                        self.build_sequence(option, opt_entry, exit, break_target);
                    }
                }
            }
            Statement::Do { options } => {
                // do-loop: options restart from entry, break goes to exit
                let loop_entry = entry;
                for (i, option) in options.iter().enumerate() {
                    let opt_entry = self.new_location(&format!("do_opt{i}"));
                    self.add_edge(loop_entry, opt_entry, None, vec![], vec![]);
                    // Each option ends by looping back to the do entry
                    self.build_sequence(option, opt_entry, loop_entry, Some(exit));
                }
            }
            Statement::Atomic { body } | Statement::DStep { body } | Statement::Block { body } => {
                // atomic/d_step are treated as regular sequences. This is correct
                // for composition-based verification of bounded models: the
                // composition semantics already enforces synchronous label
                // matching, and full interleaving-level atomicity enforcement
                // is beyond the adapter's scope. The current approach produces
                // sound over-approximations for safety properties.
                self.build_sequence(body, entry, exit, break_target);
            }
            Statement::Break => {
                if let Some(target) = break_target {
                    self.add_edge(entry, target, None, vec![], vec![]);
                }
            }
            Statement::Goto { label } => {
                // Goto requires label resolution (deferred)
                let goto_label = format!("goto_{label}");
                self.add_edge(entry, exit, None, vec![goto_label], vec![]);
            }
            Statement::Label { name, stmt } => {
                // Record the label location for goto resolution
                self.label_map.insert(name.clone(), entry);
                // Build the inner statement
                self.build_statement(stmt, entry, exit, break_target);
            }
            Statement::Assert { expr } => {
                // Assert: guard edge (must be true to proceed)
                let label = format!("assert_{}", expr_summary(expr));
                self.add_edge(entry, exit, Some(expr.clone()), vec![label], vec![]);
            }
            Statement::Send { channel, args: _ } => {
                let label = format!("send_{channel}");
                self.add_edge(entry, exit, None, vec![label], vec![]);
            }
            Statement::Recv { channel, args: _ } => {
                let label = format!("recv_{channel}");
                self.add_edge(entry, exit, None, vec![label], vec![]);
            }
            Statement::Printf { .. } => {
                // Printf: skip (no state change)
                self.add_edge(entry, exit, None, vec![], vec![]);
            }
            Statement::Unless { body, escape } => {
                // Unless: build the body statement, then from every intermediate
                // location in the body, add an epsilon edge to the escape entry.
                // This models the preemption semantics: at any point during the
                // body execution, the escape sequence can take over.
                let body_exit = self.new_location("unless_body_exit");
                let locs_before = self.locations.len();
                self.build_statement(body, entry, body_exit, break_target);
                let locs_after = self.locations.len();

                // Build escape sequence leading to the overall exit
                let escape_entry = self.new_location("unless_escape");
                self.build_sequence(escape, escape_entry, exit, break_target);

                // From every location created during body construction,
                // add an epsilon edge to the escape entry (preemption)
                for loc_id in locs_before..locs_after {
                    self.add_edge(loc_id, escape_entry, None, vec![], vec![]);
                }
                // Also connect body_exit to overall exit (normal completion)
                self.add_edge(body_exit, exit, None, vec![], vec![]);
            }
        }
    }

    fn finish(mut self) -> Cfg {
        // Resolve goto targets: replace edges with `goto_<label>` labels
        // by updating their destination to the label's recorded location.
        for edge in &mut self.edges {
            if edge.labels.len() == 1 && edge.labels[0].starts_with("goto_") {
                let label_name = edge.labels[0]["goto_".len()..].to_string();
                if let Some(&target_loc) = self.label_map.get(&label_name) {
                    edge.dst = target_loc;
                    edge.labels[0] = format!("goto_{}_resolved", label_name);
                }
            }
        }

        Cfg {
            name: self.name,
            locations: self.locations,
            initial: 0, // entry is always index 0
            edges: self.edges,
        }
    }
}

/// Generate a short summary string for an expression (used in label names).
fn expr_summary(expr: &Expr) -> String {
    match expr {
        Expr::IntLit(n) => format!("{n}"),
        Expr::BoolLit(b) => format!("{b}"),
        Expr::VarRef(vr) => vr.name.clone(),
        Expr::BinOp { op, left, right: _ } => {
            let op_str = match op {
                BinOp::Eq => "eq",
                BinOp::Ne => "ne",
                BinOp::Lt => "lt",
                BinOp::Le => "le",
                BinOp::Gt => "gt",
                BinOp::Ge => "ge",
                BinOp::Add => "add",
                BinOp::Sub => "sub",
                BinOp::And => "and",
                BinOp::Or => "or",
                _ => "op",
            };
            format!("{}_{}", expr_summary(left), op_str)
        }
        Expr::UnOp {
            op: UnOp::Not,
            operand,
        } => format!("not_{}", expr_summary(operand)),
        Expr::Timeout => "timeout".to_string(),
        Expr::RemoteRef { process, label } => format!("{}_at_{}", process, label),
        Expr::RemoteVar { process, var } => format!("{}_{}", process, var),
        _ => "expr".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::promela::parser::parse;

    #[test]
    fn extract_peterson_p0_cfg() {
        let input = r#"
byte turn = 0;
bool flag0 = false;
bool cs0 = false;
bool flag1 = false;

active proctype P0() {
    do
    :: true ->
        flag0 = true;
        turn = 1;
        (flag1 == false || turn == 0);
        cs0 = true;
        cs0 = false;
        flag0 = false;
    od
}

ltl mutex { [] !(cs0) }
"#;
        let program = parse(input).unwrap();
        let proc = &program.proctypes[0];
        let cfg = extract_cfg(&proc.name, &proc.body);

        assert!(!cfg.locations.is_empty(), "CFG should have locations");
        assert!(!cfg.edges.is_empty(), "CFG should have edges");
        assert_eq!(cfg.initial, 0, "Initial location should be 0");

        // The CFG should have edges for: assignment (flag0=true), assignment (turn=1),
        // guard (flag1==false || turn==0), assignment (cs0=true), assignment (cs0=false),
        // assignment (flag0=false), plus structural edges for the do-loop.
        let assignment_edges: Vec<_> = cfg
            .edges
            .iter()
            .filter(|e| !e.labels.is_empty() && e.labels[0].starts_with("set_"))
            .collect();
        assert!(
            assignment_edges.len() >= 4,
            "Should have at least 4 assignment edges, got {}",
            assignment_edges.len()
        );
    }
}
