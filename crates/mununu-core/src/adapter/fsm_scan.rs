//! P1 — automatic FSM recoverability scan (no user input).
//!
//! Discovers the FSM-like state registers of a BTOR2 design, derives each one's
//! idle/reset target from the design (its `init` value, or the reset-mux constant of an
//! init-less yosys lift — spike-validated on the real OpenTitan `csrng_main_sm`), and
//! checks recoverability `AG EF (reg == idle)` with the reset left **free**.
//!
//! # Why reset-free is the intended-vs-unintended filter
//!
//! A `VIOLATED` verdict means: a reachable state exists from which the FSM can never
//! get back to its reset state — **even with reset available**. That is a genuine
//! *unrecoverable trap*, a real finding. A design's *intended* reset-dependence (the
//! FSM recovers only when reset is asserted, e.g. an error state cleared by reset)
//! still `HOLDS` here precisely because reset is free — so intended terminals do not
//! show up as findings. This is the automatic bug-finder for the branching property
//! SVA cannot express, on the small FSM cone where mununu is strongest and needs no
//! predicates.
//!
//! # Idle derivation (two sources — spike-validated 2026-07-08)
//!
//! The idle/reset target is derived from the design, in order:
//!
//! 1. **BTOR2 `init` value.** Hand-written / HWMCC-style / mununu-abstraction BTOR2
//!    declares the reset state with an `init` line.
//! 2. **Reset-mux constant.** yosys-lifted RTL (sv2v → yosys → BTOR2) models reset as an
//!    *explicit signal* and carries **no `init` lines**; instead the register's `next`
//!    is a top-level `ite(reset_input, normal_logic, RESET_CONST)`. The constant branch
//!    of that reset mux is the reset state ([`derive_reset_idle`]). This is exactly how
//!    the real OpenTitan `csrng_main_sm` encodes `MainSmIdle = 55` — so the scan works
//!    end-to-end on real RTL with no user input.
//!
//! A register with neither an init nor a recognizable reset mux is skipped (not wrong).
//!
//! # Scope (honest limits)
//!
//! - **Named state.** A register is scanned if a symbol names it — directly on the
//!   `state` cell or via a `uext`/reset-mux alias ([`parser::resolve_state_alias`]),
//!   which is how yosys surfaces `state_q`. Unnamed internal flops are not scanned.
//! - **Reset-free framing.** With reset left free, asserting reset forces a well-formed
//!   FSM back to idle, so recoverability `HOLDS` for any register whose reset
//!   unconditionally restores idle. A `VIOLATED` verdict therefore isolates the genuine
//!   bug class: a reachable state from which the FSM cannot reach idle **even with reset
//!   available** — a gated/ineffective reset, a wrong idle encoding, or an upstream trap.
//!   Intended reset-*dependence* (recovers only when reset is asserted) still `HOLDS`.
//! - Each check runs the exact 3-valued engine over the whole design; on a design
//!   whose *total* state exceeds the engine's cone cap the verdict is `Unknown`
//!   (skipped, not wrong). Routing the over-cap case to the cube + `smt-hyper-must`
//!   path is a P0.2 follow-up.
//! - `max_width` filters which registers count as "FSM-like" (narrow enum state, not a
//!   wide datapath / counter — recoverability of a counter to 0 is not a meaningful
//!   bug).

use crate::adapter::btor2::ast::{Btor2File, Nid, Node, Op};
use crate::adapter::btor2::bit_blast::resolve_btor2_constant;
use crate::adapter::btor2::parser;
use crate::adapter::recoverability::verify_recoverability;
use crate::verdict::PropertyVerdict;
use std::collections::{HashMap, HashSet};

/// The default "FSM-like" width bound: a state register wider than this is treated as
/// a datapath / counter and skipped (256 encodings is a generous enum ceiling).
pub const DEFAULT_FSM_MAX_WIDTH: u32 = 8;

/// One register's recoverability result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsmFinding {
    /// The state register's symbol.
    pub register: String,
    /// The idle/reset value recoverability targets (the register's init or reset-mux
    /// constant).
    pub idle_value: u64,
    /// `Holds` = always recoverable to idle; `Violated` = an **unrecoverable trap**
    /// (a finding); `Unknown` = over the exact engine's cap (skipped).
    pub verdict: PropertyVerdict,
}

impl FsmFinding {
    /// A `Violated` verdict is a real finding (an unrecoverable trap).
    pub fn is_finding(&self) -> bool {
        self.verdict == PropertyVerdict::Violated
    }
}

/// Scan `btor2` for FSM-like state registers and check recoverability-to-init of each
/// with reset free. Returns one [`FsmFinding`] per checked register (init-less or
/// too-wide registers are skipped silently). See the module docs.
pub fn fsm_recoverability_scan(btor2: &str, max_width: u32) -> Result<Vec<FsmFinding>, String> {
    let file = parser::parse(btor2).map_err(|e| format!("parsing BTOR2: {}", e.message))?;
    let symbols = parser::collect_symbols(&file);

    // Enumerate NAMED signals and resolve each to its underlying state cell — directly
    // (the symbol is on the `state` node) or via a `uext`/reset-mux alias (the symbol is
    // on an `Op`/`Output`, as yosys surfaces `state_q`). Dedup by the resolved cell so an
    // alias plus its own cell aren't scanned twice. Deterministic order by symbol nid.
    let mut named: Vec<(&Nid, &String)> = symbols.iter().collect();
    named.sort_by_key(|(nid, _)| **nid);

    let mut seen_cells: HashSet<Nid> = HashSet::new();
    let mut out = Vec::new();
    for (_, sym) in named {
        // `allow_reset_mux = true`: yosys surfaces `state_q` as `uext(ite(rst, cell,
        // reset_const))`, so the name resolves to its cell only through the reset mux.
        let Some(state_nid) = parser::resolve_state_alias(&file, sym, true) else {
            continue;
        };
        if !seen_cells.insert(state_nid) {
            continue;
        }
        // The resolved cell must be an FSM-width state.
        let Some(Node::State { sort, .. }) = file.lookup(state_nid).map(|l| &l.node) else {
            continue;
        };
        let Some(width) = parser::bv_width(&file, *sort) else {
            continue;
        };
        if width == 0 || width > max_width {
            continue;
        }
        // idle = the register's init value if present, else the reset-mux constant of an
        // init-less yosys lift; skip a register whose idle we can't pin down.
        let Some(idle) = derive_idle(&file, state_nid, &symbols) else {
            continue;
        };

        // AG EF (reg == idle) with reset FREE (reset_pinned = false inside).
        let verdict = verify_recoverability(btor2, &format!("{sym} == {idle}"))?;
        out.push(FsmFinding {
            register: sym.clone(),
            idle_value: idle,
            verdict,
        });
    }
    Ok(out)
}

/// Derive a state register's idle/reset value: its BTOR2 `init` value if present, else
/// the constant branch of its reset mux (init-less yosys lifts). `None` if neither.
fn derive_idle(file: &Btor2File, state_nid: Nid, symbols: &HashMap<Nid, String>) -> Option<u64> {
    // 1. An explicit `init` constant.
    let init = file.lines.iter().find_map(|l| match &l.node {
        Node::Init { state, value, .. } if *state == state_nid => Some(*value),
        _ => None,
    });
    if let Some(v) = init.and_then(|op| resolve_btor2_constant(file, op.nid())) {
        return Some(v);
    }
    // 2. The reset-mux constant of an init-less register.
    derive_reset_idle(file, state_nid, symbols)
}

/// Derive the reset value of an init-less state cell whose `next` is a top-level reset
/// mux `ite(reset_input, normal_logic, RESET_CONST)`. yosys `async2sync` lowers an RTL
/// reset to exactly this shape. Returns the constant branch when the condition is a
/// reset-named input and exactly one branch is a constant (the other being the normal
/// next-state logic, which is never a bare constant). Polarity-independent.
fn derive_reset_idle(
    file: &Btor2File,
    state_nid: Nid,
    symbols: &HashMap<Nid, String>,
) -> Option<u64> {
    // The register's `next` value node.
    let next_val = file.lines.iter().find_map(|l| match &l.node {
        Node::Next { state, value, .. } if *state == state_nid => Some(*value),
        _ => None,
    })?;
    // It must be a top-level `ite(cond, then, else)`.
    let Node::Op {
        op: Op::Ite, args, ..
    } = &file.lookup(next_val.nid())?.node
    else {
        return None;
    };
    let [cond, then_op, else_op] = args[..] else {
        return None;
    };
    // The condition must reference a reset-named input (guards against picking up a
    // normal FSM transition `ite(go, SOME_STATE_const, stay)` as if it were a reset).
    if !symbols.get(&cond.nid()).is_some_and(|s| is_reset_name(s)) {
        return None;
    }
    // The reset value is the constant branch; the normal-logic branch is not a constant.
    match (
        resolve_btor2_constant(file, then_op.nid()),
        resolve_btor2_constant(file, else_op.nid()),
    ) {
        (Some(c), None) | (None, Some(c)) => Some(c),
        _ => None, // both-const / neither-const is ambiguous — skip
    }
}

/// Heuristic: does this signal name a reset input? Matches `rst` / `reset` roots with
/// the usual polarity suffixes (`rst_ni`, `rst_n`, `reset`, `arst_n`, `resetn`, …).
fn is_reset_name(sym: &str) -> bool {
    let s = sym.to_ascii_lowercase();
    s.contains("rst") || s.contains("reset")
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 3-state responder FSM: st 0=idle,1=req,2=grant; always returns to idle.
    // AG EF (st == 0) HOLDS ⇒ no finding.
    const RESPONDER: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 st
4 zero 1
5 init 1 3 4
6 input 2 go
7 one 1
8 constd 1 2
9 eq 2 3 4
10 eq 2 3 7
11 ite 1 6 7 4
12 ite 1 10 8 4
13 ite 1 9 11 12
14 next 1 3 13
";

    // A 4-state staller: st 0=idle,1=req,3=stuck (absorbing); 2=grant unreachable.
    // From `stuck` it can NEVER get back to idle ⇒ AG EF (st == 0) VIOLATED ⇒ a finding.
    const STALLER: &str = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 st
4 zero 1
5 init 1 3 4
6 input 2 go
7 one 1
8 constd 1 2
9 constd 1 3
10 eq 2 3 4
11 eq 2 3 7
12 ite 1 6 7 4
13 ite 1 11 9 3
14 ite 1 10 12 13
15 next 1 3 14
";

    #[test]
    fn responder_scan_finds_no_trap() {
        let findings = fsm_recoverability_scan(RESPONDER, DEFAULT_FSM_MAX_WIDTH).expect("scan");
        assert_eq!(findings.len(), 1, "one FSM register (st)");
        assert_eq!(findings[0].register, "st");
        assert_eq!(findings[0].idle_value, 0);
        assert_eq!(findings[0].verdict, PropertyVerdict::Holds);
        assert!(
            !findings[0].is_finding(),
            "responder always recovers to idle"
        );
    }

    #[test]
    fn staller_scan_finds_the_unrecoverable_trap() {
        let findings = fsm_recoverability_scan(STALLER, DEFAULT_FSM_MAX_WIDTH).expect("scan");
        let st = findings
            .iter()
            .find(|f| f.register == "st")
            .expect("st register scanned");
        assert_eq!(
            st.verdict,
            PropertyVerdict::Violated,
            "stuck state is a trap"
        );
        assert!(st.is_finding(), "the absorbing trap is a real finding");
    }

    // An init-LESS FSM whose reset is an explicit active-low `rst_ni`, lowered to a
    // top-level reset mux `next = rst_ni ? normal : IDLE_CONST` exactly as yosys
    // `async2sync` emits. idle=5 must be recovered from the mux constant (no init line).
    // Normal logic cycles 5→1→0→5, so AG EF (st == 5) HOLDS ⇒ no finding.
    const RESET_MUX: &str = "\
1 sort bitvec 1
2 sort bitvec 3
3 state 2 st
4 input 1 rst_ni
5 constd 2 5
6 constd 2 1
7 zero 2
8 eq 1 3 5
9 eq 1 3 6
10 ite 2 9 7 5
11 ite 2 8 6 10
12 ite 2 4 11 5
13 next 2 3 12
";

    #[test]
    fn reset_mux_idle_is_recovered_without_an_init_line() {
        let findings = fsm_recoverability_scan(RESET_MUX, DEFAULT_FSM_MAX_WIDTH).expect("scan");
        let st = findings
            .iter()
            .find(|f| f.register == "st")
            .expect("st register scanned (idle derived from the reset mux)");
        assert_eq!(
            st.idle_value, 5,
            "idle = the reset-mux constant, not an init"
        );
        assert_eq!(
            st.verdict,
            PropertyVerdict::Holds,
            "the FSM cycles back to idle"
        );
        assert!(!st.is_finding());
    }

    #[test]
    fn wide_registers_are_skipped() {
        // A 16-bit datapath register (init 0, holds): too wide to be FSM-like.
        const WIDE: &str =
            "1 sort bitvec 16\n2 state 1 data\n3 zero 1\n4 init 1 2 3\n5 next 1 2 2\n";
        let findings = fsm_recoverability_scan(WIDE, DEFAULT_FSM_MAX_WIDTH).expect("scan");
        assert!(
            findings.is_empty(),
            "a 16-bit datapath register is not FSM-like (max_width=8)"
        );
    }
}
