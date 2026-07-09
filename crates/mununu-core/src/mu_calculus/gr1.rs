//! Sound GR(1) controller synthesis via direct symbolic fixpoints over the
//! turn-based game arena.
//!
//! # Why this module exists
//!
//! The `ControllerMode::{Functional, SignatureMemory, ProductGame}` extractors
//! synthesise strategies by min-signature selection over a μ-calculus
//! model-checking evaluation. That is **unsound for conjunctive / alternating
//! (GR(1)) objectives**: the evaluation intersects per-conjunct winning regions
//! pointwise ("can force each") rather than requiring a single strategy ("can
//! force both"), and the extraction can pick different controllable moves for
//! different obligations at the same plant state. Oracle-confirmed on
//! `request_grant.tlsf`: the extracted controller violates `G(grant→X!grant)`.
//! See `.claude/plans/gr1-synthesis-engine.md`.
//!
//! # What this module does
//!
//! The turn-based plant is already a two-player game arena: a state whose
//! outgoing transitions are **controllable** is owned by the controller (Eve,
//! ∃), one whose transitions are **uncontrollable** by the environment (Adam,
//! ∀). The load-bearing primitive is the **controllable predecessor** `CPre`.
//! Safety, reachability, Büchi, and the full GR(1) winning region are least/
//! greatest fixpoints over `CPre`. Crucially, **safety is enforced by
//! restricting the arena** (via the `safe` set threaded through `CPre`), not by
//! intersecting a safety region with a liveness region — which is exactly what
//! makes the result sound where the model-checking path over-approximates.
//!
//! This file currently provides the validated game-solving primitives; the
//! GR(1) fixpoint, strategy extraction, and CLI/API wiring build on them.

use crate::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use bitvec::prelude::{BitVec, Lsb0};

/// A set of plant states, indexed by `StateId::index()`.
pub type StateSet = BitVec<usize, Lsb0>;

fn full(n: usize) -> StateSet {
    bitvec::bitvec![usize, Lsb0; 1; n]
}

fn empty(n: usize) -> StateSet {
    bitvec::bitvec![usize, Lsb0; 0; n]
}

/// Controllable predecessor within a `safe`-restricted arena: the set of states
/// from which the controller (Eve) can, in **one step and without leaving
/// `safe`**, force the play into `target`.
///
/// Ownership is read off the plant's controllability:
/// - **Eve-owned** (has ≥1 controllable outgoing edge): Eve chooses, so the
///   state qualifies iff **some** controllable successor is in `target`.
/// - **Adam-owned** (only uncontrollable outgoing edges): the environment
///   chooses, so the state qualifies iff **every** successor is in `target`.
/// - **Deadlock** (no outgoing): Eve cannot move — never in `CPre`.
///
/// Every qualifying source must itself be `safe`; unsafe states are pruned. In
/// the turn-based plant each state is pure (all-controllable or
/// all-uncontrollable), so ownership is unambiguous.
pub fn cpre(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    safe: &StateSet,
    target: &StateSet,
) -> StateSet {
    let n = clts.state_count();
    let mut result = empty(n);
    for s in clts.states() {
        let si = s.index();
        if !safe[si] {
            continue;
        }
        let outs = clts.outgoing(s);
        if outs.is_empty() {
            continue;
        }
        let eve_owned = outs.iter().any(|t| t.is_controllable(clts));
        let qualifies = if eve_owned {
            outs.iter()
                .filter(|t| t.is_controllable(clts))
                .any(|t| target[t.target().index()])
        } else {
            outs.iter().all(|t| target[t.target().index()])
        };
        result.set(si, qualifies);
    }
    result
}

/// `νX. safe ∧ CPre_safe(X)` — the states from which Eve can stay inside `safe`
/// **forever**. Greatest fixpoint: start at all states, shrink to convergence.
pub fn safety_region(clts: &Clts<DefaultStateIdx, DefaultLabelIdx>, safe: &StateSet) -> StateSet {
    let n = clts.state_count();
    let mut x = full(n);
    loop {
        let cp = cpre(clts, safe, &x);
        // next = safe ∧ CPre(x); but CPre already prunes to `safe`, so cp ⊆ safe.
        if cp == x {
            return x;
        }
        x = cp;
    }
}

/// `μX. target ∨ CPre_safe(X)` — the attractor: states from which Eve can force
/// reaching `target` while staying in `safe`. Least fixpoint: grow from
/// `target` to convergence.
pub fn reach(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    safe: &StateSet,
    target: &StateSet,
) -> StateSet {
    let mut x = target.clone();
    // restrict the seed to safe states (Eve cannot rely on unsafe targets)
    x &= safe;
    loop {
        let cp = cpre(clts, safe, &x);
        let mut next = x.clone();
        next |= &cp;
        if next == x {
            return x;
        }
        x = next;
    }
}

/// `νZ. μY. (F ∧ CPre(Z)) ∨ CPre(Y)` — the Büchi winning region: states from
/// which Eve can force visiting `f` **infinitely often** while staying in
/// `safe`. Greatest fixpoint of "reach an `f`-state that can loop back".
pub fn buchi(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    safe: &StateSet,
    f: &StateSet,
) -> StateSet {
    let n = clts.state_count();
    let mut z = full(n);
    z &= safe;
    loop {
        let cz = cpre(clts, safe, &z);
        let mut fz = f.clone();
        fz &= &cz; // F ∧ CPre(Z)
        let next_z = reach(clts, safe, &fz); // μY. fz ∨ CPre(Y)
        if next_z == z {
            return z;
        }
        z = next_z;
    }
}

/// The GR(1) winning region (Piterman–Pnueli–Sá'ar 2006):
///
/// ```text
/// Zwin = νZ. ⋀_j μY. ⋁_i νX. [ (g_j ∧ CPre(Z)) ∨ CPre(Y) ∨ (¬a_i ∧ CPre(X)) ]
/// ```
///
/// where `sys_fair = [g_1..g_m]` are the system guarantee recurrence sets,
/// `env_fair = [a_1..a_n]` the environment assumption recurrence sets, and every
/// `CPre` is taken **inside** the `safe`-restricted arena. Eve wins iff she can,
/// for every guarantee `g_j`, force reaching `g_j` (and loop), UNLESS the
/// environment stops satisfying some assumption `a_i` (the `¬a_i ∧ CPre(X)` term
/// lets Eve "wait out" an unfair environment).
///
/// With `env_fair` empty the assumptions are vacuously true and each guarantee
/// reduces to a plain Büchi objective (`μY. (g_j ∧ CPre(Z)) ∨ CPre(Y)`), so the
/// whole thing is generalized Büchi.
pub fn gr1_win(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    safe: &StateSet,
    sys_fair: &[StateSet],
    env_fair: &[StateSet],
) -> StateSet {
    let n = clts.state_count();
    // No system guarantees ⇒ nothing to force ⇒ stay-safe-forever is enough.
    if sys_fair.is_empty() {
        return safety_region(clts, safe);
    }
    let mut z = safety_region(clts, safe);
    loop {
        // ⋀_j over guarantees.
        let mut z_next = full(n);
        z_next &= safe;
        for g_j in sys_fair {
            let y = gr1_mu_y(clts, safe, g_j, &z, env_fair);
            z_next &= &y;
        }
        if z_next == z {
            return z;
        }
        z = z_next;
    }
}

/// `μY. ⋁_i νX. [ (g_j ∧ CPre(Z)) ∨ CPre(Y) ∨ (¬a_i ∧ CPre(X)) ]` for a fixed
/// guarantee `g_j` and outer `z`. Least fixpoint: grow `Y` from ∅.
fn gr1_mu_y(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    safe: &StateSet,
    g_j: &StateSet,
    z: &StateSet,
    env_fair: &[StateSet],
) -> StateSet {
    let n = clts.state_count();
    let cpre_z = cpre(clts, safe, z);
    let mut g_and_cprez = g_j.clone();
    g_and_cprez &= &cpre_z; // g_j ∧ CPre(Z)
    let mut y = empty(n);
    loop {
        let cpre_y = cpre(clts, safe, &y);
        // base = (g_j ∧ CPre(Z)) ∨ CPre(Y)
        let mut base = g_and_cprez.clone();
        base |= &cpre_y;
        // ⋁_i νX. [ base ∨ (¬a_i ∧ CPre(X)) ]. With no assumptions the νX term is
        // absent and the disjunction is just `base`.
        let disj = if env_fair.is_empty() {
            base.clone()
        } else {
            let mut d = empty(n);
            for a_i in env_fair {
                let nx = gr1_nu_x(clts, safe, &base, a_i);
                d |= &nx;
            }
            d
        };
        // μY step: Y grows to include disj.
        let mut y_next = y.clone();
        y_next |= &disj;
        if y_next == y {
            return y;
        }
        y = y_next;
    }
}

/// `νX. [ base ∨ (¬a_i ∧ CPre(X)) ]` for fixed `base` and assumption `a_i`.
/// Greatest fixpoint: shrink `X` from the safe arena. Captures "Eve reaches
/// `base`, or forces the play to stay in `¬a_i` forever" (env fails its
/// fairness `a_i`, so Eve wins vacuously).
fn gr1_nu_x(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    safe: &StateSet,
    base: &StateSet,
    a_i: &StateSet,
) -> StateSet {
    let n = clts.state_count();
    let mut not_a = a_i.clone();
    not_a = !not_a; // ¬a_i (complement over the full index space)
    let mut x = full(n);
    x &= safe;
    loop {
        let cpre_x = cpre(clts, safe, &x);
        let mut term = not_a.clone();
        term &= &cpre_x; // ¬a_i ∧ CPre(X)
        let mut next = base.clone();
        next |= &term;
        if next == x {
            return x;
        }
        x = next;
    }
}

/// Attractor **ranks** toward `target` within `arena`: `rank[s] = Some(k)` when
/// Eve can force reaching `target` from `s` in exactly `k` controllable-
/// predecessor steps (`0` iff `s ∈ target`), `None` when `s` cannot be forced to
/// `target` inside `arena`. Used to derive a progress-making positional strategy
/// (descend the rank).
pub fn gr1_reach_ranks(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    arena: &StateSet,
    target: &StateSet,
) -> Vec<Option<usize>> {
    let n = clts.state_count();
    let mut rank = vec![None; n];
    let mut current = target.clone();
    current &= arena;
    for i in 0..n {
        if current[i] {
            rank[i] = Some(0);
        }
    }
    let mut r = 0usize;
    loop {
        let cp = cpre(clts, arena, &current);
        let mut added = false;
        for i in 0..n {
            if cp[i] && rank[i].is_none() {
                rank[i] = Some(r + 1);
                current.set(i, true);
                added = true;
            }
        }
        if !added {
            return rank;
        }
        r += 1;
    }
}

/// Positional controller strategy for a **single-guarantee** GR(1) game (the
/// common reactive-control shape, e.g. request_grant). For each controllable
/// (Eve) state in the winning region `z`, choose the outgoing controllable
/// transition whose target has the smallest attractor rank toward
/// `g ∧ CPre(z)` — i.e. the move that makes progress toward satisfying the
/// guarantee while staying winning. Returns `strategy[eve_state] = target index`.
pub fn gr1_strategy_single(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    z: &StateSet,
    g: &StateSet,
) -> std::collections::HashMap<usize, usize> {
    let cpre_z = cpre(clts, z, z);
    let mut base = g.clone();
    base &= &cpre_z; // g ∧ CPre(Z)
    let rank = gr1_reach_ranks(clts, z, &base);
    let big = usize::MAX;
    let mut strat = std::collections::HashMap::new();
    for s in clts.states() {
        let si = s.index();
        if !z[si] {
            continue;
        }
        let outs = clts.outgoing(s);
        // Only controllable (Eve) states have a choice to make.
        if !outs.iter().any(|t| t.is_controllable(clts)) {
            continue;
        }
        let mut best: Option<(usize, usize)> = None; // (rank, target_idx)
        for t in outs.iter().filter(|t| t.is_controllable(clts)) {
            let ti = t.target().index();
            if !z[ti] {
                continue; // never leave the winning region
            }
            let tr = rank[ti].unwrap_or(big);
            if best.is_none_or(|(br, _)| tr < br) {
                best = Some((tr, ti));
            }
        }
        if let Some((_, ti)) = best {
            strat.insert(si, ti);
        }
    }
    strat
}

/// Per-state rank of the `μY` fixpoint for one guarantee `g_j` (the same fixpoint
/// `gr1_win` iterates), given the outer winning region `z`. `rank[s] = Some(k)`
/// means `s` entered `Y_j` at iteration `k` — descending it makes progress toward
/// reaching `g_j` **or** toward an environment-unfairness region (the `νX` wait).
/// Unlike [`gr1_reach_ranks`], this leverages the assumptions, so states from
/// which Eve can only win *because the environment must eventually cooperate* get
/// a finite rank.
pub fn gr1_mu_y_ranks(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    safe: &StateSet,
    g_j: &StateSet,
    z: &StateSet,
    env_fair: &[StateSet],
) -> Vec<Option<usize>> {
    let n = clts.state_count();
    let cpre_z = cpre(clts, safe, z);
    let mut g_and_cprez = g_j.clone();
    g_and_cprez &= &cpre_z;
    let mut rank = vec![None; n];
    let mut y = empty(n);
    let mut k = 0usize;
    loop {
        let cpre_y = cpre(clts, safe, &y);
        let mut base = g_and_cprez.clone();
        base |= &cpre_y;
        let disj = if env_fair.is_empty() {
            base.clone()
        } else {
            let mut d = empty(n);
            for a_i in env_fair {
                d |= &gr1_nu_x(clts, safe, &base, a_i);
            }
            d
        };
        let mut y_next = y.clone();
        y_next |= &disj;
        let mut added = false;
        for i in 0..n {
            if y_next[i] && !y[i] {
                rank[i] = Some(k);
                added = true;
            }
        }
        if !added {
            return rank;
        }
        y = y_next;
        k += 1;
    }
}

/// A memoryful GR(1) controller strategy for **multiple** system guarantees
/// (generalized Büchi). Memory is the guarantee index (mode) `j ∈ 0..m`: the
/// controller pursues `g_j`, and advances to `(j+1) mod m` once `g_j` is reached
/// — round-robin, so every guarantee is served infinitely often.
#[derive(Debug)]
pub struct Gr1MultiStrategy {
    /// `(game_state, mode) → (chosen controllable target, next mode)` for Eve
    /// (controllable) states in the winning region.
    pub moves: std::collections::HashMap<(usize, usize), (usize, usize)>,
    /// Number of modes (= number of guarantees, ≥ 1).
    pub n_modes: usize,
}

/// Extract the memoryful strategy. In mode `j` at state `s`: the mode advances to
/// `(j+1) mod m` iff `s ∈ g_j` (guarantee `j` reached); the controllable move then
/// descends the μY-rank of the (possibly advanced) mode, staying in `z`.
pub fn gr1_strategy_multi(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    safe: &StateSet,
    z: &StateSet,
    sys_fair: &[StateSet],
    env_fair: &[StateSet],
) -> Gr1MultiStrategy {
    let m = sys_fair.len().max(1);
    let ranks: Vec<Vec<Option<usize>>> = sys_fair
        .iter()
        .map(|g| gr1_mu_y_ranks(clts, safe, g, z, env_fair))
        .collect();
    let big = usize::MAX;
    let mut moves = std::collections::HashMap::new();
    for s in clts.states() {
        let si = s.index();
        if !z[si] {
            continue;
        }
        let outs = clts.outgoing(s);
        if !outs.iter().any(|t| t.is_controllable(clts)) {
            continue; // Adam state — env chooses; memory advance handled at emit
        }
        for mode in 0..m {
            // Pursue g_{mode}: pick the controllable move descending mode's μY-rank.
            let mut best: Option<(usize, usize)> = None;
            for t in outs.iter().filter(|t| t.is_controllable(clts)) {
                let ti = t.target().index();
                if !z[ti] {
                    continue;
                }
                let tr = ranks[mode][ti].unwrap_or(big);
                if best.is_none_or(|(br, _)| tr < br) {
                    best = Some((tr, ti));
                }
            }
            if let Some((_, ti)) = best {
                // Advance the mode iff the chosen target reaches guarantee `mode`.
                let next_mode = if sys_fair[mode][ti] {
                    (mode + 1) % m
                } else {
                    mode
                };
                moves.insert((si, mode), (ti, next_mode));
            }
        }
    }
    Gr1MultiStrategy { moves, n_modes: m }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clts::{Clts, LabelControllability};

    /// Build a tiny req/grant-shaped arena:
    ///   env (Adam) --e0--> c0 (Eve) ; env --e1--> c1 (Eve)
    ///   c0 --grant--> g ; c0 --hold--> env
    ///   c1 --grant--> g ; c1 --hold--> env
    ///   g  --back--> env
    /// `grant`/`hold`/`back` controllable; `e0`/`e1` uncontrollable.
    /// State indices: env=0, c0=1, c1=2, g=3.
    fn arena() -> Clts<crate::clts::DefaultStateIdx, crate::clts::DefaultLabelIdx> {
        let mut b = Clts::builder();
        let e0 = b.labels().intern(["e0"]).unwrap();
        let e1 = b.labels().intern(["e1"]).unwrap();
        let grant = b.labels().intern(["grant"]).unwrap();
        let hold = b.labels().intern(["hold"]).unwrap();
        let back = b.labels().intern(["back"]).unwrap();
        b.set_label_controllability(e0, LabelControllability::Uncontrollable);
        b.set_label_controllability(e1, LabelControllability::Uncontrollable);
        b.set_label_controllability(grant, LabelControllability::Controllable);
        b.set_label_controllability(hold, LabelControllability::Controllable);
        b.set_label_controllability(back, LabelControllability::Controllable);
        let env = b.state_with_name("env".into()).unwrap();
        let c0 = b.state_with_name("c0".into()).unwrap();
        let c1 = b.state_with_name("c1".into()).unwrap();
        let g = b.state_with_name("g".into()).unwrap();
        b.initial_state_id(env);
        b.transition_ids(env, &[e0], c0);
        b.transition_ids(env, &[e1], c1);
        b.transition_ids(c0, &[grant], g);
        b.transition_ids(c0, &[hold], env);
        b.transition_ids(c1, &[grant], g);
        b.transition_ids(c1, &[hold], env);
        b.transition_ids(g, &[back], env);
        b.build().unwrap()
    }

    fn set_of(n: usize, idxs: &[usize]) -> StateSet {
        let mut s = empty(n);
        for &i in idxs {
            s.set(i, true);
        }
        s
    }

    #[test]
    fn cpre_eve_and_adam_ownership() {
        let a = arena();
        let n = a.state_count();
        let all = full(n);
        // target = {g}. Eve at c0/c1 can force g (grant). env (Adam) can only
        // force g if ALL its succ are in target — its succ are c0,c1 ∉ {g} — so
        // env ∉ CPre. g has succ env ∉ target → g ∉ CPre.
        let cp = cpre(&a, &all, &set_of(n, &[3]));
        assert!(cp[1] && cp[2], "c0,c1 (Eve) can force g");
        assert!(!cp[0], "env (Adam) cannot force g (has non-g successors)");
        assert!(!cp[3], "g cannot force g in one step");
    }

    #[test]
    fn buchi_grant_infinitely_often_realizable() {
        let a = arena();
        let n = a.state_count();
        let all = full(n);
        // GF g: Eve can always route through g (grant then back). Winning
        // region = all states (every state can reach a g-loop).
        let w = buchi(&a, &all, &set_of(n, &[3]));
        assert!(
            w[0] && w[1] && w[2] && w[3],
            "GF grant realizable from all states, got {w:?}"
        );
    }

    #[test]
    fn buchi_grant_under_never_grant_safety_is_empty() {
        let a = arena();
        let n = a.state_count();
        // Safety "never visit g" removes g from the arena. Then GF g is
        // impossible → winning region empty. This is the contradiction case
        // (GF grant ∧ G ¬grant) that the model-checking path wrongly calls
        // realizable; the arena-constraint makes it correctly UNrealizable.
        let safe = set_of(n, &[0, 1, 2]); // env, c0, c1 — g excluded
        let w = buchi(&a, &safe, &set_of(n, &[3]));
        assert!(
            w.not_any(),
            "GF grant under never-grant safety must be empty, got {w:?}"
        );
    }

    #[test]
    fn safety_region_stay_out_of_g() {
        let a = arena();
        let n = a.state_count();
        let safe = set_of(n, &[0, 1, 2]); // avoid g
        // Eve can stay out of g forever: at c0/c1 play `hold`, env cycles.
        let w = safety_region(&a, &safe);
        assert!(w[0] && w[1] && w[2], "can avoid g from env/c0/c1");
        assert!(!w[3], "g is not in the avoid-g region");
    }

    #[test]
    fn gr1_single_guarantee_no_assumptions_equals_buchi() {
        let a = arena();
        let n = a.state_count();
        let all = full(n);
        let g = set_of(n, &[3]);
        let w = gr1_win(&a, &all, std::slice::from_ref(&g), &[]);
        let b = buchi(&a, &all, &g);
        assert_eq!(w, b, "GR(1) with one guarantee, no assumptions = Büchi");
        assert!(
            w[0] && w[1] && w[2] && w[3],
            "GF grant realizable everywhere"
        );
    }

    #[test]
    fn gr1_contradiction_gf_grant_and_never_grant_is_unrealizable() {
        // The load-bearing soundness case: GF grant ∧ G ¬grant. Safety removes
        // g from the arena; the guarantee GF grant then has empty winning
        // region. The model-checking path wrongly reports this realizable.
        let a = arena();
        let n = a.state_count();
        let safe = set_of(n, &[0, 1, 2]); // G ¬grant: never visit g
        let g = set_of(n, &[3]);
        let w = gr1_win(&a, &safe, std::slice::from_ref(&g), &[]);
        assert!(
            w.not_any(),
            "GF grant ∧ G ¬grant must be UNrealizable, got {w:?}"
        );
    }

    #[test]
    fn gr1_generalized_buchi_two_guarantees() {
        // GF grant ∧ GF (visit env) — the play env→c→g→env satisfies both, so
        // realizable from all states.
        let a = arena();
        let n = a.state_count();
        let all = full(n);
        let g = set_of(n, &[3]); // grant
        let e = set_of(n, &[0]); // env
        let w = gr1_win(&a, &all, &[g, e], &[]);
        assert!(
            w[0] && w[1] && w[2] && w[3],
            "GF grant ∧ GF env realizable, got {w:?}"
        );
    }

    #[test]
    fn multi_guarantee_strategy_reaches_both_guarantees() {
        // GF grant ∧ GF env — the memoryful strategy must round-robin so its
        // (state, mode) product reaches BOTH guarantee sets from init.
        let a = arena();
        let n = a.state_count();
        let all = full(n);
        let g = set_of(n, &[3]); // grant
        let e = set_of(n, &[0]); // env
        let sys_fair = vec![g, e];
        let z = gr1_win(&a, &all, &sys_fair, &[]);
        assert!(z[0], "realizable");
        let strat = gr1_strategy_multi(&a, &all, &z, &sys_fair, &[]);
        // Simulate the (state, mode) product from (env=0, mode=0): Eve follows
        // the strategy, Adam explores all successors; the mode advances on g_mode.
        let m = strat.n_modes;
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![(0usize, 0usize)];
        let (mut hit_grant, mut hit_env) = (false, false);
        while let Some((s, mode)) = stack.pop() {
            if !seen.insert((s, mode)) {
                continue;
            }
            if s == 3 {
                hit_grant = true;
            }
            if s == 0 {
                hit_env = true;
            }
            let next_mode = if sys_fair[mode][s] {
                (mode + 1) % m
            } else {
                mode
            };
            let sid = crate::clts::StateId::<DefaultStateIdx>::from_index(s).unwrap();
            let outs = a.outgoing(sid);
            if outs.iter().any(|t| t.is_controllable(&a)) {
                if let Some(&(t, nm)) = strat.moves.get(&(s, mode)) {
                    stack.push((t, nm));
                }
            } else {
                for t in outs {
                    stack.push((t.target().index(), next_mode));
                }
            }
        }
        assert!(
            hit_grant && hit_env,
            "multi-guarantee strategy product must reach both grant and env"
        );
    }

    #[test]
    fn gr1_assumption_rescues_otherwise_unwinnable_guarantee() {
        // Arena where Eve can reach `g` ONLY when the environment cooperates by
        // going to c1 (req=1); at c0 (req=0) she is forced back to env with no
        // path to g. Guarantee GF g is then UNwinnable unconditionally, but
        // becomes winnable under the assumption GF c1 (env visits c1 infinitely
        // often ⇒ Eve grants there; else env fails its fairness and Eve wins).
        let mut b = Clts::builder();
        let e0 = b.labels().intern(["e0"]).unwrap();
        let e1 = b.labels().intern(["e1"]).unwrap();
        let grant = b.labels().intern(["grant"]).unwrap();
        let hold = b.labels().intern(["hold"]).unwrap();
        let back = b.labels().intern(["back"]).unwrap();
        b.set_label_controllability(e0, LabelControllability::Uncontrollable);
        b.set_label_controllability(e1, LabelControllability::Uncontrollable);
        b.set_label_controllability(grant, LabelControllability::Controllable);
        b.set_label_controllability(hold, LabelControllability::Controllable);
        b.set_label_controllability(back, LabelControllability::Controllable);
        let env = b.state_with_name("env".into()).unwrap(); // 0 (Adam)
        let c0 = b.state_with_name("c0".into()).unwrap(); // 1 (Eve, req=0)
        let c1 = b.state_with_name("c1".into()).unwrap(); // 2 (Eve, req=1)
        let g = b.state_with_name("g".into()).unwrap(); // 3
        b.initial_state_id(env);
        b.transition_ids(env, &[e0], c0);
        b.transition_ids(env, &[e1], c1);
        b.transition_ids(c0, &[hold], env); // c0 CANNOT reach g
        b.transition_ids(c1, &[grant], g); // only c1 can grant
        b.transition_ids(c1, &[hold], env);
        b.transition_ids(g, &[back], env);
        let a = b.build().unwrap();
        let n = a.state_count();
        let all = full(n);
        let gset = set_of(n, &[3]);
        let c1set = set_of(n, &[2]);

        // Without assumptions: env can sit at c0 forever (Adam picks e0), so GF g
        // is not forceable from env. env ∉ winning region.
        let w_no = gr1_win(&a, &all, std::slice::from_ref(&gset), &[]);
        assert!(!w_no[0], "unconditionally, env cannot be forced to grant");

        // With assumption GF c1: if env visits c1 infinitely, Eve grants there;
        // otherwise env violates its own fairness and Eve wins. env ∈ region.
        let w_yes = gr1_win(
            &a,
            &all,
            std::slice::from_ref(&gset),
            std::slice::from_ref(&c1set),
        );
        assert!(
            w_yes[0],
            "under GF c1 assumption, env is winning, got {w_yes:?}"
        );
    }

    // ---- request_grant, as a monitor-augmented round game ----
    //
    // Round: env picks `req` (Adam), then ctrl picks `grant` (Eve). Monitor bits:
    //   pending' = (pending ∨ req) ∧ ¬grant     (response  G(req→F grant) ≡ GF ¬pending)
    //   was_grant' = grant
    // Safety G(grant→X!grant): granting when `was_grant` already holds → BAD (unsafe).
    // States: E{p}{w} (env-turn, Adam), C{p}{w}{r} (ctrl-turn, Eve), BAD (unsafe sink).
    // env_fair = GF req = {C**1}; sys_fair = GF ¬pending = {E0*}; safe = all but BAD.
    struct RgGame {
        clts: Clts<crate::clts::DefaultStateIdx, crate::clts::DefaultLabelIdx>,
        safe: StateSet,
        sys_fair: Vec<StateSet>,
        env_fair: Vec<StateSet>,
        init: usize,
        /// state name → index, so tests can decode the strategy.
        id: std::collections::HashMap<String, usize>,
    }

    fn build_request_grant_game(forbid_grant: bool) -> RgGame {
        use std::collections::HashMap;
        let mut b = Clts::builder();
        let e0 = b.labels().intern(["e0"]).unwrap();
        let e1 = b.labels().intern(["e1"]).unwrap();
        let g0 = b.labels().intern(["g0"]).unwrap();
        let g1 = b.labels().intern(["g1"]).unwrap();
        let bad_l = b.labels().intern(["bad"]).unwrap();
        b.set_label_controllability(e0, LabelControllability::Uncontrollable);
        b.set_label_controllability(e1, LabelControllability::Uncontrollable);
        b.set_label_controllability(g0, LabelControllability::Controllable);
        b.set_label_controllability(g1, LabelControllability::Controllable);
        b.set_label_controllability(bad_l, LabelControllability::Uncontrollable);

        let mut id: HashMap<String, _> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        for p in 0..2 {
            for w in 0..2 {
                names.push(format!("E{p}{w}"));
            }
        }
        for p in 0..2 {
            for w in 0..2 {
                for r in 0..2 {
                    names.push(format!("C{p}{w}{r}"));
                }
            }
        }
        names.push("BAD".into());
        for name in &names {
            let sid = b.state_with_name(name.clone()).unwrap();
            id.insert(name.clone(), sid);
        }
        let bad = id["BAD"];
        let e00 = id["E00"];
        b.initial_state_id(e00);

        // env moves (uncontrollable): E{p}{w} -e{r}-> C{p}{w}{r}
        for p in 0..2 {
            for w in 0..2 {
                let e = id[&format!("E{p}{w}")];
                b.transition_ids(e, &[e0], id[&format!("C{p}{w}0")]);
                b.transition_ids(e, &[e1], id[&format!("C{p}{w}1")]);
            }
        }
        // ctrl moves (controllable): from C{p}{w}{r}
        for p in 0..2 {
            for w in 0..2 {
                for r in 0..2 {
                    let c = id[&format!("C{p}{w}{r}")];
                    // grant = 0: pending' = (p|r), was_grant' = 0
                    let pp = if (p | r) == 1 { 1 } else { 0 };
                    b.transition_ids(c, &[g0], id[&format!("E{pp}0")]);
                    // grant = 1: unsafe if forbid_grant, or if was_grant (w==1); else E{0}{1}
                    if forbid_grant || w == 1 {
                        b.transition_ids(c, &[g1], bad);
                    } else {
                        b.transition_ids(c, &[g1], id["E01"]);
                    }
                }
            }
        }
        // BAD self-loop (uncontrollable) so it is a well-defined sink.
        b.transition_ids(bad, &[bad_l], bad);

        let clts = b.build().unwrap();
        let n = clts.state_count();
        let mut safe = full(n);
        safe.set(bad.index(), false);
        // env_fair = GF req = { C**1 }
        let mut ef = empty(n);
        for p in 0..2 {
            for w in 0..2 {
                ef.set(id[&format!("C{p}{w}1")].index(), true);
            }
        }
        // sys_fair = GF ¬pending = { E0* }
        let mut sf = empty(n);
        sf.set(id["E00"].index(), true);
        sf.set(id["E01"].index(), true);
        let id_idx: std::collections::HashMap<String, usize> =
            id.iter().map(|(k, v)| (k.clone(), v.index())).collect();
        RgGame {
            clts,
            safe,
            sys_fair: vec![sf],
            env_fair: vec![ef],
            init: e00.index(),
            id: id_idx,
        }
    }

    #[test]
    fn gr1_request_grant_is_realizable() {
        let game = build_request_grant_game(false);
        let w = gr1_win(&game.clts, &game.safe, &game.sys_fair, &game.env_fair);
        assert!(
            w[game.init],
            "request_grant GR(1) must be REALIZABLE from init, got win={w:?}"
        );
    }

    #[test]
    fn gr1_request_grant_strategy_is_safe_and_serving() {
        let game = build_request_grant_game(false);
        let z = gr1_win(&game.clts, &game.safe, &game.sys_fair, &game.env_fair);
        assert!(z[game.init], "must be realizable");
        // Single guarantee (sys_fair[0] = GF ¬pending).
        let strat = gr1_strategy_single(&game.clts, &z, &game.sys_fair[0]);
        let e01 = game.id["E01"];
        // grant chosen iff the strategy target is E01 (the only safe g1 target).
        let grant_at = |p: usize, w: usize, r: usize| -> Option<bool> {
            let c = game.id[&format!("C{p}{w}{r}")];
            if !z[c] {
                return None; // not in winning region
            }
            strat.get(&c).map(|&t| t == e01)
        };
        for p in 0..2 {
            for r in 0..2 {
                // SAFETY: never grant when was_grant already holds (w == 1).
                if let Some(g) = grant_at(p, 1, r) {
                    assert!(
                        !g,
                        "must NOT grant at C{p}1{r} (would violate G(grant→X!grant))"
                    );
                }
            }
        }
        // SERVING: when there is a request/pending and it is safe to grant
        // (was_grant == 0), the controller grants.
        assert_eq!(
            grant_at(0, 0, 1),
            Some(true),
            "grant a fresh request when safe"
        );
        assert_eq!(
            grant_at(1, 0, 0),
            Some(true),
            "grant a pending request when safe"
        );
        assert_eq!(
            grant_at(1, 0, 1),
            Some(true),
            "grant when pending+request and safe"
        );
    }

    /// Emits the extracted GR(1) controller as a Mealy SV module for the btormc
    /// oracle. Gated behind an env var so it only runs when explicitly requested
    /// (writes a file); the strategy itself is validated by the tests above.
    #[test]
    fn gr1_request_grant_emit_mealy_sv() {
        let Ok(out_path) = std::env::var("GR1_EMIT_SV") else {
            return;
        };
        let game = build_request_grant_game(false);
        let z = gr1_win(&game.clts, &game.safe, &game.sys_fair, &game.env_fair);
        let strat = gr1_strategy_single(&game.clts, &z, &game.sys_fair[0]);
        let e01 = game.id["E01"];
        // grant(p,w,r): the controller's Moore/Mealy output. From C{p}{w}{r} the
        // chosen target E01 ⟺ grant. Missing (unreachable-in-Z) ⇒ 0.
        let mut terms: Vec<String> = Vec::new();
        for p in 0..2 {
            for w in 0..2 {
                for r in 0..2 {
                    let c = game.id[&format!("C{p}{w}{r}")];
                    let grant = z[c] && strat.get(&c) == Some(&e01);
                    if grant {
                        terms.push(format!("(pending=={p} && was_grant=={w} && req=={r})"));
                    }
                }
            }
        }
        let grant_expr = if terms.is_empty() {
            "1'b0".to_string()
        } else {
            terms.join(" || ")
        };
        let sv = format!(
            "module gr1_ctrl (input logic clk, input logic req, output logic grant, output logic bad_o);\n\
             \x20 logic pending = 1'b0;\n\
             \x20 logic was_grant = 1'b0;\n\
             \x20 assign grant = ({grant_expr});\n\
             \x20 // SAFETY monitor: G(grant -> X !grant) violated iff grant two rounds running\n\
             \x20 assign bad_o = was_grant & grant;\n\
             \x20 always_ff @(posedge clk) begin\n\
             \x20\x20\x20 pending   <= (pending | req) & ~grant;\n\
             \x20\x20\x20 was_grant <= grant;\n\
             \x20 end\n\
             endmodule\n"
        );
        std::fs::write(&out_path, sv).unwrap();
        eprintln!("wrote GR(1) controller SV to {out_path}");
    }

    #[test]
    fn gr1_request_grant_unrealizable_when_grant_forbidden() {
        // Forbidding grant (a safety violation on every g1) leaves the response
        // GF ¬pending unsatisfiable under GF req: a request sets pending and it
        // can never clear. Correctly UNrealizable.
        let game = build_request_grant_game(true);
        let w = gr1_win(&game.clts, &game.safe, &game.sys_fair, &game.env_fair);
        assert!(
            !w[game.init],
            "grant-forbidden request_grant must be UNrealizable, got win={w:?}"
        );
    }
}
