//! H.O.0 — internal exact oracle for verify-auto verdicts (increment 1).
//!
//! The 2026-06-29 soundness review found we have **no independent check** that a
//! DEFINITE verify-auto verdict is correct — and a spurious `HOLDS` (silently
//! claiming a property holds) is more dangerous than a spurious `VIOLATED` and
//! nothing catches it. This module is the cheapest oracle: it computes a safety
//! property's **concrete** truth by bounded reachability enumeration (mununu's
//! own exact one-step semantics, no abstraction), so a cube-path `HOLDS` that the
//! concrete design contradicts is caught.
//!
//! **Scope (increment 1).** The `AG (signal ⋈ value)` fragment over a **state
//! register** signal — input-independent, exactly where a spurious HOLDS on the
//! state-predicate cube core would surface. Combinational / input atoms (which
//! need input quantification) and the `|=>` (`AX`) shape are later increments
//! (H.O.0.2+). The reachability is **sound for finding violations** always, and
//! **sound for concluding AG-true** only when the full input space was
//! enumerated (`!bounded`); a `bounded` result never concludes true.
//!
//! Not wired into the production path — a validation oracle, consumed by the
//! verify-auto e2e/regression tests (`AgOracle` ⟷ the cube verdict).

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::adapter::AdapterError;
use crate::adapter::btor2::ast::{ConstValue, Node};
use crate::adapter::btor2::parser::{self, bv_width};
use crate::adapter::btor2::predicate_expr::CmpOp;
use crate::adapter::btor2::{ast::Btor2File, bit_blast};

/// Bounded reachable set of concrete register valuations.
#[derive(Debug, Clone)]
pub struct Reachability {
    /// Reachable register-state valuations (keyed by state-cell symbol).
    pub states: Vec<BTreeMap<String, u128>>,
    /// True when enumeration was capped (by `max_states`) or some input could
    /// not be exhaustively enumerated (wider than `max_input_bits`). A bounded
    /// reachability can still witness a violation, but cannot conclude AG-true.
    pub bounded: bool,
}

/// The concrete `AG (atom)` verdict from bounded reachability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgOracle {
    /// The atom holds at every reachable state AND the full input space was
    /// enumerated — `AG atom` is concretely TRUE.
    Holds,
    /// A reachable state violates the atom — `AG atom` is concretely FALSE.
    /// Carries the witness register valuation.
    Violated(BTreeMap<String, u128>),
    /// No violation found, but enumeration was bounded — inconclusive for TRUE
    /// (the oracle can refute a cube-HOLDS but not confirm it here).
    Inconclusive,
}

/// State-cell symbols of the design.
fn state_cells(file: &Btor2File) -> Vec<(String, u32)> {
    let symbols = parser::collect_symbols(file);
    file.lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::State { sort, .. } => {
                let name = symbols.get(&l.nid)?.clone();
                let w = bv_width(file, *sort).unwrap_or(0);
                Some((name, w))
            }
            _ => None,
        })
        .collect()
}

/// Init value of each state cell (from BTOR2 `init` lines, default 0 — the
/// `setundef -zero` power-up).
fn init_valuation(file: &Btor2File) -> BTreeMap<String, u128> {
    let symbols = parser::collect_symbols(file);
    let mut init_of: HashMap<crate::adapter::btor2::ast::Nid, u128> = HashMap::new();
    for line in &file.lines {
        if let Node::Init { state, value, .. } = &line.node
            && let Some(cl) = file.lookup(value.nid())
            && let Node::Const { sort, value: cv } = &cl.node
        {
            init_of.insert(*state, const_to_u128(file, *sort, cv));
        }
    }
    let mut out = BTreeMap::new();
    for line in &file.lines {
        if matches!(line.node, Node::State { .. })
            && let Some(name) = symbols.get(&line.nid)
        {
            out.insert(name.clone(), init_of.get(&line.nid).copied().unwrap_or(0));
        }
    }
    out
}

fn const_to_u128(file: &Btor2File, sort: crate::adapter::btor2::ast::Nid, cv: &ConstValue) -> u128 {
    match cv {
        ConstValue::Zero => 0,
        ConstValue::One => 1,
        ConstValue::Ones => {
            let w = bv_width(file, sort).unwrap_or(0);
            if w == 0 || w >= 128 {
                u128::MAX
            } else {
                (1u128 << w) - 1
            }
        }
        ConstValue::Dec(d) => *d as u128,
        ConstValue::Bin(b) => u128::from_str_radix(b, 2).unwrap_or(0),
        ConstValue::Hex(h) => u128::from_str_radix(h, 16).unwrap_or(0),
    }
}

/// 1-bit primary input symbols (the enumerable environment).
fn boolean_inputs(file: &Btor2File) -> (Vec<String>, bool) {
    let symbols = parser::collect_symbols(file);
    let mut bool_ins = Vec::new();
    let mut has_wide = false;
    for line in &file.lines {
        if let Node::Input { sort, .. } = &line.node {
            let w = bv_width(file, *sort).unwrap_or(0);
            match (w, symbols.get(&line.nid)) {
                (1, Some(name)) => bool_ins.push(name.clone()),
                // a wide (>1-bit) input we will NOT exhaustively enumerate →
                // reachability becomes bounded (sound only for finding violations).
                _ => has_wide = true,
            }
        }
    }
    bool_ins.sort();
    (bool_ins, has_wide)
}

/// BFS the reachable concrete register-states from init, simulating one step over
/// every combination of the (1-bit) primary inputs. Bounded by `max_states`; a
/// wide input or hitting the cap sets `bounded`.
pub fn reachable_register_states(
    file: &Btor2File,
    max_states: usize,
    max_input_bits: usize,
) -> Result<Reachability, AdapterError> {
    let (bool_ins, has_wide) = boolean_inputs(file);
    let n_in = bool_ins.len().min(max_input_bits);
    // bounded if a wide input exists, or we capped the input bits we enumerate.
    let mut bounded = has_wide || bool_ins.len() > max_input_bits;
    let n_combos: usize = 1usize << n_in;

    let init = init_valuation(file);
    let mut seen: HashSet<Vec<(String, u128)>> = HashSet::new();
    // `order` doubles as the BFS queue (walked by `head`) and the result list.
    let mut order: Vec<BTreeMap<String, u128>> = Vec::new();
    let key = |s: &BTreeMap<String, u128>| -> Vec<(String, u128)> {
        s.iter().map(|(k, v)| (k.clone(), *v)).collect()
    };
    seen.insert(key(&init));
    order.push(init);

    let mut head = 0;
    while head < order.len() {
        if order.len() >= max_states {
            bounded = true;
            break;
        }
        let state = order[head].clone();
        head += 1;
        let regs: HashMap<String, u128> = state.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for combo in 0..n_combos {
            let inputs: HashMap<String, u128> = bool_ins
                .iter()
                .take(n_in)
                .enumerate()
                .map(|(b, name)| (name.clone(), ((combo >> b) & 1) as u128))
                .collect();
            let next = bit_blast::simulate_one_step(file, &regs, &inputs)?;
            let next_sorted: BTreeMap<String, u128> = next.into_iter().collect();
            if seen.insert(key(&next_sorted)) {
                if order.len() < max_states {
                    order.push(next_sorted);
                } else {
                    bounded = true;
                }
            }
        }
    }
    Ok(Reachability {
        states: order,
        bounded,
    })
}

fn cmp_holds(op: CmpOp, lhs: u128, rhs: u128) -> bool {
    match op {
        CmpOp::Eq => lhs == rhs,
        CmpOp::Ne => lhs != rhs,
        CmpOp::Lt => lhs < rhs,
        CmpOp::Le => lhs <= rhs,
        CmpOp::Gt => lhs > rhs,
        CmpOp::Ge => lhs >= rhs,
    }
}

/// Concrete `AG (signal ⋈ value)` oracle over a **state-register** signal. The
/// signal must be a state cell (input-independent); a non-state signal returns an
/// error (out of increment-1 scope). Reads each reachable state's register value
/// directly — no abstraction.
pub fn ag_state_atom(
    file: &Btor2File,
    signal: &str,
    op: CmpOp,
    value: u128,
    max_states: usize,
    max_input_bits: usize,
) -> Result<AgOracle, AdapterError> {
    if !state_cells(file).iter().any(|(n, _)| n == signal) {
        return Err(AdapterError {
            kind: crate::adapter::AdapterErrorKind::IrConsistencyError,
            location: None,
            message: format!(
                "concrete_oracle::ag_state_atom: `{signal}` is not a state cell \
                 (increment 1 covers state-register safety atoms only)"
            ),
        });
    }
    let reach = reachable_register_states(file, max_states, max_input_bits)?;
    for st in &reach.states {
        let v = st.get(signal).copied().unwrap_or(0);
        if !cmp_holds(op, v, value) {
            return Ok(AgOracle::Violated(st.clone()));
        }
    }
    if reach.bounded {
        Ok(AgOracle::Inconclusive)
    } else {
        Ok(AgOracle::Holds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::btor2::parser::parse;

    // A 2-bit FSM cycling 0→1→2→0 (caps at 2, never reaches 3), input-free in
    // the next-state (deterministic). `state` is a 2-bit register.
    //   next(state) = (state == 2) ? 0 : state + 1
    // BTOR2: 1 sort2, 2 state, consts, the ite chain.
    const CYCLE_FSM: &str = "\
1 sort bitvec 1
2 sort bitvec 2
3 state 2 state
4 zero 2
5 one 2
6 constd 2 2
7 eq 1 3 6
8 add 2 3 5
9 ite 2 7 4 8
10 next 2 3 9
11 init 2 3 4
";

    #[test]
    fn reachability_enumerates_the_cycle() {
        let file = parse(CYCLE_FSM).expect("parse cycle fsm");
        let r = reachable_register_states(&file, 64, 8).expect("reach");
        let vals: std::collections::HashSet<u128> = r.states.iter().map(|s| s["state"]).collect();
        // reachable states: 0, 1, 2 (never 3). Fully enumerated (no inputs).
        assert_eq!(
            vals,
            [0, 1, 2].into_iter().collect(),
            "0→1→2→0 cycle, 3 unreachable"
        );
        assert!(!r.bounded, "no inputs ⇒ full enumeration");
    }

    #[test]
    fn ag_holds_for_unreachable_value() {
        let file = parse(CYCLE_FSM).expect("parse");
        // AG (state != 3) — 3 is unreachable → HOLDS (full enumeration).
        assert_eq!(
            ag_state_atom(&file, "state", CmpOp::Ne, 3, 64, 8).unwrap(),
            AgOracle::Holds
        );
    }

    #[test]
    fn ag_violated_for_reachable_value() {
        let file = parse(CYCLE_FSM).expect("parse");
        // AG (state != 1) — 1 IS reachable → VIOLATED, with the witness.
        match ag_state_atom(&file, "state", CmpOp::Ne, 1, 64, 8).unwrap() {
            AgOracle::Violated(st) => assert_eq!(st["state"], 1),
            other => panic!("expected Violated(state=1), got {other:?}"),
        }
    }

    #[test]
    fn ag_le_bound_holds() {
        let file = parse(CYCLE_FSM).expect("parse");
        // AG (state <= 2) — every reachable state ≤ 2 → HOLDS.
        assert_eq!(
            ag_state_atom(&file, "state", CmpOp::Le, 2, 64, 8).unwrap(),
            AgOracle::Holds
        );
    }

    #[test]
    fn non_state_signal_is_rejected() {
        let file = parse(CYCLE_FSM).expect("parse");
        assert!(ag_state_atom(&file, "not_a_state", CmpOp::Eq, 0, 64, 8).is_err());
    }
}
