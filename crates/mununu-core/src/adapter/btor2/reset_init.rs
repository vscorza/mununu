//! Reset-gated initial state — inject BTOR2 `init` lines at the post-reset state.
//!
//! An async-reset flop (`always_ff @(posedge clk or negedge rst_ni)
//! if (!rst_ni) q <= ResetValue; else q <= d;`) lifts — via Yosys `async2sync` —
//! to a next-state mux `next(q) = ite(rst, ResetValue, d)` with **no `init`
//! line**: the *reset*, not a power-on value, establishes the start state.
//! verify-auto then PINS the reset input to its inactive level (the "verified
//! out of reset" discipline), so the mux never selects `ResetValue`, and both
//! verify-auto engines default the init-less register to 0
//! ([`crate::adapter::btor2::symbolic_bitblast`]'s `initial_state_bdd` and the
//! predicate-cube path's `state_cell_init_values`, per the `setundef -zero`
//! convention).
//!
//! For a design whose valid initial state is established BY reset — e.g. an
//! OpenTitan sparse-FSM whose `ResetValue` is a non-zero sparse encoding
//! (`MainSmIdle = 6'b110111`) — starting at 0 lands on an **illegal encoding**,
//! which the FSM's `default` arm traps into its error state. Every
//! reset-dependent verdict (recoverability `AG EF idle`, liveness-from-reset) is
//! then computed from a state the real design never occupies.
//!
//! This transformation restores the intended semantics. BEFORE the reset is
//! pinned inactive, it derives the post-reset state by simulating one cycle with
//! the reset ASSERTED (the same 1-cycle hold as
//! [`crate::adapter::btor2::bit_blast`]'s F1.1 `apply_auto_reset`, which does the
//! equivalent for the enum/CTXDSL path) and appends an `init` line per state
//! cell at that value. Working at the BTOR2 **text** level means both
//! verify-auto engines — which each re-parse the text and read `init` lines —
//! pick up the reset state with no per-engine change.
//!
//! Scope guard: only fires when reset-gating is on (`resets` non-empty — empty
//! under `--no-gate-reset`, where the design chooses its own power-up, including
//! the undefined-encoding scenarios CWE-1245 detection relies on) AND the design
//! carries no `init` line already (the pure-async-reset shape; a design with an
//! authoritative BTOR2 init is left untouched).

use crate::adapter::AdapterError;
use crate::adapter::btor2::ast::{Nid, Node, Sort};
use crate::adapter::btor2::bit_blast::simulate_one_step;
use crate::adapter::btor2::parser;
use std::collections::HashMap;

/// Inject `init` lines at the post-reset state for a reset-gated async-reset
/// design that has none. `resets` is the set of `(name, inactive_value)` reset
/// pins verify-auto detected — the same set it pins inactive.
///
/// No-op (returns `content` unchanged) when: `resets` is empty; the design has
/// no state cells; or ANY state cell already carries an `init` line (then the
/// BTOR2 init is authoritative and must not be advanced past — matching
/// `apply_auto_reset`'s guard).
///
/// The post-reset state is `simulate_one_step` from the all-zero power-on cube
/// with each reset input pinned to its ASSERTED level (the complement of its
/// inactive level). A reset-register's next-state mux then selects its
/// `ResetValue`; a register with no reset advances one cycle from 0 (its
/// post-reset value is whatever a held-reset cycle yields — the faithful "one
/// reset cycle then release" state).
pub fn inject_reset_init(content: &str, resets: &[(String, u64)]) -> Result<String, AdapterError> {
    if resets.is_empty() {
        return Ok(content.to_string());
    }

    let file = parser::parse(content).map_err(|mut e| {
        e.message = format!("adapter/btor2/reset_init: {}", e.message);
        e
    })?;

    // Guard: only the pure-async-reset shape. Any existing `init` line means the
    // BTOR2 init is authoritative — leave it untouched.
    if file
        .lines
        .iter()
        .any(|l| matches!(&l.node, Node::Init { .. }))
    {
        return Ok(content.to_string());
    }

    // State cells (nid, sort, key). Yosys leaves the async2sync FSM register
    // unnamed; `simulate_one_step` keys those by `st_n<nid>`, so mirror that.
    let symbols = parser::collect_symbols(&file);
    let states: Vec<(Nid, Nid, String)> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::State { sort, .. } => {
                let key = symbols
                    .get(&l.nid)
                    .cloned()
                    .unwrap_or_else(|| format!("st_n{}", l.nid));
                Some((l.nid, *sort, key))
            }
            _ => None,
        })
        .collect();
    if states.is_empty() {
        return Ok(content.to_string());
    }

    // Reset ASSERTED = complement of the inactive level (resets are 1-bit).
    // Registers default 0 (setundef -zero power-on); non-reset inputs default 0.
    let reset_inputs: HashMap<String, u128> = resets
        .iter()
        .map(|(name, inactive)| {
            let asserted: u128 = if *inactive == 0 { 1 } else { 0 };
            (name.clone(), asserted)
        })
        .collect();
    let post_reset = simulate_one_step(&file, &HashMap::new(), &reset_inputs)?;

    let mut next_nid: Nid = file.lines.iter().map(|l| l.nid).max().unwrap_or(0) + 1;
    let mut appended: Vec<String> = Vec::new();
    for (state_nid, sort_nid, key) in &states {
        let value = post_reset.get(key).copied().unwrap_or(0);
        let const_nid = next_nid;
        next_nid += 1;
        let init_nid = next_nid;
        next_nid += 1;
        appended.push(format!("{const_nid} constd {sort_nid} {value}"));
        appended.push(format!(
            "{init_nid} init {sort_nid} {state_nid} {const_nid}"
        ));
    }

    Ok(format!("{}\n{}\n", content.trim_end(), appended.join("\n")))
}

/// Complete the BTOR2 init to the `setundef -zero` power-on: append an `init … 0`
/// line for every BITVEC state cell that carries no `init` line.
///
/// **Why.** The cube and exact engines already default an init-less state cell to
/// 0 (`state_cell_init_values` / `initial_state_bdd`, per the `setundef -zero`
/// power-up). The reachability portfolio (native BMC / spacer / Boolector),
/// however, leaves an init-less cell FREE — BTOR2's nondeterministic-init
/// semantics. On a reset-less design with a *partial* `initial` (a Xilinx-style
/// wrapper: `initial state = IDLE` but an init-less status flop), that mismatch
/// hands the portfolio a power-up counterexample the exact engine never sees — a
/// verdict DISAGREEMENT (portfolio `VIOLATED` at 0 cells vs exact `HOLDS`).
/// Making the 0 power-up EXPLICIT in the BTOR2 puts every engine on the same
/// initial state.
///
/// **Scope / soundness.** Only for the reset-gated verify-auto path (its lift is
/// the `setundef -zero` model the cube/exact already assume); raw `btor2 verify`
/// keeps BTOR2's free-init semantics. Existing `init` lines — authoritative
/// `initial` values, or [`inject_reset_init`]'s post-reset state — are LEFT
/// UNTOUCHED; only init-less cells are completed. Array-sorted states are skipped
/// (a `constd` init is ill-typed for them).
pub fn inject_zero_init(content: &str) -> Result<String, AdapterError> {
    let file = parser::parse(content).map_err(|mut e| {
        e.message = format!("adapter/btor2/reset_init(zero-init): {}", e.message);
        e
    })?;

    // Sort nids that are bitvec — array-sorted states are skipped.
    let bitvec_sorts: std::collections::HashSet<Nid> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::Sort {
                sort: Sort::BitVec { .. },
            } => Some(l.nid),
            _ => None,
        })
        .collect();

    // States that already carry an `init` line stay authoritative.
    let has_init: std::collections::HashSet<Nid> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::Init { state, .. } => Some(*state),
            _ => None,
        })
        .collect();

    let initless: Vec<(Nid, Nid)> = file
        .lines
        .iter()
        .filter_map(|l| match &l.node {
            Node::State { sort, .. }
                if !has_init.contains(&l.nid) && bitvec_sorts.contains(sort) =>
            {
                Some((l.nid, *sort))
            }
            _ => None,
        })
        .collect();
    if initless.is_empty() {
        return Ok(content.to_string());
    }

    let mut next_nid: Nid = file.lines.iter().map(|l| l.nid).max().unwrap_or(0) + 1;
    let mut appended: Vec<String> = Vec::new();
    for (state_nid, sort_nid) in &initless {
        let const_nid = next_nid;
        next_nid += 1;
        let init_nid = next_nid;
        next_nid += 1;
        appended.push(format!("{const_nid} constd {sort_nid} 0"));
        appended.push(format!(
            "{init_nid} init {sort_nid} {state_nid} {const_nid}"
        ));
    }
    Ok(format!("{}\n{}\n", content.trim_end(), appended.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve a state cell's init value (mirrors how the exact engine's
    /// `initial_state_bdd` reads the BTOR2 init), for assertions.
    fn init_value(content: &str, state_symbol: &str) -> Option<u128> {
        let file = parser::parse(content).ok()?;
        let symbols = parser::collect_symbols(&file);
        let state_nid = file.lines.iter().find_map(|l| match &l.node {
            Node::State { .. } if symbols.get(&l.nid).map(String::as_str) == Some(state_symbol) => {
                Some(l.nid)
            }
            _ => None,
        })?;
        let value_op = file.lines.iter().find_map(|l| match &l.node {
            Node::Init { state, value, .. } if *state == state_nid => Some(*value),
            _ => None,
        })?;
        // The init value operand references a const line; resolve its value.
        file.lines.iter().find_map(|l| {
            if l.nid != value_op.nid() {
                return None;
            }
            match &l.node {
                Node::Const {
                    value: crate::adapter::btor2::ast::ConstValue::Dec(d),
                    ..
                } => Some(*d as u128),
                _ => None,
            }
        })
    }

    // Async-reset FSM, active-HIGH reset: `next(fsm) = ite(rst, 1, 2)` — asserting
    // rst forces the reset value 1. No `init` line (the async-reset shape). The
    // injection must init fsm at its reset value 1, NOT the power-on default 0.
    const ASYNC_RESET_HIGH: &str = r#"1 sort bitvec 1
2 sort bitvec 2
3 state 2 fsm
4 constd 2 1
5 constd 2 2
6 input 1 rst
7 ite 2 6 4 5
8 next 2 3 7
"#;

    #[test]
    fn injects_reset_value_as_init_active_high() {
        // Active-high reset ⇒ inactive level 0 ⇒ asserted level 1.
        let out = inject_reset_init(ASYNC_RESET_HIGH, &[("rst".to_string(), 0)]).unwrap();
        assert_eq!(
            init_value(&out, "fsm"),
            Some(1),
            "fsm must init at the reset value 1, not the power-on default 0; got:\n{out}"
        );
    }

    #[test]
    fn injects_reset_value_as_init_active_low() {
        // Active-low reset shape: `next(fsm) = ite(rst_ni, 2, 1)` — rst_ni LOW
        // (asserted) selects the reset value 1. Inactive level 1 ⇒ asserted 0.
        let src = r#"1 sort bitvec 1
2 sort bitvec 2
3 state 2 fsm
4 constd 2 1
5 constd 2 2
6 input 1 rst_ni
7 ite 2 6 5 4
8 next 2 3 7
"#;
        let out = inject_reset_init(src, &[("rst_ni".to_string(), 1)]).unwrap();
        assert_eq!(
            init_value(&out, "fsm"),
            Some(1),
            "fsm must init at the reset value 1 for an active-low reset; got:\n{out}"
        );
    }

    #[test]
    fn no_op_without_resets() {
        let out = inject_reset_init(ASYNC_RESET_HIGH, &[]).unwrap();
        assert_eq!(out, ASYNC_RESET_HIGH, "empty resets ⇒ unchanged");
    }

    #[test]
    fn no_op_when_init_already_present() {
        // A design with an authoritative BTOR2 init (fsm init = 2) must be left
        // untouched — the reset injection never overrides an explicit init.
        let src = r#"1 sort bitvec 1
2 sort bitvec 2
3 state 2 fsm
4 constd 2 1
5 constd 2 2
6 input 1 rst
7 ite 2 6 4 5
8 next 2 3 7
9 init 2 3 5
"#;
        let out = inject_reset_init(src, &[("rst".to_string(), 0)]).unwrap();
        assert_eq!(out, src, "existing init is authoritative ⇒ unchanged");
    }

    // A reset-less design with a PARTIAL initial: `st` (2-bit) carries an
    // authoritative `init 2`, but `stall` (1-bit) has none. `inject_zero_init`
    // must complete `stall` to 0 (matching the cube/exact power-up) while leaving
    // `st`'s init untouched — the wbicapetwo shape that caused the exact-vs-
    // portfolio disagreement.
    const PARTIAL_INITIAL: &str = r#"1 sort bitvec 1
2 sort bitvec 2
3 state 2 st
4 state 1 stall
5 constd 2 2
6 init 2 3 5
7 constd 2 0
8 next 2 3 7
9 constd 1 1
10 next 1 4 9
"#;

    #[test]
    fn zero_init_completes_initless_bitvec_cell() {
        let out = inject_zero_init(PARTIAL_INITIAL).unwrap();
        // `stall` gets an explicit 0 init; `st` keeps its authoritative 2.
        assert_eq!(init_value(&out, "stall"), Some(0), "init-less stall ⇒ 0");
        assert_eq!(
            init_value(&out, "st"),
            Some(2),
            "authoritative st untouched"
        );
    }

    #[test]
    fn zero_init_no_op_when_all_cells_initialised() {
        // Every state already has an init ⇒ unchanged.
        let src = r#"1 sort bitvec 1
2 state 1 q
3 constd 1 1
4 init 1 2 3
5 next 1 2 3
"#;
        assert_eq!(inject_zero_init(src).unwrap(), src, "all-init ⇒ unchanged");
    }

    #[test]
    fn zero_init_skips_array_sorted_state() {
        // An array-sorted state (a memory) must NOT get a `constd` init (ill-typed);
        // the bitvec `q` still gets its 0.
        let src = r#"1 sort bitvec 1
2 sort bitvec 8
3 sort array 1 2
4 state 3 mem
5 state 1 q
6 next 1 5 5
"#;
        let out = inject_zero_init(src).unwrap();
        assert_eq!(init_value(&out, "q"), Some(0), "bitvec q ⇒ 0");
        assert!(
            !out.contains("init 3 4"),
            "array-sorted mem must not be zero-inited"
        );
    }
}
