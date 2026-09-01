//! BTOR2 structural mutation (#468) — apply a **named** fault to a lifted design
//! and re-emit, so a verify pass can assert its property verdicts **flip**.
//!
//! This is the engine behind `sv mutate`: a mutation that flips a `holds` to
//! `violated` confirms the property actually **constrains** the mutated behaviour;
//! a mutation that does NOT flip it is the finding — a **vacuous** property or a
//! dead line the spec never pins down. Per [claims-integrity §2], a mutation
//! result is a statement about **property adequacy**, never a bug finding about the
//! design.
//!
//! Every mutation is a pure `parse → mutate [`Node`] → `emit_btor2` round-trip
//! (the emitter is a fixed point of the parser, so an untouched model re-emits
//! byte-for-byte; only the mutated node changes). No lift is needed, so the core
//! is unit-tested on a BTOR2 fixture in make-ci.

use super::ast::{Btor2File, Nid, Node, Op, Operand};
use super::bit_blast::resolve_btor2_constant;
use super::parser::{self, collect_symbols};

/// A named structural fault applied to a lifted BTOR2 design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    /// **Freeze** a register: replace its `next` value with the register's own
    /// current value (identity), so it never updates. Universal — every register
    /// with a `next` can be stuck.
    Stick(String),
    /// **Remove the reset arm**: a register whose `next` is a reset mux
    /// `ite(rst, RESET, d)` is rewritten to the data branch `d`, so it no longer
    /// returns to its reset value. Flips reset-dependent properties (recoverability).
    DropReset(String),
}

impl Mutation {
    /// The stable CLI/label spelling (`stick:<reg>` / `drop-reset:<reg>`).
    pub fn as_label(&self) -> String {
        match self {
            Mutation::Stick(r) => format!("stick:{r}"),
            Mutation::DropReset(r) => format!("drop-reset:{r}"),
        }
    }

    /// Parse a `stick:<reg>` / `drop-reset:<reg>` selector (the CLI/API spelling).
    pub fn parse(spec: &str) -> Result<Mutation, String> {
        if let Some(reg) = spec.strip_prefix("stick:") {
            reg_nonempty(reg).map(|r| Mutation::Stick(r.to_string()))
        } else if let Some(reg) = spec.strip_prefix("drop-reset:") {
            reg_nonempty(reg).map(|r| Mutation::DropReset(r.to_string()))
        } else {
            Err(format!(
                "unknown mutation `{spec}` — expected `stick:<reg>` or `drop-reset:<reg>` \
                 (see `sv mutate --list` for the available targets)"
            ))
        }
    }
}

fn reg_nonempty(reg: &str) -> Result<&str, String> {
    if reg.is_empty() {
        Err("mutation target register name is empty".to_string())
    } else {
        Ok(reg)
    }
}

/// The mutation targets available in a design — the `sv mutate --list` payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutationTargets {
    /// Every **named** register — a `stick:<reg>` target (sorted, deduped).
    pub stick: Vec<String>,
    /// Every named register whose `next` is a one-constant reset mux — a
    /// `drop-reset:<reg>` target (sorted, deduped; a subset of `stick`).
    pub drop_reset: Vec<String>,
}

/// Apply `m` to the BTOR2 text and return the mutated BTOR2 (round-trip via the
/// emitter). Errors if the named register does not exist or the mutation does not
/// apply to it (e.g. `drop-reset` on a register with no reset mux).
pub fn apply_mutation(btor2: &str, m: &Mutation) -> Result<String, String> {
    let mut file = parser::parse(btor2).map_err(|e| format!("BTOR2 parse: {}", e.message))?;
    match m {
        Mutation::Stick(reg) => apply_stick(&mut file, reg)?,
        Mutation::DropReset(reg) => apply_drop_reset(&mut file, reg)?,
    }
    Ok(super::emit::emit_btor2(&file))
}

/// Enumerate the mutation targets in a lifted design (`sv mutate --list`).
pub fn list_targets(btor2: &str) -> Result<MutationTargets, String> {
    let file = parser::parse(btor2).map_err(|e| format!("BTOR2 parse: {}", e.message))?;
    let symbols = collect_symbols(&file);
    let mut stick: Vec<String> = Vec::new();
    let mut drop_reset: Vec<String> = Vec::new();
    for line in &file.lines {
        if !matches!(line.node, Node::State { .. }) {
            continue;
        }
        // Only NAMED registers are addressable targets (an anonymous split
        // sub-cell has no stable handle).
        let Some(name) = symbols.get(&line.nid) else {
            continue;
        };
        stick.push(name.clone());
        if reset_data_branch(&file, line.nid).is_some() {
            drop_reset.push(name.clone());
        }
    }
    stick.sort();
    stick.dedup();
    drop_reset.sort();
    drop_reset.dedup();
    Ok(MutationTargets { stick, drop_reset })
}

/// Resolve a user register NAME to its `state` line nid — directly (the symbol is
/// on the `state` line) or via a `uext … 0 NAME` alias (`collect_symbols` attaches
/// the alias name to the underlying state nid). `None` if no state cell carries it.
fn resolve_state_nid(file: &Btor2File, name: &str) -> Option<Nid> {
    let symbols = collect_symbols(file);
    symbols.iter().find_map(|(nid, sym)| {
        if sym == name && matches!(file.lookup(*nid)?.node, Node::State { .. }) {
            Some(*nid)
        } else {
            None
        }
    })
}

/// Freeze a register: point its `next` at its own state nid (identity → frozen).
fn apply_stick(file: &mut Btor2File, reg: &str) -> Result<(), String> {
    let state_nid = resolve_state_nid(file, reg).ok_or_else(|| {
        format!("stick: `{reg}` is not a register (no state cell in the lift carries that name)")
    })?;
    let mut found = false;
    for l in file.lines.iter_mut() {
        if let Node::Next { state, value, .. } = &mut l.node
            && *state == state_nid
        {
            *value = Operand(state_nid);
            found = true;
        }
    }
    if found {
        Ok(())
    } else {
        Err(format!(
            "stick: register `{reg}` (nid {state_nid}) has no `next` line to freeze \
             (it is already an undriven/free register)"
        ))
    }
}

/// Drop a register's reset arm: rewrite its reset-mux `next` to the data branch.
fn apply_drop_reset(file: &mut Btor2File, reg: &str) -> Result<(), String> {
    let state_nid = resolve_state_nid(file, reg).ok_or_else(|| {
        format!("drop-reset: `{reg}` is not a register (no state cell carries that name)")
    })?;
    let data = reset_data_branch(file, state_nid).ok_or_else(|| {
        format!(
            "drop-reset: register `{reg}`'s next is not a one-constant reset mux \
             (`ite(rst, RESET, d)`) — there is no reset arm to drop"
        )
    })?;
    for l in file.lines.iter_mut() {
        if let Node::Next { state, value, .. } = &mut l.node
            && *state == state_nid
        {
            *value = data;
        }
    }
    Ok(())
}

/// If `state_nid`'s `next` is a one-constant reset mux `ite(cond, then, else)`
/// (exactly one branch a constant — the reset value), return the OTHER (data)
/// branch operand; else `None`. Mirrors `fsm_scan::analyze_reset_mux`'s shape test
/// (the constant branch is the reset value, whatever the polarity).
fn reset_data_branch(file: &Btor2File, state_nid: Nid) -> Option<Operand> {
    let next_val_nid = file.lines.iter().find_map(|l| match &l.node {
        Node::Next { state, value, .. } if *state == state_nid => Some(value.nid()),
        _ => None,
    })?;
    // Copy the two data operands out (Operand: Copy), releasing the file borrow
    // before the constant lookups below.
    let (then_op, else_op) = match file.lookup(next_val_nid).map(|l| &l.node) {
        Some(Node::Op {
            op: Op::Ite, args, ..
        }) if args.len() == 3 => (args[1], args[2]),
        _ => return None,
    };
    match (
        resolve_btor2_constant(file, then_op.nid()),
        resolve_btor2_constant(file, else_op.nid()),
    ) {
        (None, Some(_)) => Some(then_op), // reset in the `else` branch ⇒ data is `then`
        (Some(_), None) => Some(else_op), // reset in the `then` branch ⇒ data is `else`
        _ => None, // both-const / neither-const ⇒ not an unambiguous reset mux
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 2-register design: `q` toggles (no reset mux); `r` is a reset mux
    // `ite(rst, 0, d)` — reset in the THEN branch (active-high, reset value 0),
    // data in the ELSE branch (`d`). Both named.
    const FIXTURE: &str = "1 sort bitvec 1
2 input 1 rst
3 input 1 d
4 state 1 q
5 not 1 4
6 next 1 4 5
7 zero 1
8 init 1 4 7
9 state 1 r
10 ite 1 2 7 3
11 next 1 9 10
12 init 1 9 7
";

    fn next_value_of(btor2: &str, state_nid: Nid) -> Nid {
        let file = parser::parse(btor2).expect("parse mutated");
        file.lines
            .iter()
            .find_map(|l| match &l.node {
                Node::Next { state, value, .. } if *state == state_nid => Some(value.nid()),
                _ => None,
            })
            .expect("a next for the state")
    }

    #[test]
    fn stick_freezes_the_register_to_its_own_state() {
        let out = apply_mutation(FIXTURE, &Mutation::Stick("q".into())).expect("stick q");
        // next(q=4) now points at 4 (identity → frozen), was 5 (not q).
        assert_eq!(next_value_of(&out, 4), 4);
        // The other register is untouched.
        assert_eq!(next_value_of(&out, 9), 10);
    }

    #[test]
    fn drop_reset_rewrites_next_to_the_data_branch() {
        let out = apply_mutation(FIXTURE, &Mutation::DropReset("r".into())).expect("drop-reset r");
        // next(r=9) now points at the data branch `d` (nid 3), dropping the ite (10).
        assert_eq!(next_value_of(&out, 9), 3);
        // `q` untouched.
        assert_eq!(next_value_of(&out, 4), 5);
    }

    #[test]
    fn list_targets_reports_stick_for_all_named_and_drop_reset_for_reset_muxes() {
        let t = list_targets(FIXTURE).expect("list");
        assert_eq!(t.stick, vec!["q".to_string(), "r".to_string()]);
        assert_eq!(
            t.drop_reset,
            vec!["r".to_string()],
            "only the reset-mux register `r` is a drop-reset target; `q` toggles"
        );
    }

    #[test]
    fn stick_unknown_register_errors() {
        assert!(apply_mutation(FIXTURE, &Mutation::Stick("nope".into())).is_err());
    }

    #[test]
    fn drop_reset_on_a_non_reset_register_errors() {
        // `q` has no reset mux (its next is `not q`), so there is nothing to drop.
        assert!(apply_mutation(FIXTURE, &Mutation::DropReset("q".into())).is_err());
    }

    #[test]
    fn mutation_parse_roundtrips_the_labels() {
        assert_eq!(
            Mutation::parse("stick:foo").unwrap(),
            Mutation::Stick("foo".into())
        );
        assert_eq!(
            Mutation::parse("drop-reset:bar").unwrap(),
            Mutation::DropReset("bar".into())
        );
        assert_eq!(
            Mutation::parse("stick:foo").unwrap().as_label(),
            "stick:foo"
        );
        assert!(Mutation::parse("frobnicate:x").is_err());
        assert!(Mutation::parse("stick:").is_err());
    }
}
