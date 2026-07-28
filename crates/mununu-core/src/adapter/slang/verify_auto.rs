//! XL.6b — automated SVA verification (SV source → per-property verdict, no sidecar).
//!
//! The headline no-sidecar verify path. Composes the shipped pieces:
//!
//! 1. [`extract_sva`](crate::adapter::slang::extract::extract_sva) — slang →
//!    translated mu-calculus property set (+ `__past` shadow requirements).
//! 2. [`sv_to_btor2`](crate::adapter::yosys::sv_to_btor2) — SV → flattened BTOR2.
//! 3. [`augment_with_past_shadows`](crate::adapter::btor2::shadow::augment_with_past_shadows)
//!    — synthesise the 1-step shadow flops the Tier-2 history formulas reference.
//! 4. Per property: auto-seed cube predicates from the formula's state-cell atoms
//!    (the minimal H.1, [`seed_from_formula`]) → [`cegar_refine_loop`] → verdict.
//!
//! **Binding.** Each seeded cube predicate is NAMED exactly the formula atom
//! string, so the evaluator's name-match bridge (the M.4 fix in `evaluator.rs`)
//! binds the formula's `Node::Predicate` to its cube bit. Simple `reg == value`
//! atoms seed a [`PredicateSpec`]; relational / compound atoms (`reg == reg`
//! incl. `$stable`'s `state_q == state_q__past`, `reg != value`, …) seed a
//! compound predicate via a synthesised `SvAnnotation` sidecar that
//! [`cegar_refine_loop`] consumes (forcing the SmtAllPairs eager lift).
//!
//! **Scope (sound gate).** The cube abstraction binds predicates over **state
//! cells** and (H.B) over primary **inputs** as *free* cube dimensions. A simple
//! `input == value` atom (an arbiter's `req_i`, sysrst's `cfg_enable_i` /
//! `trigger_*`) becomes a free dimension whose source-pin / target-free shape the
//! SMT may/must seam realises (`build_register_nid_map_with_inputs`), and whose
//! verdict is read across every initial environment value — the "for all input
//! sequences" over-approximation (see `docs/design/free-input-atoms.md`).
//!
//! A relational compound carrying an **input** operand (sysrst's
//! `cnt_q >= cfg_*_timer_i`) BINDS as a derived RELATIONAL label (H.F): the
//! per-cube 3-valued labeller resolves each operand over the uniform source
//! image (state ∪ inputs ∪ combinational), so the property reaches an honest ⊥
//! rather than skipping.
//!
//! A **combinational output whose cone is state-only** (`event_detected_o =
//! f(state_q)`) binds as a CUBE DIMENSION (H.U.2): the uniform predicate-image
//! resolves its combinational node (the `Op`/`output`-line symbol map below) and
//! pins its value per cube, so the property reaches a **definite** verdict.
//!
//! A combinational whose cone **reaches a free input** (`trigger_active =
//! !trigger_i`; csrng's `main_sm_err_o`, whose cone reaches `enable_i`) binds as
//! a derived ⊥-label (H.E) — definite where the labeller proves it constant over
//! the cube, ⊥ where the input can swing it. **Slice 3 (the unwrap lever)** then
//! seeds the raw inputs in that cone (`trigger_i`) as free H.B cube dimensions:
//! this refines the may-relation so a consequent box over a transition GOVERNED
//! by the input (`AG((state ∧ trigger) → AX state′)`) becomes definite (Kleene
//! `⊥ ∨ [] = T`), and the derived label itself turns definite at cubes pinning
//! the input. The raw input is the H.B source-pin / target-free shape (sound; no
//! fabricated must-edge). This is what lifts sysrst's trigger-governed
//! conditional-transition safety SVA (sva_4/6/8/9) from ⊥ to definite HOLDS.
//! Gated on a small cone-input count so a wide combinational does not blow up the
//! cube (it stays a bare ⊥-label).
//!
//! What is still **Skipped** (never given a misleading verdict): an atom over a
//! combinational signal with **no resolvable node** in the lifted BTOR2 — one
//! Yosys `flatten`/`opt` dropped, leaving no `state` / `input` / `Op` / `output`
//! symbol to map. It is reported Skipped with a reason — labelling what we
//! cannot resolve would fall through to the evaluator's "unknown ⇒ false"
//! under-approx and silently produce a vacuous verdict.

use std::collections::HashSet;

use crate::adapter::btor2::kmts_lift::{MayEdgeInference, MustEdgeInference, PredicateSpec};
use crate::adapter::btor2::predicate_expr::{CmpOp, PredicateExpr, parse_predicate_expr};
use crate::adapter::slang::extract::extract_sva_with_options;
use crate::adapter::slang::translate::{SvaKind, TranslateOptions};
use crate::adapter::yosys::YosysOptions;
use crate::adapter::{AdapterError, AdapterErrorKind};
use crate::mu_calculus::{Formula, Node};

/// Per-property verification outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// No definite-false and no ⊥ cube cells — the property holds on the abstraction.
    Holds,
    /// Some cube cells are definite-false (a violation witness exists).
    Violated { false_cells: usize },
    /// The verdict carries ⊥ (unknown) cells — the abstraction couldn't decide.
    Unknown { unknown_cells: usize },
    /// Not verified, with the reason: an atom over a non-state signal
    /// (combinational/IO — not cube-bindable), no atoms at all, or an error.
    Skipped { reason: String },
}

/// One assertion's auto-verification result.
#[derive(Debug, Clone)]
pub struct PropertyVerdict {
    pub name: String,
    pub kind: SvaKind,
    pub formula: String,
    pub outcome: VerifyOutcome,
    /// The cube predicates auto-seeded for this property (atom strings) — the
    /// diagnostic "what was tracked".
    pub seeded_predicates: Vec<String>,
    /// D1.8b — a concrete stall-lasso counterexample, present only when the exact
    /// engine (`--engine exact-symbolic`) reports a bare `AF p` property `Violated`
    /// and the stall is reachable at the reset state. `None` otherwise.
    pub counterexample: Option<ExactCounterexample>,
}

/// D1.8b — a concrete stall-lasso counterexample for a `Violated` liveness property
/// from the exact engine: a `prefix` from the reset state into a repeating `cycle`,
/// each state a list of `(register, value)` pairs in register order. The cycle
/// witnesses that the property is avoided forever (the "stall").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactCounterexample {
    /// States from the reset state up to (excluding) the cycle entry.
    pub prefix: Vec<Vec<(String, u64)>>,
    /// The repeating stall cycle; the last state steps back to `cycle[0]`.
    pub cycle: Vec<Vec<(String, u64)>>,
    /// P3 — the INPUT assignment driving each transition of `prefix ++ cycle` (`inputs[i]` is
    /// the input at path-state `i`), for RTL replay. Empty when the engine did not record inputs.
    pub inputs: Vec<Vec<(String, u64)>>,
    /// A.4 — the UNREACHABLE-target witness for a `Violated` bare `EF p` (reachability):
    /// there is no trace (`prefix`/`cycle` empty), because the target `p` is never reached
    /// from reset. These are the predicate-atom strings comprising `p` — the actionable
    /// repair signal "the design never reaches `<atoms>`". Empty for the stall-lasso /
    /// trap-path (liveness/recoverability) counterexamples, which carry a concrete path.
    pub unreachable_target: Vec<String>,
}

/// Convert an engine [`StallLasso`](crate::adapter::btor2::symbolic_bitblast::StallLasso)
/// (register→value maps, `u128`) into the surface [`ExactCounterexample`] (ordered
/// `(name, u64)` pairs; state-register values fit in `u64` under the exact engine's
/// bit-blast cap).
fn exact_counterexample_from_lasso(
    lasso: crate::adapter::btor2::symbolic_bitblast::StallLasso,
) -> ExactCounterexample {
    let conv = |states: Vec<std::collections::BTreeMap<String, u128>>| {
        states
            .into_iter()
            .map(|st| st.into_iter().map(|(k, v)| (k, v as u64)).collect())
            .collect()
    };
    ExactCounterexample {
        prefix: conv(lasso.prefix),
        cycle: conv(lasso.cycle),
        inputs: conv(lasso.inputs),
        unreachable_target: Vec::new(),
    }
}

/// A.4 — build the UNREACHABLE-target counterexample for a `Violated` bare `EF p`
/// (`μX. (p ∨ ◇X)`): the violation *is* the proof that `p` is unreachable from reset,
/// so no trace exists. Returns the target's predicate atoms as the witness (empty
/// path). `None` when `formula` is not a bare `EF` shape (a liveness/recoverability
/// failure keeps its concrete stall-lasso / trap-path witness instead).
fn ef_unreachable_counterexample(
    formula: &crate::mu_calculus::Formula,
) -> Option<ExactCounterexample> {
    crate::adapter::btor2::symbolic_bitblast::ef_target_atoms(formula).map(|atoms| {
        ExactCounterexample {
            prefix: Vec::new(),
            cycle: Vec::new(),
            inputs: Vec::new(),
            unreachable_target: atoms,
        }
    })
}

/// RANKING RESCUE — route a recoverability `AG EF <good>` whose `good` is a single
/// `reg == value` / `reg == reg` atom to the scalable recoverability engine
/// ([`verify_recoverability_scalable`](crate::adapter::recoverability)), which proves
/// `AG EF good` by a well-founded datapath **ranking** (a down-counter/drain to a
/// value) BEFORE its cube. This decides the deep-counter recoverability class that the
/// bit-blaster is over-cap for and the predicate cube leaves ⊥ — e.g. a timer/FIFO
/// `AG EF (cnt == N)` at counter widths the exact engine can't fit. Used as a fallback
/// (not a replacement) — `None` when the formula is not that shape, `good` is not a
/// single register-comparison atom, or the engine abstains, so the caller keeps its
/// Skipped/⊥ verdict. The ranking certificate is a **sound sufficient** condition (it
/// establishes the stronger `AG AF good`), and the engine abstains rather than emit an
/// unsound Holds on a UF-wrapping datapath (PR #301).
fn try_ranking_recoverability(
    btor2: &str,
    formula: &crate::mu_calculus::Formula,
) -> Option<VerifyOutcome> {
    let atoms = crate::adapter::btor2::symbolic_bitblast::ag_ef_good_atoms(formula)?;
    let [good] = atoms.as_slice() else {
        return None; // only a single register-comparison good routes to the engine
    };
    match crate::adapter::recoverability::verify_recoverability_scalable(btor2, good, &[]) {
        Ok(crate::verdict::PropertyVerdict::Holds) => Some(VerifyOutcome::Holds),
        Ok(crate::verdict::PropertyVerdict::Violated) => {
            Some(VerifyOutcome::Violated { false_cells: 1 })
        }
        _ => None, // Unknown / Skipped / Err → keep the caller's verdict
    }
}

/// Model-level diagnostics for an automated verification run — what the lift
/// produced, so a "couldn't verify" outcome is traceable to a root cause
/// (rather than only a per-property symptom like "atom over non-state signal").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelDiagnostics {
    /// Number of `state` register lines in the lifted (augmented) BTOR2. A
    /// suspiciously low / zero count is a strong signal that an FSM or other
    /// state was cut (see [`Self::blackboxed_modules`]) — properties over the
    /// cut registers are then SKIPPED rather than verified.
    pub state_register_count: usize,
    /// Modules instantiated without a body — either auto-black-boxed by Yosys
    /// (outputs cut to free inputs by `cutpoint -blackbox`) or left as a
    /// dangling undefined-module cell (e.g. an FSM wrapped in OpenTitan's
    /// `prim_sparse_fsm_flop`, whose register then vanishes from the lift).
    /// Both are sound, but a register hidden behind one is **not** modeled as
    /// state, so every property over it is SKIPPED. Surfacing it here is the
    /// "stop-silent-cut" half: the cut is no longer invisible. The fix is to
    /// provide the missing module source(s).
    pub blackboxed_modules: Vec<String>,
    /// Reset inputs that were pinned inactive at the model level so the body is
    /// verified only while not in reset (the `disable iff` guards were dropped
    /// from the formulas). Empty when reset-gating is off or no `disable iff`
    /// reset was recognized. Each entry is `"<signal>=<inactive_value>"`.
    pub gated_resets: Vec<String>,
    /// Flop-primitive modules that were cut (instantiated with no body) and for
    /// which verify-auto auto-injected a behavioral stub so the register
    /// survives the lift (H.C — e.g. OpenTitan's `prim_sparse_fsm_flop`). The
    /// stub is an exact behavioral model of the flop datapath; auto-injection is
    /// reported here so it is never silent.
    pub auto_provided_stubs: Vec<String>,
}

/// Severity of a [`VerificationNote`] — how it bears on the verdict's trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteLevel {
    /// Informational — the decision does not narrow soundness (e.g. the
    /// abstraction posture, the coverage summary).
    Info,
    /// The verdict's **scope** is narrowed by a deliberate restriction (e.g. a
    /// config input pinned to a constant → "for these values, not all configs").
    ScopeCaveat,
    /// A soundness-relevant cut (e.g. a black-boxed module → state not modeled →
    /// properties over it are SKIPPED, not verified).
    SoundnessCaveat,
}

/// A human-facing note explaining one abstraction / scoping decision the
/// verify-auto pipeline made, so a verdict's SCOPE and CAVEATS are explicit
/// rather than silent or buried in raw diagnostics. Rendered on every surface
/// (CLI text + JSON, API, UI). `kind` is an open kebab-case category so new
/// note types (per-property provenance, oracle status, …) slot in without a
/// schema change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationNote {
    /// Machine-stable category, kebab-case (e.g. `"config-concretization"`,
    /// `"reset-gating"`, `"blackbox-cut"`, `"abstraction-posture"`).
    pub kind: String,
    /// Severity for UX styling + honest framing.
    pub level: NoteLevel,
    /// One-line human summary.
    pub summary: String,
    /// Longer explanation: the WHY + the soundness / scope implication.
    pub detail: String,
    /// Structured operands, where relevant (e.g. `["cfg_detect_timer_i=7"]`,
    /// `["prim_sparse_fsm_flop"]`). Empty when the note has no operands.
    pub items: Vec<String>,
}

/// Result of an automated SVA verification run.
#[derive(Debug, Clone, Default)]
pub struct AutoVerifyReport {
    pub properties: Vec<PropertyVerdict>,
    /// Assertions that did not translate (name, reason), carried from extraction.
    pub unsupported: Vec<(String, String)>,
    /// Model-level diagnostics — state-register count + black-boxed (cut)
    /// modules. Lets a SKIPPED / vacuous outcome point at its root cause.
    pub diagnostics: ModelDiagnostics,
    /// H.J — human-facing provenance notes: every abstraction / scoping decision
    /// the run made (config concretizations, reset-gating, flop stubs, cut
    /// modules, the abstraction posture, the coverage summary), so the verdict's
    /// scope and caveats are explicit on every surface. Built by [`build_notes`].
    pub notes: Vec<VerificationNote>,
}

/// The skip reason for a property whose atoms reference non-state signals,
/// enriched with the model-level root cause when one is evident: a black-boxed
/// (cut) module whose registers became free inputs, or a model with no state
/// registers at all. Pure so it can be unit-tested without the toolchain.
fn unseedable_skip_reason(unseedable: &[String], diag: &ModelDiagnostics) -> String {
    let base = format!(
        "atom(s) over non-state signals (combinational/IO — not cube-bindable): {}",
        unseedable.join(", ")
    );
    if !diag.blackboxed_modules.is_empty() {
        format!(
            "{base}. Root cause: {} module(s) instantiated with no body ({}) — \
             registers they drive are not modeled as state. Provide the missing \
             module source(s) to model them.",
            diag.blackboxed_modules.len(),
            diag.blackboxed_modules.join(", ")
        )
    } else if diag.state_register_count == 0 {
        format!(
            "{base}. Root cause: the lifted model has no state registers — the design's \
             state may have been optimized away or cut."
        )
    } else {
        base
    }
}

/// A short human label for the must-edge inference posture (used in the
/// abstraction-posture note).
fn must_edge_inference_label(mode: MustEdgeInference) -> &'static str {
    match mode {
        MustEdgeInference::Off => "off (may-only over-approximation)",
        MustEdgeInference::SmtPerTarget => "SMT ∀∀ per-target",
        MustEdgeInference::SmtPerTargetStandard => "SMT ∀∃ per-target (canonical KMTS)",
        MustEdgeInference::SmtHyperMust => "SMT ∀∃ hyper-must (GKMTS, sound νμ)",
    }
}

/// H.J.b — token-aware substitution of concretized config inputs with their
/// constants in a formula string. Each **whole-identifier** occurrence of a
/// pinned signal name becomes its value (`cnt_q >= cfg_detect_timer_i` →
/// `cnt_q >= 7` with `{cfg_detect_timer_i: 7}`), so a relational-with-wide-input
/// atom lowers to a decidable state-vs-constant `Cmp`. Identifier chars match the
/// mu-parser (alnum, `_`, `$`, `.`); non-identifier text is preserved. Only the
/// signals actually pinned in the BTOR2 are substituted, so the formula and the
/// pinned model agree. Empty config ⇒ the string is returned unchanged. Pure +
/// unit-tested.
fn substitute_config_in_formula(formula: &str, config: &[(String, u64)]) -> String {
    if config.is_empty() {
        return formula.to_string();
    }
    let map: std::collections::HashMap<&str, u64> =
        config.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '.';
    let chars: Vec<char> = formula.chars().collect();
    let mut out = String::with_capacity(formula.len());
    let mut i = 0;
    while i < chars.len() {
        if is_ident(chars[i]) {
            let start = i;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            let tok: String = chars[start..i].iter().collect();
            match map.get(tok.as_str()) {
                Some(v) => out.push_str(&v.to_string()),
                None => out.push_str(&tok),
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// H.J — build the human-facing provenance notes from a completed run's
/// diagnostics + the abstraction posture + the coverage tally + any config
/// concretizations applied. Pure (no toolchain) so it is unit-testable; called
/// at the end of [`verify_auto`]. `applied_config_values` are the user-requested
/// `config_values` that actually pinned a signal this run (H.J.b); empty when no
/// concretization was requested. `posture` (A.3) makes the abstraction-posture
/// note honest about the may-relation the run actually used, rather than
/// hardcoding "may-over-approximation".
enum NotePosture {
    /// Exact-symbolic engine: the full bit-blasted state, no abstraction — a
    /// definite HOLDS/VIOLATED is sound and there is no `⊥`.
    Exact,
    /// Predicate-cube path. The cube lift always uses the sound `SmtAllPairs`
    /// may-relation (AR-S2 retired the sampling-may fallback + its A.4 ⊥-guard),
    /// so a definite HOLDS on a safety property is sound by over-approximation.
    Cube,
}

fn build_notes(
    report: &AutoVerifyReport,
    must_edge_inference: MustEdgeInference,
    applied_config_values: &[(String, u64)],
    counter_bounds: &[String],
    cutpoint_signals: &[String],
    posture: &NotePosture,
) -> Vec<VerificationNote> {
    let d = &report.diagnostics;
    let mut notes = Vec::new();

    // Coverage summary (Info) — the honest at-a-glance tally.
    let (mut holds, mut violated, mut unknown, mut skipped) = (0usize, 0usize, 0usize, 0usize);
    for p in &report.properties {
        match p.outcome {
            VerifyOutcome::Holds => holds += 1,
            VerifyOutcome::Violated { .. } => violated += 1,
            VerifyOutcome::Unknown { .. } => unknown += 1,
            VerifyOutcome::Skipped { .. } => skipped += 1,
        }
    }
    notes.push(VerificationNote {
        kind: "coverage-summary".into(),
        level: NoteLevel::Info,
        summary: format!(
            "{} assertion(s): {holds} definite (HOLDS), {violated} violated, {unknown} unknown (⊥), {skipped} skipped; {} untranslatable",
            report.properties.len(),
            report.unsupported.len(),
        ),
        detail: "A definite verdict transfers to the RTL (see abstraction-posture); ⊥ means the \
                 abstraction is too coarse to decide — an honest 'don't know', not a violation."
            .into(),
        items: Vec::new(),
    });

    // Config concretization (ScopeCaveat) — H.J.b; present only when the user
    // pinned config inputs to constants.
    if !applied_config_values.is_empty() {
        notes.push(VerificationNote {
            kind: "config-concretization".into(),
            level: NoteLevel::ScopeCaveat,
            summary: format!(
                "{} config input(s) pinned to constants — verdicts hold for THESE values, not all configurations.",
                applied_config_values.len()
            ),
            detail: "A wide config input (e.g. a timer threshold) was fixed to a representative \
                     constant so comparisons against it become decidable. The verdicts below are \
                     therefore scoped to this configuration; a different config value could change \
                     them. Re-run with other values to cover more of the configuration space."
                .into(),
            items: applied_config_values
                .iter()
                .map(|(sig, v)| format!("{sig}={v}"))
                .collect(),
        });
    }

    // Control-slice cut points (ScopeCaveat) — present only when the user cut nets
    // to free `$anyseq` inputs in the lift. This is an OVER-APPROXIMATION: it adds
    // transitions, so a definite HOLDS is sound but a definite VIOLATED may be
    // spurious unless it is guard-independent (the orphaned-FSM-state case).
    if !cutpoint_signals.is_empty() {
        // T3 (property-class honesty, 2026-07-23) — a cutpoint is a PLAIN over-approximation
        // (it adds transitions). That is sound-transferring for a definite HOLDS on a SAFETY /
        // universal property, but NOT for an ALTERNATING fixpoint (νμ recoverability `AG EF`),
        // which collapses under a plain over- or under-approximation (CLAUDE.md soundness table;
        // Bruns–Godefroid): a HOLDS on the cutpointed model does not automatically transfer to
        // the concrete RTL. Only append the νμ caveat when the report actually carries an
        // alternating property (a parse failure ⇒ conservatively omit — the base claim stays
        // accurate for the safety case).
        let has_alternating = report.properties.iter().any(|p| {
            crate::mu_calculus::parser::parse(&p.formula)
                .map(|f| f.alternation_depth() >= 2)
                .unwrap_or(false)
        });
        let numu_caveat = if has_alternating {
            " ⚠ But for an ALTERNATING recoverability property (`AG EF good`, νμ) a HOLDS under this \
              cut is sound for the abstract (cutpointed) model ONLY — it does NOT automatically transfer \
              to the concrete RTL, because alternating fixpoints collapse under a plain over-approximation. \
              Decide those on the un-cutpointed cone (raise the bit cap or pin the freed inputs) rather \
              than trusting the cut."
        } else {
            ""
        };
        notes.push(VerificationNote {
            kind: "control-slice".into(),
            level: NoteLevel::ScopeCaveat,
            summary: format!(
                "{} net(s) cut to free inputs (control-slice) — an over-approximation of the design.",
                cutpoint_signals.len()
            ),
            detail: format!(
                "Each listed net was replaced by a free `$anyseq` input (Yosys `cutpoint`), so its \
                 datapath fanin drops out of the cone (this is what lets `exact-symbolic` fit a wide \
                 control FSM). Because freeing a net only ADDS transitions, for a SAFETY / universal \
                 property a definite HOLDS transfers to the concrete RTL (safety + over-approx = \
                 sound).{numu_caveat} A definite VIOLATED is sound only when the counterexample is \
                 guard-independent — the canonical case being an ORPHANED FSM state (in-degree 0), \
                 which no freed guard can make reachable, so `AG EF <orphaned-state>` stays soundly \
                 VIOLATED. Treat any other VIOLATED under cut points as possibly spurious and re-check \
                 without the cut."
            ),
            items: cutpoint_signals.to_vec(),
        });
    }

    // Counter-bound seeding (Info) — H.H; present only when a bound `X <= K` was
    // seeded (user-supplied or config-inferred). A HOLDS it unlocks is PROVEN (the
    // bound is a sound partition the SMT must-edges verify), so no scope caveat
    // beyond the config-concretization one above.
    if !counter_bounds.is_empty() {
        notes.push(VerificationNote {
            kind: "counter-bound".into(),
            level: NoteLevel::Info,
            summary: format!(
                "{} counter-bound predicate(s) seeded to refine monotonicity properties.",
                counter_bounds.len()
            ),
            detail: "A monotonicity/increment property over a counter (`cnt_q >= $past(cnt_q)`) is \
                     ⊥ only because the abstraction admits the 32-bit wraparound. Seeding an upper \
                     bound `X <= K` PARTITIONS the state space (it does not assume the bound — the \
                     must-edges verify the reachable cubes stay bounded), excluding the wraparound \
                     state so the verdict can become definite. Sound regardless of K: a wrong bound \
                     only leaves the property at ⊥, never mis-verdicts. (config-inferred) bounds come \
                     from a counter's comparison against a pinned config threshold; (user-supplied) \
                     from --counter-bound / counter_bounds."
                .into(),
            items: counter_bounds.to_vec(),
        });
    }

    // Abstraction posture — A.3: honest about the may-relation actually used.
    match posture {
        NotePosture::Exact => notes.push(VerificationNote {
            kind: "abstraction-posture".into(),
            level: NoteLevel::Info,
            summary: "Exact-symbolic model checking — no abstraction, so a definite verdict is sound and there is no ⊥."
                .into(),
            detail: "Each property is decided over the full bit-blasted state space by ROBDD \
                     fixpoint; a definite HOLDS or VIOLATED transfers to the concrete RTL, and no \
                     property is left ⊥ (the predicate abstraction that would produce ⊥ is not used)."
                .into(),
            items: Vec::new(),
        }),
        NotePosture::Cube => {
            notes.push(VerificationNote {
                kind: "abstraction-posture".into(),
                level: NoteLevel::Info,
                summary: "KMTS may-over-approximation — a definite HOLDS on a safety property is sound."
                    .into(),
                detail: format!(
                    "The may-relation over-approximates the concrete transitions (SMT all-pairs), so \
                     a definite HOLDS transfers to the concrete RTL (safety + over-approximation = \
                     sound); a definite VIOLATED is a real reachable counterexample class. Must-edge \
                     inference: {}.",
                    must_edge_inference_label(must_edge_inference),
                ),
                items: Vec::new(),
            });
        }
    }

    // Reset gating (Info).
    if !d.gated_resets.is_empty() {
        notes.push(VerificationNote {
            kind: "reset-gating".into(),
            level: NoteLevel::Info,
            summary: format!("{} reset(s) pinned inactive.", d.gated_resets.len()),
            detail: "The `disable iff (!rst)` guards were dropped and the reset input pinned to \
                     its inactive value, so the body is verified only while out of reset (the \
                     standard formal reset discipline)."
                .into(),
            items: d.gated_resets.clone(),
        });
    }

    // Flop stubs (Info).
    if !d.auto_provided_stubs.is_empty() {
        notes.push(VerificationNote {
            kind: "flop-stub".into(),
            level: NoteLevel::Info,
            summary: format!(
                "{} flop primitive(s) behaviorally stubbed.",
                d.auto_provided_stubs.len()
            ),
            detail: "A cut flop primitive (no body in the source set) was auto-replaced with an \
                     EXACT behavioral model of its datapath, so the register it drives survives \
                     the lift (no soundness loss — the stub is functionally identical)."
                .into(),
            items: d.auto_provided_stubs.clone(),
        });
    }

    // Black-boxed / cut modules (SoundnessCaveat).
    if !d.blackboxed_modules.is_empty() {
        notes.push(VerificationNote {
            kind: "blackbox-cut".into(),
            level: NoteLevel::SoundnessCaveat,
            summary: format!(
                "{} module(s) cut (no body) — state behind them is not modeled.",
                d.blackboxed_modules.len()
            ),
            detail: "Registers driven by a cut module become free inputs, so properties over them \
                     are SKIPPED (never given a verdict), not verified. Provide the missing module \
                     source(s) to model that state."
                .into(),
            items: d.blackboxed_modules.clone(),
        });
    }

    notes
}

/// Options for [`verify_auto`] beyond the Yosys lift options.
#[derive(Debug, Clone)]
pub struct VerifyAutoOptions {
    /// Max CEGAR iterations per property (default 16).
    pub max_iterations: usize,
    /// Must-edge inference policy passed to each property's CEGAR run. Default
    /// `Off`; `SmtHyperMust` gives sound νμ verdicts (the recoverability case).
    pub must_edge_inference: MustEdgeInference,
    /// When `true` (default), `disable iff (reset)` guards are dropped from the
    /// formulas and the recognized reset inputs are pinned inactive at the
    /// model level (so the body is verified only while not in reset, matching
    /// SVA `disable iff` semantics). This removes the otherwise-unbindable
    /// reset-input atom that would force a SKIP. The dominant idiom in real
    /// SVA, so it is on by default; set `false` to keep the guard and leave
    /// the reset free.
    pub gate_reset: bool,
    /// When `true` (default), if the first lift cuts a known flop primitive
    /// (instantiated with no body — e.g. OpenTitan's `prim_sparse_fsm_flop`),
    /// verify-auto injects a behavioral stub for it and re-lifts, so the
    /// register survives and its state atoms become bindable (H.C). The stub is
    /// an exact behavioral model of the flop datapath; the auto-injection is
    /// reported in `diagnostics.auto_provided_stubs`. Set `false` to leave cut
    /// flops cut (reported in `blackboxed_modules`).
    pub auto_stub_flops: bool,
    /// H.J.b — user-supplied config concretization: pin a (typically wide, e.g. a
    /// timer threshold) input to a **constant** so comparisons against it become
    /// decidable. Each entry `signal → value` is pinned in the lifted BTOR2 (via
    /// `pin_inputs_to_constants`, like reset-gating) AND substituted into every
    /// formula atom (`cnt_q >= cfg_detect_timer_i` → `cnt_q >= 7`), turning a
    /// relational-with-wide-input into a decidable state-vs-constant comparison.
    /// **Scope-reduced:** the verdicts then hold FOR THESE values, not all
    /// configurations — surfaced as a `config-concretization` ScopeCaveat note.
    /// Default empty ⇒ no concretization, no behaviour change. Only signals that
    /// are actual inputs are pinned; the rest are ignored (and not reported).
    pub config_values: std::collections::HashMap<String, u64>,
    /// H.H — user-supplied counter upper bounds: `register → K` seeds a compound
    /// cube dimension `register <= K`, which PARTITIONS the abstract state into
    /// `{register<=K}` / `{register>K}` (it does NOT assume the bound — the SMT
    /// must-edges verify whether the reachable cubes stay bounded). This excludes
    /// the abstract 32-bit wraparound state that leaves counter-monotonicity
    /// properties (`cnt_q >= $past(cnt_q)`) at ⊥. **Sound regardless of K:** the
    /// partition never mis-verdicts (a wrong K just leaves the property at honest
    /// ⊥). Bounds are ALSO auto-derived from config concretization (a counter
    /// compared against a pinned config `cnt_q >= cfg_detect_timer_i`, cfg=7,
    /// yields `cnt_q <= 7`); a manual entry here overrides the config-inferred one.
    /// Requires `must_edge_inference` ON for the box modality to close. Default
    /// empty ⇒ only the config-inferred bounds fire (none without concretization).
    /// Surfaced as a `counter-bound` note. Only counter registers (a state cell
    /// appearing in a self-relational `$past` atom) are bounded; other registers
    /// are ignored.
    pub counter_bounds: std::collections::HashMap<String, u64>,
    /// W1/W2 — user-supplied **predicate hints** (the CLI `--predicate` / API
    /// `predicates` peer of the in-source `@mununu_predicate <expr>` annotation).
    /// Each entry is a predicate expression (`reg == value`, `reg == reg`,
    /// `reg >= K`) seeded into every property's cube via `seed_from_formula`'s
    /// `extra_atoms`, MERGED with the scanned `@mununu_predicate` annotations.
    /// Sound by monotonicity of predicate abstraction — a hint only refines the
    /// cube (a ⊥ can become definite; a definite verdict never flips; a hint that
    /// fails classification is dropped). Default empty ⇒ no behaviour change.
    pub predicate_hints: Vec<String>,
    /// R-F5.5d (2026-07-03) — run each property through the R-F5 **symbolic**
    /// predicate-cube engine (BDD relation + WP CEGAR loop; no per-cube-pair
    /// SMT) instead of the explicit `cegar_refine_loop`. Default `false`
    /// (explicit). The symbolic path is orders of magnitude faster at large
    /// `|P|` (the FSM-cone residual) but supports only cube-dimension predicates
    /// (equality + non-derived compounds) + the bare `[]`/`<>` fragment — a
    /// derived/combinational predicate or a guarded modality errors → the
    /// property is `Skipped` with the reason. Mirrors the CLI/API `--engine`.
    pub symbolic_engine: bool,
    /// D1.6 (2026-07-04) — run each property through **exact** full-state symbolic
    /// MC (`--engine exact-symbolic`): the μ-calculus is decided EXACTLY over the
    /// reset-gated btor2's bit-blasted state (no predicate abstraction), so the
    /// verdict is a **definite** 2-valued Holds/Violated — never ⊥. Decides
    /// `AF`-liveness (and any μ-calculus property) where the cube path returns
    /// Unknown; bounded by BDD size (a design over the bit cap ⇒ `Skipped`). Takes
    /// precedence over `symbolic_engine`. Mirrors the CLI/API `--engine`.
    pub exact_symbolic: bool,
    /// PORTFOLIO (2026-07-06) — when set, IGNORE `symbolic_engine`/`exact_symbolic`
    /// and instead run MULTIPLE engines (exact → symbolic-cube → explicit-CEGAR),
    /// taking the definite verdict from whichever engine produces one. Proven sound
    /// by `diff_corpus_cegar_vs_symbolic_engine_parity`: the three engines never
    /// contradict on a definite verdict, and each cube definite matches the exact MC.
    /// The mode is a BUDGET knob — [`PortfolioMode::Sequential`] early-exits (cheap),
    /// [`PortfolioMode::Parallel`] runs all three concurrently (fast, 3× compute).
    /// Default `None` ⇒ single-engine dispatch (unchanged). Mirrors the CLI/API
    /// `--engine portfolio-sequential` / `portfolio-parallel`.
    pub portfolio: Option<PortfolioMode>,
    /// On a *safety* property the cube portfolio leaves `⊥`, escalate to the
    /// multi-engine reachability portfolio (reduce the AG-invariant to a `bad`-monitor
    /// and decide it symbolically — exact ⊕ native ⊕ spacer ⊕ btormc ⊕ Pono). Only
    /// fires on `⊥` reducible AG-invariants, so there is no common-path cost and
    /// liveness / recoverability are untouched. Default `true`; the CLI `--no-rescue`
    /// / API `rescue_bottom_safety: false` opts out.
    pub rescue_bottom_safety: bool,
    /// On a *box-response liveness* property `AG(a → AF b)` the cube portfolio leaves
    /// `⊥`, escalate to the liveness-to-safety reduction + reachability portfolio
    /// (reduce to a `bad`-monitor lasso and decide it symbolically). Only fires on `⊥`
    /// reducible box-AF responses — the diamond-EF recoverability liveness (`AG(a → EF
    /// b)`, νμ) is NOT l2s-able and stays on the exact / cube νμ path, so there is no
    /// common-path cost and no mis-route. Default `true`; opts out with the same
    /// `--no-rescue` / API `rescue_bottom_liveness: false`.
    pub rescue_bottom_liveness: bool,
    /// On a νμ **recoverability** property `AG EF good` the cube portfolio leaves `⊥`,
    /// escalate to the cube + `smt-hyper-must` path (definite νμ verdicts transfer).
    /// Only fires on the `AG EF (REG == VALUE)` shape. Default `true`; `--no-rescue` /
    /// API `rescue_bottom_recoverability: false` opts out.
    pub rescue_bottom_recoverability: bool,
}

/// PORTFOLIO scheduling mode — the budget knob for the multi-engine default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortfolioMode {
    /// Run the engines in PRECISION order (exact → symbolic-cube → explicit-CEGAR),
    /// stopping the moment the report has no ⊥ property. Minimises compute — often
    /// only the exact engine runs — at the cost of worst-case latency (the sum of
    /// each engine tried until one decides). The budget-frugal choice.
    Sequential,
    /// Run ALL engines CONCURRENTLY (scoped threads) and merge; each property takes
    /// the definite verdict from whichever engine produced one. Maximises compute
    /// (every engine always runs) for minimum latency (the fastest engine to decide
    /// each property). The budget-rich, low-latency choice.
    Parallel,
}

/// Map an optional `--engine` / `"engine"` selector string to the
/// `(symbolic_engine, exact_symbolic, portfolio)` option triple, applying the DEFAULT when the
/// selector is unspecified. **The default (2026-07-06) is `portfolio-sequential`** — the
/// exact-first, cube-fallback portfolio, the most precise sound choice and no slower than the
/// former `explicit` default on designs `explicit` already decided (exact runs first). An
/// explicit `"explicit"` selects the single predicate-abstraction CEGAR engine. The CLI reaches
/// the same defaults through clap's `default_value_t = EngineArg::PortfolioSequential`; this
/// helper is the API/string entry point (and the single place the default is defined for it).
pub fn engine_selection(engine: Option<&str>) -> (bool, bool, Option<PortfolioMode>) {
    match engine {
        Some(e) if e.eq_ignore_ascii_case("symbolic") => (true, false, None),
        Some(e) if e.eq_ignore_ascii_case("exact-symbolic") => (false, true, None),
        Some(e) if e.eq_ignore_ascii_case("portfolio-parallel") => {
            (false, false, Some(PortfolioMode::Parallel))
        }
        Some(e) if e.eq_ignore_ascii_case("portfolio-sequential") => {
            (false, false, Some(PortfolioMode::Sequential))
        }
        Some(e) if e.eq_ignore_ascii_case("explicit") => (false, false, None),
        // Unspecified (or unrecognised) → the default portfolio-sequential.
        _ => (false, false, Some(PortfolioMode::Sequential)),
    }
}

impl Default for VerifyAutoOptions {
    fn default() -> Self {
        Self {
            max_iterations: 16,
            must_edge_inference: MustEdgeInference::Off,
            gate_reset: true,
            auto_stub_flops: true,
            config_values: std::collections::HashMap::new(),
            counter_bounds: std::collections::HashMap::new(),
            predicate_hints: Vec::new(),
            symbolic_engine: false,
            exact_symbolic: false,
            portfolio: None,
            rescue_bottom_safety: true,
            rescue_bottom_liveness: true,
            rescue_bottom_recoverability: true,
        }
    }
}

/// Auto-seeded predicates for one formula: simple `reg == value` specs, compound
/// `(name, expr)` pairs (relational / `!=` / boolean combinations), and the atom
/// strings that could NOT be seeded (reference a non-state, non-input signal).
/// H.E — how a combinational signal's value is determined, deciding how it binds:
/// - `InputDependent` — its cone reaches a primary input (`trigger_active =
///   !trigger_i`). Routed through the FREE-INPUT path (a free cube dimension,
///   source-pin / target-free), so the may/must edges respect it. Treating it as
///   a state-cube label is unsound (the sysrst sva_6 / sva_9 spurious VIOLATED).
/// - `StateOnly` — a pure function of state (`event_detected_o = f(state_q)`).
///   Sound as a derived per-cube 3-valued label (Approach B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CombKind {
    InputDependent,
    StateOnly,
}

#[derive(Debug, Clone, Default)]
struct Seeded {
    specs: Vec<PredicateSpec>,
    compounds: Vec<(String, PredicateExpr)>,
    unseedable: Vec<String>,
    /// H.B (free-input atoms) — the subset of `specs` registers that are
    /// primary **inputs** (not state cells). An input predicate is a *free*
    /// cube dimension: the environment picks its value each cycle, so it is
    /// NOT pinned at init (the env is free at cycle 0) and the verdict is read
    /// across **all** of its initial polarities. See
    /// `docs/design/free-input-atoms.md`. Empty ⇒ the pre-H.B state-only shape.
    input_registers: HashSet<String>,
    /// H.E (combinational outputs) — **derived** combinational predicates: a
    /// simple atom whose register is a combinational node (e.g. csrng's
    /// `main_sm_err_o`), NOT a cube dimension. The lift labels each per cube via
    /// SMT (Approach B). Threaded to `cegar_refine_loop` through the synthesised
    /// sidecar's `combinational_predicates`. Empty ⇒ the pre-H.E shape.
    derived: Vec<PredicateSpec>,
    /// H.F — **relational** derived predicates: a `PredicateExpr` (`cnt_q >=
    /// cfg_detect_timer_i`, `trigger_i != trigger_active`) whose operands include
    /// an input or combinational signal, so it is NOT a sound cube dimension. Like
    /// [`Seeded::derived`] it is labelled per cube via SMT, but it carries a full
    /// expr (not `register == value`). Threaded through the synthesised sidecar's
    /// `compound_predicates` with `derived = true`. Empty ⇒ the pre-H.F shape.
    derived_relational: Vec<(String, PredicateExpr)>,
}

/// The minimal H.1 (+ H.B) — derive cube predicates from a formula's
/// `Node::Predicate` atoms. Each predicate is named exactly the atom string (so
/// the evaluator's name-match binds the atom to its cube bit).
///
/// `is_state(name)` returns true when `name` resolves to a lifted state cell —
/// either directly (the symbol is on a `state` line) or via the BTOR2
/// alias-resolution BFS (H.A: a `uext`/`Output` alias whose register Yosys'
/// `flatten` left unnamed on the `state` line). The lift's
/// `resolve_predicate_registers` canonicalises the same alias, so a name the
/// seeder accepts here also binds downstream.
///
/// `is_input(name)` (H.B) returns true when `name` is a primary **input** of
/// the lifted BTOR2. A **simple** `input == value` / bare-input atom is admitted
/// as a *free* cube dimension (its register is recorded in
/// [`Seeded::input_registers`]); the SMT may/must seam realises the source-pin /
/// target-free shape (`build_register_nid_map_with_inputs`). State takes
/// precedence — `is_state` is consulted first — so a name that is both never
/// double-classifies.
///
/// **Scope.** Inputs are admitted only inside a *simple* atom (the compound SMT
/// branch reads `state_curr`/`state_next` BVs, not `view.inputs`, so a compound
/// referencing an input would mis-encode). A compound (`!=`, relational, boolean)
/// still requires **all-state** registers; an input inside a compound is
/// unseedable. A non-state non-input register (a pure combinational function of
/// state — the strict resolver's reject case) is unseedable too — never given a
/// misleading verdict.
fn seed_from_formula(
    formula: &Formula,
    is_state: impl Fn(&str) -> bool,
    is_input: impl Fn(&str) -> bool,
    combinational_kind: impl Fn(&str) -> Option<CombKind>,
    cone_inputs_of: impl Fn(&str) -> Vec<String>,
    extra_atoms: &[String],
) -> Seeded {
    // Slice 3 (the unwrap lever) — cap how many raw cone inputs a single
    // combinational-of-input atom may add as free cube dimensions. Each added
    // input doubles the cube, so a combinational reaching many inputs is left as
    // a plain ⊥-label (unrefined) rather than blowing up the abstraction. One
    // input (`trigger_active = !trigger_i`) — the common case — always fires.
    const MAX_CONE_INPUTS: usize = 2;
    let mut out = Seeded::default();
    let mut seen: HashSet<&str> = HashSet::new();
    // Route one simple `register == value` (or bare-boolean ≡ `== 1`) atom by
    // the provenance of its register:
    // - state cell → cube dimension (H.A);
    // - free input → cube dimension, free at init (H.B);
    // - input-dependent combinational → cube dimension via the free-input path
    //   (H.E: source-pin / target-free, recorded in `input_registers`);
    // - state-only combinational → derived per-cube label (H.E Approach B);
    // - otherwise → unseedable (never a misleading verdict).
    let seed_simple = |out: &mut Seeded, atom: &str, register: &str, value: u64| {
        if is_state(register) {
            out.specs.push(PredicateSpec {
                name: atom.to_string(),
                register: register.to_string(),
                value,
            });
        } else if is_input(register) {
            out.input_registers.insert(register.to_string());
            out.specs.push(PredicateSpec {
                name: atom.to_string(),
                register: register.to_string(),
                value,
            });
        } else {
            match combinational_kind(register) {
                Some(CombKind::StateOnly) => {
                    // H.U.2 — a combinational function of STATE only is a
                    // determined function of state, so the uniform predicate-image
                    // handles it as a CUBE DIMENSION (resolved to the combinational
                    // node via the H.U.1d nid-map; its value over `(s,i)` /
                    // `(s',i')` from the signal cache + primed cache). This
                    // subsumes the retired H.E derived-label pass. The init-cube
                    // bit for this dimension is computed by observing the signal at
                    // the reset state (see the init-cube section below).
                    out.specs.push(PredicateSpec {
                        name: atom.to_string(),
                        register: register.to_string(),
                        value,
                    });
                }
                // H.E.r2 (combinational-input-atoms.md §6.1) — a combinational
                // function of a FREE INPUT (`trigger_active = !trigger_i`,
                // `main_sm_err_o = f(state, enable_i)`). It is NOT a sound cube
                // dimension (target-free fabricates must-edges; §5.1 / H.U.0), so
                // route it to a DERIVED 3-valued label: per cube the labeller
                // (`smt_combinational_label`) decides KleeneT/F where the cube pins
                // the signal, KleeneBot where the free input swings it (the generic
                // case for combinational-of-input). SOUND — a pure observation, no
                // edge change (§6.1 Prop 3) — and a `⊥` atom can never produce a
                // spurious VIOLATED (in Kleene `⊥ ∨ []C` is `T` or `⊥`, never `F`);
                // for the `AG(A→AX C)` safety shape with the atom in the antecedent
                // it yields a DEFINITE HOLDS via `⊥ ∨ []C = T` when the consequent
                // carries the property.
                Some(CombKind::InputDependent) => {
                    out.derived.push(PredicateSpec {
                        name: atom.to_string(),
                        register: register.to_string(),
                        value,
                    });
                    // Slice 3 (the unwrap lever, combinational-input-atoms.md
                    // §6.1 boundary → resolved): ALSO seed the raw free inputs in
                    // the combinational's cone as free H.B cube dimensions. This
                    // refines the may-relation so a consequent box over a
                    // transition GOVERNED by the input (`AG((state ∧ trigger) →
                    // AX state′)`) becomes definite — the sysrst sva_4..11 class
                    // that a ⊥-label alone leaves at ⊥. The derived ⊥-label above
                    // then turns DEFINITE at every cube that pins the input (the
                    // labeller proves the combinational constant there). Sound:
                    // a free-input dimension is the H.B source-pin / target-free
                    // shape (no fabricated must-edge — the H.E unsoundness came
                    // from a state-cube dimension, not a raw input); the label is
                    // a pure observation. Gated on a small cone-input count.
                    let cone = cone_inputs_of(register);
                    if !cone.is_empty() && cone.len() <= MAX_CONE_INPUTS {
                        for inp in cone {
                            if out.input_registers.insert(inp.clone()) {
                                out.specs.push(PredicateSpec {
                                    name: inp.clone(),
                                    register: inp,
                                    value: 1,
                                });
                            }
                        }
                    }
                }
                // Unresolvable combinational (no nid in the lifted design): cannot
                // label what we cannot resolve — honest SKIP.
                None => out.unseedable.push(atom.to_string()),
            }
        }
    };
    // Formula atoms first, then annotation `@mununu_predicate` atoms — both run the
    // IDENTICAL provenance classification (state → cube dimension, state-only
    // combinational → dimension, input-dependent combinational → derived label,
    // unresolvable → skip). Annotation predicates seed the cube even when they do NOT
    // appear in the property formula: they are auxiliary invariant HINTS. Soundness is
    // unaffected — predicate abstraction is monotone, so an extra dimension only
    // refines the may/must partition (a ⊥ can become definite; a definite verdict can
    // never flip). A hint that fails classification lands in `unseedable` and is
    // dropped, never a spurious verdict.
    let atoms = formula
        .nodes()
        .iter()
        .filter_map(|node| match node {
            Node::Predicate(atom) => Some(atom.as_str()),
            _ => None,
        })
        .chain(extra_atoms.iter().map(String::as_str));
    for atom in atoms {
        if !seen.insert(atom) {
            continue;
        }
        match parse_predicate_expr(atom) {
            Ok(expr) => match &expr {
                // Simple `reg == value` → routed by `seed_simple`.
                PredicateExpr::Cmp {
                    register,
                    op: CmpOp::Eq,
                    value,
                } => seed_simple(&mut out, atom, register, *value),
                // Relational / `!=` / boolean combination.
                _ => {
                    let regs = expr.registers();
                    let resolvable = regs
                        .iter()
                        .all(|r| is_state(r) || is_input(r) || combinational_kind(r).is_some());
                    if expr.has_addend() {
                        // H.G — an arithmetic relational (`cnt_q == cnt_q__past + 1`,
                        // sva_15). Its `CmpRegAddend` leaf is SMT-only (a `width==0`
                        // production leaf must NOT be `eval`'d, and the all-state
                        // compound path evals dimensions at the init cube). Route it
                        // to the DERIVED relational label path (`build_constraint`
                        // only, BV `bvadd` at the real register width). Sound — a
                        // pure per-cube observation, like any derived relational.
                        if resolvable {
                            out.derived_relational.push((atom.to_string(), expr));
                        } else {
                            out.unseedable.push(atom.to_string());
                        }
                    } else if regs.iter().all(|r| is_state(r)) {
                        // All-state → a cube DIMENSION (B.1 compound). The SMT
                        // dimension path reads state BVs.
                        out.compounds.push((atom.to_string(), expr));
                    } else if resolvable {
                        // H.F — a relational with an input / combinational operand
                        // (`cnt_q >= cfg_detect_timer_i`, `trigger_i !=
                        // trigger_active`). Its value depends on the demonic input,
                        // so it is NOT a sound cube dimension (the same reason a
                        // combinational-of-input atom isn't). Route it to a DERIVED
                        // per-cube 3-valued label: the SMT labeller decides
                        // KleeneT/F where the design's logic pins it (e.g.
                        // `trigger_i != ~trigger_i` ≡ true → HOLDS), KleeneBot where
                        // a free operand swings it. Sound (a pure observation; a
                        // KleeneBot atom is ⊥, never a spurious verdict).
                        out.derived_relational.push((atom.to_string(), expr));
                    } else {
                        // An operand resolves to nothing in the lifted design —
                        // cannot label it. Honest SKIP.
                        out.unseedable.push(atom.to_string());
                    }
                }
            },
            // A bare identifier atom (`parse_predicate_expr` needs an operator):
            // a 1-bit boolean signal `sig` ≡ `sig == 1`.
            Err(_) => seed_simple(&mut out, atom, atom, 1),
        }
    }
    out
}

/// Every register name referenced by a property's seeded predicates (specs +
/// compound + derived-relational operands). Used to detect counters and to decide
/// whether a counter's `$past` shadow is present (so it, too, gets bounded).
fn referenced_registers(seeded: &Seeded) -> std::collections::BTreeSet<String> {
    let mut all: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for s in &seeded.specs {
        all.insert(s.register.clone());
    }
    for (_, e) in &seeded.compounds {
        for r in e.registers() {
            all.insert(r);
        }
    }
    for (_, e) in &seeded.derived_relational {
        for r in e.registers() {
            all.insert(r);
        }
    }
    all
}

/// H.H — the counter registers referenced by a property's seeded predicates: a
/// state register `X` whose `$past` shadow `X__past` is ALSO referenced (a
/// self-relational monotonicity / increment atom — `cnt_q >= cnt_q__past` (sva_13)
/// or `cnt_q == cnt_q__past + 1` (sva_15)). These are exactly the registers whose
/// abstract 32-bit wraparound leaves the property at ⊥ — the ones a bound predicate
/// refines. Restricting the bound-seed to these avoids adding useless cube
/// dimensions to unrelated properties.
fn counter_registers_in(seeded: &Seeded) -> std::collections::BTreeSet<String> {
    let all = referenced_registers(seeded);
    let mut counters = std::collections::BTreeSet::new();
    for name in &all {
        if let Some(base) = name.strip_suffix("__past")
            && all.contains(base)
        {
            counters.insert(base.to_string());
        }
    }
    counters
}

/// H.H — auto-derive counter upper bounds from config concretization. Scans EVERY
/// H.5-GR1 — the outcome of scanning the SV source(s) for `@mununu_*` property
/// annotations (the mununu-exclusive properties an author adds beyond the design's
/// own SVA — recoverability, realizability, assume-guarantee liveness). The
/// `guarantees` are appended to the translated property set and verified uniformly;
/// `assumes` and `skipped` are surfaced as a provenance note.
#[derive(Debug, Default, Clone)]
struct AnnotationScan {
    /// `@mununu_guarantee <mu-calculus>` properties that parsed, as
    /// `TranslatedAssertion`s ready to merge into `extraction.translated`.
    guarantees: Vec<crate::adapter::slang::translate::TranslatedAssertion>,
    /// `@mununu_assume <body>` bodies (verbatim) — recorded for provenance. A
    /// `<signal> = <value>` assume can be applied via the existing config
    /// concretization; a temporal `GF ...` fairness assume is documented future
    /// work (mununu has no native fairness-constrained model checking).
    assumes: Vec<String>,
    /// `@mununu_guarantee` bodies that did NOT parse — surfaced, never silently
    /// dropped.
    skipped: Vec<String>,
    /// `@mununu_predicate <expr>` auxiliary abstraction-predicate hints (verbatim
    /// expr strings, validated as parseable). Seeded into every property's cube via
    /// `seed_from_formula`'s `extra_atoms`. Sound by monotonicity — a hint only
    /// refines the cube (a ⊥ can become definite; a definite verdict never flips).
    predicates: Vec<String>,
}

/// H.5-GR1 — scan the SV source(s) for `@mununu_guarantee` / `@mununu_assume`
/// property annotations and turn each guarantee into a verifiable property. Reuses
/// the existing [`crate::mununu_annotations::extract_from_sv_source`] scanner and
/// the [`crate::mu_calculus::parser`] — no new grammar. A guarantee body is a
/// mu-calculus formula (the same string form the SVA translator emits and the
/// per-property loop parses); box-`F` liveness (`μY.(φ ∨ []Y)`) is expressible
/// directly, unlike the LTL translator's existential-`F`. Bodies that do not parse
/// are recorded in `skipped`, never silently dropped.
fn scan_annotation_properties(sources: &[(String, String)]) -> AnnotationScan {
    use crate::adapter::slang::translate::TranslatedAssertion;
    use crate::mununu_annotations::{MununuTag, extract_from_sv_source};

    let mut scan = AnnotationScan::default();
    let mut idx = 0usize;
    for (_name, content) in sources {
        for ann in extract_from_sv_source(content) {
            match ann.tag {
                MununuTag::Guarantee => {
                    let body = ann.value.trim();
                    if body.is_empty() {
                        scan.skipped
                            .push("@mununu_guarantee with an empty body".to_string());
                        continue;
                    }
                    match crate::mu_calculus::parser::parse(body) {
                        Ok(_) => {
                            scan.guarantees.push(TranslatedAssertion {
                                name: format!("ann_guarantee_{idx}"),
                                kind: SvaKind::Assert,
                                formula: body.to_string(),
                                recoverability_companion: None,
                            });
                            idx += 1;
                        }
                        Err(e) => scan.skipped.push(format!(
                            "@mununu_guarantee `{body}` did not parse as a mu-calculus formula: {e}"
                        )),
                    }
                }
                MununuTag::Assume => scan.assumes.push(ann.value.trim().to_string()),
                MununuTag::Predicate => {
                    let body = ann.value.trim();
                    // A bare identifier (`sig` ≡ `sig == 1`) is admitted like a bare
                    // formula atom; anything else must parse as a predicate expr.
                    let ok = !body.is_empty()
                        && (!body.contains(['=', '<', '>', '!'])
                            || parse_predicate_expr(body).is_ok());
                    if ok {
                        scan.predicates.push(body.to_string());
                    } else {
                        scan.skipped.push(format!(
                            "@mununu_predicate `{body}` did not parse as a predicate expression"
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    scan
}

/// H.5-GR1 — the provenance note for an [`AnnotationScan`]: how many
/// `@mununu_guarantee` properties were merged, which `@mununu_assume` bodies were
/// seen, and any guarantee bodies that failed to parse (surfaced, never dropped).
/// `None` when the source carried no `@mununu` property annotations.
fn annotation_note(scan: &AnnotationScan) -> Option<VerificationNote> {
    if scan.guarantees.is_empty() && scan.assumes.is_empty() && scan.skipped.is_empty() {
        return None;
    }
    let mut items = Vec::new();
    for g in &scan.guarantees {
        items.push(format!("guarantee {}: {}", g.name, g.formula));
    }
    for a in &scan.assumes {
        items.push(format!("assume: {a}"));
    }
    for s in &scan.skipped {
        items.push(format!("skipped: {s}"));
    }
    Some(VerificationNote {
        kind: "annotation-properties".into(),
        level: if scan.skipped.is_empty() {
            NoteLevel::Info
        } else {
            NoteLevel::ScopeCaveat
        },
        summary: format!(
            "{} `@mununu_guarantee` propert{} verified alongside the design's SVA{}.",
            scan.guarantees.len(),
            if scan.guarantees.len() == 1 { "y" } else { "ies" },
            if scan.skipped.is_empty() {
                String::new()
            } else {
                format!("; {} unparsable, skipped", scan.skipped.len())
            }
        ),
        detail: "`@mununu_guarantee <mu-calculus>` / `@mununu_assume <body>` annotations carry the \
                 mununu-exclusive properties an author adds beyond the design's own SVA (e.g. \
                 assume-guarantee liveness the SVA fragment cannot express). Guarantee bodies are \
                 mu-calculus formulas parsed by the same parser as the translated SVA and verified \
                 through the same pipeline. `@mununu_assume` of the form `<signal> = <value>` can be \
                 applied via config concretization; a temporal (`GF …`) fairness assume is not yet \
                 checkable (no native fairness-constrained model checking) and is recorded here."
            .into(),
        items,
    })
}

/// translated property's ORIGINAL formula for a relational atom comparing a register
/// `R` against a pinned config register `C` (`cnt_q >= cfg_detect_timer_i`); the
/// pinned value `V = config[C]` becomes a candidate bound `R <= V` (a counter counts
/// up to the threshold it is compared against). GLOBAL across properties because the
/// property that NEEDS the bound (sva_13, `cnt_q >= cnt_q__past`) does not itself
/// reference the config — the comparison lives in a sibling property (sva_5,
/// `cnt_q >= cfg_detect_timer_i`). The widest value wins on collision (the loosest
/// sound partition). Empty when no config was concretized.
fn config_inferred_counter_bounds(
    translated: &[crate::adapter::slang::translate::TranslatedAssertion],
    applied_config_values: &[(String, u64)],
) -> std::collections::HashMap<String, u64> {
    let mut bounds: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    if applied_config_values.is_empty() {
        return bounds;
    }
    let cfg: std::collections::HashMap<&str, u64> = applied_config_values
        .iter()
        .map(|(n, v)| (n.as_str(), *v))
        .collect();
    for t in translated {
        let Ok(formula) = crate::mu_calculus::parser::parse(&t.formula) else {
            continue;
        };
        for node in formula.nodes() {
            let Node::Predicate(atom) = node else {
                continue;
            };
            let Ok(PredicateExpr::CmpReg { lhs, rhs, .. }) = parse_predicate_expr(atom) else {
                continue;
            };
            // One operand is a pinned config register; the other is the register
            // it bounds. (Either order — the comparison direction does not matter
            // for a sound partition.)
            let (reg, v) = if let Some(&v) = cfg.get(rhs.as_str()) {
                (lhs, v)
            } else if let Some(&v) = cfg.get(lhs.as_str()) {
                (rhs, v)
            } else {
                continue;
            };
            let e = bounds.entry(reg).or_insert(v);
            if v > *e {
                *e = v;
            }
        }
    }
    bounds
}

/// H.H — seed a bound compound `X <= K` for each counter register `X`, manual bound
/// (`opts.counter_bounds`) winning over the config-inferred one. Returns the applied
/// `(register, bound, provenance)` triples for the `counter-bound` note. **Sound
/// regardless of K:** the bound is a cube-dimension PARTITION (`{X<=K}` / `{X>K}`),
/// not an assumption — the SMT must-edges verify whether the reachable cubes stay
/// bounded, so a wrong K only leaves the property at honest ⊥, never mis-verdicts.
/// Dedups against compounds already present (an identical `X <= K` atom from the
/// formula itself).
fn seed_counter_bounds(
    seeded: &mut Seeded,
    counters: &std::collections::BTreeSet<String>,
    manual: &std::collections::HashMap<String, u64>,
    config_inferred: &std::collections::HashMap<String, u64>,
) -> Vec<(String, u64, &'static str)> {
    let referenced = referenced_registers(seeded);
    let mut applied = Vec::new();
    for reg in counters {
        let (k, provenance) = if let Some(&k) = manual.get(reg) {
            (k, "user-supplied")
        } else if let Some(&k) = config_inferred.get(reg) {
            (k, "config-inferred")
        } else {
            continue;
        };
        // Bound BOTH the counter and its `$past` shadow (when present): the
        // monotonicity/increment relational (`cnt_q >= cnt_q__past`) excludes the
        // abstract wraparound state only when BOTH operands are bounded — bounding
        // `cnt_q` alone leaves the state `(cnt_q=0, cnt_q__past=2^32-1)` reachable,
        // so the relation stays ⊥.
        let shadow = format!("{reg}__past");
        let mut targets = vec![reg.clone()];
        if referenced.contains(&shadow) {
            targets.push(shadow);
        }
        let mut added_any = false;
        for target in targets {
            let name = format!("{target} <= {k}");
            if seeded.compounds.iter().any(|(n, _)| n == &name) {
                continue;
            }
            seeded.compounds.push((
                name,
                PredicateExpr::Cmp {
                    register: target,
                    op: CmpOp::Le,
                    value: k,
                },
            ));
            added_any = true;
        }
        // Only report a bound that actually seeded a new compound this call — so
        // the note does not double-count and re-seeding is a true no-op.
        if added_any {
            applied.push((reg.clone(), k, provenance));
        }
    }
    applied
}

/// Build the synthesised `SvAnnotation` sidecar JSON `cegar_refine_loop` reads:
/// the compound predicates (`compound_predicates: [{name, expr}]`) PLUS
/// `signals[].config_values` pinning each referenced register to its design init
/// value. The config_values pin is load-bearing: without it the cube lift
/// defaults its initial cube to `cube_0` (all-predicates-false) — generally NOT
/// the reset state — and a property can falsely report VIOLATED at that
/// fictitious init. Returns `None` when there is nothing to emit.
fn synth_sidecar_json(
    compounds: &[(String, PredicateExpr)],
    derived: &[PredicateSpec],
    derived_relational: &[(String, PredicateExpr)],
    referenced: &std::collections::BTreeSet<String>,
    init_values: &std::collections::HashMap<String, u64>,
) -> Option<String> {
    let mut compound_decls: Vec<serde_json::Value> = compounds
        .iter()
        // `expr` == `name` == the atom string, which `sidecar_compound_predicates`
        // re-parses via `parse_predicate_expr` (REL handles `reg == reg`).
        .map(|(name, _)| serde_json::json!({ "name": name, "expr": name }))
        .collect();
    // H.F — relational derived predicates ride the SAME `compound_predicates`
    // field tagged `derived: true`, so `cegar_refine_loop` routes them to the
    // per-cube label path (not the cube index). `expr` == `name` as above.
    compound_decls.extend(
        derived_relational
            .iter()
            .map(|(name, _)| serde_json::json!({ "name": name, "expr": name, "derived": true })),
    );
    // H.E — derived combinational predicates: `cegar_refine_loop` reads these
    // into `lift_opts.derived_predicates`; the lift labels each per cube via the
    // SMT `combinational_labels` pass (NOT a cube dimension).
    let combinational_decls: Vec<serde_json::Value> = derived
        .iter()
        .map(|d| serde_json::json!({ "name": d.name, "signal": d.register, "value": d.value }))
        .collect();
    let signals: Vec<serde_json::Value> = referenced
        .iter()
        .filter_map(|r| {
            init_values
                .get(r)
                .map(|v| serde_json::json!({ "name": r, "config_values": [v] }))
        })
        .collect();
    if compound_decls.is_empty() && combinational_decls.is_empty() && signals.is_empty() {
        return None;
    }
    serde_json::to_string(&serde_json::json!({
        "module": "cegar",
        "source": "verify_auto.btor2",
        "compound_predicates": compound_decls,
        "combinational_predicates": combinational_decls,
        "signals": signals,
    }))
    .ok()
}

/// Init value of every state cell, keyed by symbol — from the BTOR2 `init`
/// lines, defaulting to 0 (the `setundef -zero` power-up). Used to pin the cube
/// lift's initial cube to the design's reset state.
fn state_cell_init_values(
    file: &crate::adapter::btor2::ast::Btor2File,
) -> std::collections::HashMap<String, u64> {
    use crate::adapter::btor2::ast::Node;
    let symbols = crate::adapter::btor2::parser::collect_symbols(file);
    let mut init_of_state: std::collections::HashMap<crate::adapter::btor2::ast::Nid, u64> =
        std::collections::HashMap::new();
    for line in &file.lines {
        if let Node::Init { state, value, .. } = &line.node
            && let Some(cl) = file.lookup(value.nid())
            && let Node::Const { sort, value: cv } = &cl.node
        {
            init_of_state.insert(*state, const_to_u64(file, *sort, cv));
        }
    }
    let mut out = std::collections::HashMap::new();
    for line in &file.lines {
        if matches!(line.node, Node::State { .. })
            && let Some(name) = symbols.get(&line.nid)
        {
            out.insert(
                name.clone(),
                init_of_state.get(&line.nid).copied().unwrap_or(0),
            );
        }
    }
    out
}

/// A BTOR2 constant value → `u64` (all-ones needs the sort width).
fn const_to_u64(
    file: &crate::adapter::btor2::ast::Btor2File,
    sort_nid: crate::adapter::btor2::ast::Nid,
    cv: &crate::adapter::btor2::ast::ConstValue,
) -> u64 {
    use crate::adapter::btor2::ast::ConstValue::*;
    match cv {
        Zero => 0,
        One => 1,
        Ones => {
            let w = crate::adapter::btor2::parser::bv_width(file, sort_nid).unwrap_or(0);
            if w == 0 || w >= 64 {
                u64::MAX
            } else {
                (1u64 << w) - 1
            }
        }
        Dec(d) => *d as u64,
        Bin(b) => u64::from_str_radix(b, 2).unwrap_or(0),
        Hex(h) => u64::from_str_radix(h, 16).unwrap_or(0),
    }
}

/// H.B — the set of initial cube indices: the pinned-state `base` cube ⊗ every
/// combination of the free-input bit positions. The environment is free at cycle
/// 0, so each free-input dimension ranges over both polarities. An empty
/// `free_bits` returns `[base]` (the pre-H.B single reset cube), so state-only
/// properties read exactly one cube. Bit `free_bits[k]` is toggled by bit `k` of
/// the combination index.
fn free_input_init_cubes(base: usize, free_bits: &[u32]) -> Vec<usize> {
    (0..(1usize << free_bits.len()))
        .map(|combo| {
            let mut c = base;
            for (k, &b) in free_bits.iter().enumerate() {
                if (combo >> k) & 1 == 1 {
                    c |= 1 << b;
                }
            }
            c
        })
        .collect()
}

/// R-F5.5d — project the R-F5 symbolic engine's per-cube verdicts into the
/// explicit path's `TritSet` shape (`2^|final P|` cells, same cube indexing), so
/// the reset-cube read in [`verify_auto`] is identical across engines. Feasible
/// cubes carry their `{T, F, ⊥}` verdict; infeasible cubes are `F` (never a
/// reachable reset cube, so they only pad the advisory `false_cells` count).
fn symbolic_final_verdict(
    result: &crate::adapter::btor2::symbolic_engine::SymbolicCegarResult,
) -> crate::mu_calculus::trit::TritSet {
    use bitvec::prelude::*;
    let k = result.final_verdicts.num_predicates;
    let n = 1usize << k;
    let mut must = bitvec![usize, Lsb0; 0; n];
    let mut may = bitvec![usize, Lsb0; 0; n];
    // `symbolic_cube_verdicts` tallies ONLY feasible cubes. Every other cube
    // stays absent from `cube_verdicts`. Track which cubes were tallied so the
    // untallied (infeasible) ones can be projected to ⊥ below — NOT left at
    // `must=0, may=0`, which `TritSet::verdict_at` reads as a definite-False.
    let mut tallied = bitvec![usize, Lsb0; 0; n];
    for (cube, trit) in &result.final_verdicts.cube_verdicts {
        tallied.set(*cube, true);
        match trit {
            crate::mu_calculus::trit::Trit::True => {
                must.set(*cube, true);
                may.set(*cube, true);
            }
            crate::mu_calculus::trit::Trit::Unknown => {
                may.set(*cube, true);
            }
            crate::mu_calculus::trit::Trit::False => {}
        }
    }
    // SOUNDNESS (R-F5.5d projection fix): an infeasible cube — one no concrete
    // state inhabits, hence absent from the feasible-cube tally — is VACUOUS,
    // not a definite violation. Projecting it to ⊥ (`may=1, must=0`) prevents an
    // infeasible `init_cube` from reading as a spurious VIOLATED via
    // `TritSet::verdict_at` (which maps `must=0, may=0` → `Trit::False`).
    // Feasible cubes that genuinely evaluate to False stay `must=0, may=0` (a
    // real violation is preserved) because they ARE tallied.
    for c in 0..n {
        if !tallied[c] {
            may.set(c, true);
        }
    }
    crate::mu_calculus::trit::TritSet::from_parts(must, may)
}

/// A property's definite verdict as a bool (`Some(true)` = Holds, `Some(false)` = Violated),
/// or `None` for an undecided (⊥) outcome. The portfolio merges on this.
pub(crate) fn outcome_definite(o: &VerifyOutcome) -> Option<bool> {
    match o {
        VerifyOutcome::Holds => Some(true),
        VerifyOutcome::Violated { .. } => Some(false),
        VerifyOutcome::Unknown { .. } | VerifyOutcome::Skipped { .. } => None,
    }
}

/// Merge the per-engine single-engine reports into one portfolio report. For each property
/// (matched by name), the merged outcome is the definite verdict from the FIRST engine (in
/// precision order) that produced one — carrying that engine's counterexample witness; a ⊥
/// survives only if EVERY engine left it undecided. **Runtime soundness guard:** if two engines
/// return OPPOSITE definite verdicts (Holds vs Violated) the property is forced to ⊥ with a
/// `portfolio-soundness-alarm` note — never a silent pick (the runtime form of the parity gate;
/// the differential proved this never fires on the corpus). Pure over the input reports, so it is
/// unit-testable without the toolchain.
pub(crate) fn merge_portfolio_reports(
    runs: &[(&str, Result<AutoVerifyReport, AdapterError>)],
    mode: PortfolioMode,
) -> Result<AutoVerifyReport, AdapterError> {
    // Successful reports, in precision order (exact first). Errors contribute nothing.
    let oks: Vec<(&str, &AutoVerifyReport)> = runs
        .iter()
        .filter_map(|(l, r)| r.as_ref().ok().map(|rep| (*l, rep)))
        .collect();
    let Some(&(_, base)) = oks.first() else {
        // Every engine errored — surface the first (highest-precision) error.
        return Err(runs
            .iter()
            .find_map(|(_, r)| r.as_ref().err().cloned())
            .unwrap_or_else(|| AdapterError {
                kind: AdapterErrorKind::IrConsistencyError,
                message: "portfolio: no engine produced a report".to_string(),
                location: None,
            }));
    };

    // The base (exact if available) supplies the property list, diagnostics, and notes.
    let mut merged = base.clone();
    let mut decided_by: std::collections::BTreeMap<&str, u32> = std::collections::BTreeMap::new();
    let mut contradictions: Vec<String> = Vec::new();

    for prop in merged.properties.iter_mut() {
        let candidates: Vec<(&str, &PropertyVerdict)> = oks
            .iter()
            .filter_map(|(l, rep)| {
                rep.properties
                    .iter()
                    .find(|p| p.name == prop.name)
                    .map(|p| (*l, p))
            })
            .collect();
        let has_true = candidates
            .iter()
            .any(|(_, p)| outcome_definite(&p.outcome) == Some(true));
        let has_false = candidates
            .iter()
            .any(|(_, p)| outcome_definite(&p.outcome) == Some(false));

        if has_true && has_false {
            // CONTRADICTION — one engine is unsound on this design. Degrade to ⊥, loudly.
            let trues: Vec<&str> = candidates
                .iter()
                .filter(|(_, p)| outcome_definite(&p.outcome) == Some(true))
                .map(|(l, _)| *l)
                .collect();
            let falses: Vec<&str> = candidates
                .iter()
                .filter(|(_, p)| outcome_definite(&p.outcome) == Some(false))
                .map(|(l, _)| *l)
                .collect();
            contradictions.push(format!(
                "{}: Holds@[{}] vs Violated@[{}]",
                prop.name,
                trues.join(","),
                falses.join(",")
            ));
            prop.outcome = VerifyOutcome::Unknown { unknown_cells: 0 };
            prop.counterexample = None;
            continue;
        }

        if let Some((label, pv)) = candidates
            .iter()
            .find(|(_, p)| outcome_definite(&p.outcome).is_some())
        {
            prop.outcome = pv.outcome.clone();
            prop.counterexample = pv.counterexample.clone();
            *decided_by.entry(*label).or_default() += 1;
        } else {
            // All ⊥ — prefer an Unknown (abstraction attempted, undecided) over a Skipped
            // (atom not cube-bindable) so the merged ⊥ carries the most informative cause.
            if let Some((_, pv)) = candidates
                .iter()
                .find(|(_, p)| matches!(p.outcome, VerifyOutcome::Unknown { .. }))
            {
                prop.outcome = pv.outcome.clone();
            } else if let Some((_, pv)) = candidates
                .iter()
                .find(|(_, p)| matches!(p.outcome, VerifyOutcome::Skipped { .. }))
            {
                prop.outcome = pv.outcome.clone();
            }
            prop.counterexample = None;
        }
    }

    let mode_str = match mode {
        PortfolioMode::Sequential => "sequential",
        PortfolioMode::Parallel => "parallel",
    };
    let n_props = merged.properties.len();
    let n_decided = merged
        .properties
        .iter()
        .filter(|p| outcome_definite(&p.outcome).is_some())
        .count();
    let engines_ran: Vec<String> = oks.iter().map(|(l, _)| (*l).to_string()).collect();
    let mut items: Vec<String> = engines_ran.iter().map(|l| format!("ran:{l}")).collect();
    items.extend(
        decided_by
            .iter()
            .map(|(l, n)| format!("decided-by:{l}={n}")),
    );
    merged.notes.push(VerificationNote {
        kind: "portfolio".to_string(),
        level: NoteLevel::Info,
        summary: format!("portfolio-{mode_str}: {n_decided}/{n_props} properties decided"),
        detail: format!(
            "portfolio-{mode_str}: {} engine(s) ran ({}); {n_decided}/{n_props} properties decided. \
             Each property took the definite verdict from the first engine (exact → symbolic → \
             explicit) to decide it; a ⊥ means every engine left it undecided.",
            engines_ran.len(),
            engines_ran.join(", ")
        ),
        items,
    });
    if !contradictions.is_empty() {
        merged.notes.push(VerificationNote {
            kind: "portfolio-soundness-alarm".to_string(),
            level: NoteLevel::SoundnessCaveat,
            summary: "engines returned CONTRADICTING definite verdicts (forced to ⊥)".to_string(),
            detail: format!(
                "two portfolio engines returned OPPOSITE definite verdicts on the same property; \
                 the merged outcome is ⊥ (not silently picked) — one engine is unsound on this \
                 design and must be investigated: {}",
                contradictions.join("; ")
            ),
            items: contradictions,
        });
    }
    Ok(merged)
}

pub fn verify_auto(
    sources: &[(String, String)],
    yosys_opts: &YosysOptions,
    opts: &VerifyAutoOptions,
) -> Result<AutoVerifyReport, AdapterError> {
    // PORTFOLIO dispatch — when a portfolio mode is set, hand the task to the execution
    // planner (verification-execution-planner P2.1a): `plan()` emits the engine-operator
    // ladder, `execute()` lifts SV → BTOR2 ONCE (the lift-once common-IR keystone, roadmap
    // P2.1) and drives each operator over the shared lift, merging under the runtime
    // soundness guard. Verdict-equivalent to the former inline `verify_auto_portfolio`.
    if opts.portfolio.is_some() {
        let task = crate::planner::VerificationTask {
            sources,
            yosys_opts,
            opts,
        };
        return crate::planner::execute(&crate::planner::plan(&task), &task);
    }

    // Single-engine path: lift inline (`prelift = None`) then run the body.
    verify_auto_impl(sources, yosys_opts, opts, None, None)
}

/// The SV → BTOR2 lift output (incl. the `auto_stub_flops` re-lift), factored out so the
/// execution planner ([`crate::planner::execute`]) can run the lift **once** and share it
/// across the precision-ladder operators. The yosys lift is the dominant redundant cost of
/// the per-operator portfolio body — a 3-engine ladder otherwise re-elaborates the identical
/// SV up to three times. The cheap Rust prep (SVA extraction, reset detection / pinning,
/// BTOR2 parse, cube-atom classification) still re-runs per operator; only the lift is shared.
#[derive(Clone)]
pub(crate) struct PreLift {
    /// The lifted BTOR2 (pre reset-pin / shadow / config-pin — those stay per-operator).
    pub btor2: String,
    /// Modules the lift black-boxed (instantiated, no body) — surfaced in diagnostics.
    pub blackboxed_modules: Vec<String>,
    /// Flop-primitive stubs auto-injected on the H.C re-lift (empty when none).
    pub auto_provided_stubs: Vec<String>,
}

/// Run the SV → BTOR2 lift (yosys) with the `auto_stub_flops` re-lift. Pure function of
/// `(sources, yosys_opts, opts)` — the same inputs every precision-ladder operator shares —
/// so the planner calls it once and threads the [`PreLift`] into each [`verify_auto_impl`].
pub(crate) fn lift_sv(
    sources: &[(String, String)],
    yosys_opts: &YosysOptions,
    opts: &VerifyAutoOptions,
) -> Result<PreLift, AdapterError> {
    use crate::adapter::yosys::sv_to_btor2_with_blackboxes;
    let (_, primary_content) = sources.first().ok_or_else(|| AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: "adapter/slang/verify_auto: no SV sources provided".to_string(),
        location: None,
    })?;
    // 2. SV → flattened BTOR2. The lift reports modules it black-boxed (instantiated, no
    // body) — surfaced in `diagnostics.blackboxed_modules` so a cut FSM (the
    // `prim_sparse_fsm_flop`-class root cause) is visible, not silent.
    let (mut btor2, mut blackboxed_modules) =
        sv_to_btor2_with_blackboxes(primary_content, yosys_opts).map_err(|mut e| {
            e.message = format!("verify_auto: SV → BTOR2: {}", e.message);
            e
        })?;
    // H.C — if the first lift cut a known flop primitive, inject a behavioral stub for it and
    // re-lift so the register survives (e.g. OpenTitan's `prim_sparse_fsm_flop`). Only stubs
    // ACTUALLY-cut modules (no collision with designs that provide their own body).
    let mut auto_provided_stubs = Vec::new();
    if opts.auto_stub_flops {
        let stubs = crate::adapter::slang::prim_stubs::stubs_for_cut_modules(&blackboxed_modules);
        if !stubs.is_empty() {
            let mut yopts2 = yosys_opts.clone();
            yopts2.additional_sources.extend(stubs.iter().cloned());
            let (b2, bb2) =
                sv_to_btor2_with_blackboxes(primary_content, &yopts2).map_err(|mut e| {
                    e.message = format!("verify_auto: SV → BTOR2 (with flop stubs): {}", e.message);
                    e
                })?;
            btor2 = b2;
            blackboxed_modules = bb2;
            auto_provided_stubs = stubs
                .iter()
                .map(|(name, _)| name.trim_end_matches(".sv").to_string())
                .collect();
        }
    }
    Ok(PreLift {
        btor2,
        blackboxed_modules,
        auto_provided_stubs,
    })
}

/// P-3 (common-IR hub) — the SVA + annotation extraction result, factored out so the execution
/// planner can run the slang `extract_sva` subprocess ONCE and share it across the precision-ladder
/// operators — exactly as [`PreLift`] does for the yosys lift. Without this the portfolio
/// re-extracts per engine operator (a slang subprocess apiece). Clone so each operator owns a copy.
#[derive(Clone)]
pub(crate) struct SharedExtraction {
    /// The translated SVA with the `@mununu_guarantee` properties merged in.
    extraction: crate::adapter::slang::translate::TranslationReport,
    /// The `@mununu_*` annotation scan (drives the annotation note).
    ann_scan: AnnotationScan,
    /// Abstraction-predicate hints (in-source `@mununu_predicate` + CLI/API `--predicate`).
    predicate_hints: Vec<String>,
}

/// Run the SVA + annotation extraction (the slang subprocess + the source annotation scan), merging
/// the `@mununu_guarantee` properties into the translated set. Pure function of `(sources, opts)` —
/// the same inputs every precision-ladder operator shares — so the planner calls it once and threads
/// the [`SharedExtraction`] into each [`verify_auto_impl`].
pub(crate) fn extract_front(
    sources: &[(String, String)],
    opts: &VerifyAutoOptions,
) -> Result<SharedExtraction, AdapterError> {
    let mut extraction = extract_sva_with_options(
        sources,
        &TranslateOptions {
            gate_reset: opts.gate_reset,
        },
    )?;
    let ann_scan = scan_annotation_properties(sources);
    let predicate_hints: Vec<String> = ann_scan
        .predicates
        .iter()
        .chain(opts.predicate_hints.iter())
        .cloned()
        .collect();
    extraction
        .translated
        .extend(ann_scan.guarantees.iter().cloned());
    Ok(SharedExtraction {
        extraction,
        ann_scan,
        predicate_hints,
    })
}

/// The single-engine verify body: extraction → lift → reset/pin → per-property engine →
/// replan / rescue / notes. `prelift` is `Some` when the planner supplies a shared SV → BTOR2
/// lift (the portfolio path lifts once), `None` to lift inline (the single-engine path).
/// Verdict-equivalent either way — the only difference is whether the yosys lift is reused.
pub(crate) fn verify_auto_impl(
    sources: &[(String, String)],
    yosys_opts: &YosysOptions,
    opts: &VerifyAutoOptions,
    prelift: Option<PreLift>,
    shared_extraction: Option<SharedExtraction>,
) -> Result<AutoVerifyReport, AdapterError> {
    use crate::adapter::AdapterOptions;
    use crate::adapter::btor2::cegar::{
        CegarOptions, LiftStrategy, PredicateSource, cegar_refine_loop,
    };
    use crate::adapter::btor2::shadow::augment_with_past_shadows;
    use crate::mu_calculus::Environment;
    use crate::mu_calculus::parser as mu_parser;
    use crate::mu_calculus::trit::Trit;

    let (primary_name, _) = sources.first().ok_or_else(|| AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: "adapter/slang/verify_auto: no SV sources provided".to_string(),
        location: None,
    })?;
    let _ = primary_name;

    // A.6 (soundness-ledger, 2026-07-05) — reject `--engine exact-symbolic` with
    // `--no-gate-reset`. The exact full-state engine is built for the reset-GATED
    // regime: the reset input is pinned inactive and the initial state is the
    // modeled reset state (`init` = the post-reset value injected by
    // `inject_reset_init`, or 0). With reset-gating OFF, that injection is a no-op
    // (no `init` line), so the engine starts from the power-on default 0 — an
    // illegal sparse encoding the real design never occupies — and it does not
    // explore the async reset (a runtime `state_q_next = rst_ni ? FSM : ResetValue`
    // next-mux) as a firing transition. The result is a *spurious VIOLATED*
    // (idle unreachable) presented with the exact engine's 2-valued *definite*
    // authority — a confident wrong answer. Modeling a freed reset soundly needs a
    // universal (havoc) initial state + reset-as-transition semantics the exact
    // engine does not have; until then reject the combination rather than emit an
    // unsound verdict. Free-reset reachability is available on the cube engine
    // (drop `--engine exact-symbolic`), whose over-approximating may-relation
    // soundly includes the reset edge.
    if opts.exact_symbolic && !opts.gate_reset {
        return Err(AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message:
                "adapter/slang/verify_auto (A.6): `--engine exact-symbolic` is not sound with \
                      `--no-gate-reset`. The exact engine models the post-reset state space (reset \
                      pinned inactive, init = the modeled reset state); with reset-gating off it \
                      starts from the illegal power-on default 0 and does not fire the freed async \
                      reset as a transition, yielding a spurious (but definite-looking) VIOLATED. \
                      For free-reset reachability, use the cube engine (drop `--engine \
                      exact-symbolic`); to use the exact engine, keep reset-gating on."
                    .to_string(),
            location: None,
        });
    }

    // 1. Extract + translate the SVA (slang subprocess) and merge any `@mununu_guarantee`
    // annotation properties. When reset-gating, the `disable iff` guards are dropped from the
    // formulas and the recognized reset signals are reported (pinned inactive in the lift below).
    // P-3 common-IR hub: `shared_extraction` is `Some` on the portfolio path (the planner extracted
    // ONCE and shares it across the precision-ladder operators, so the slang subprocess runs once
    // not per-engine), `None` on the single-engine path (extract inline). Verdict-equivalent either
    // way. `predicate_hints` (W1/W2) merge in-source `@mununu_predicate` with CLI/API hints — sound
    // by monotonicity. The merge happens BEFORE the empty-check so an annotation-only design verifies.
    let SharedExtraction {
        extraction,
        ann_scan,
        predicate_hints,
    } = match shared_extraction {
        Some(s) => s,
        None => extract_front(sources, opts)?,
    };

    let mut report = AutoVerifyReport {
        unsupported: extraction
            .unsupported
            .iter()
            .map(|u| (u.name.clone(), u.reason.clone()))
            .collect(),
        ..Default::default()
    };
    if extraction.translated.is_empty() {
        // No properties: the posture note is informational; reflect the engine.
        let posture = if opts.exact_symbolic {
            NotePosture::Exact
        } else {
            NotePosture::Cube
        };
        report.notes = build_notes(
            &report,
            opts.must_edge_inference,
            &[],
            &[],
            &yosys_opts.cutpoint_signals,
            &posture,
        );
        if let Some(n) = annotation_note(&ann_scan) {
            report.notes.push(n);
        }
        return Ok(report);
    }

    // Reset inputs to pin inactive (reset-gating) — applied to the lifted BTOR2
    // below via `pin_inputs_to_constants`. Empty when gating is off or no
    // `disable iff` reset was recognized.
    let mut reset_pins: Vec<(String, u64)> = if opts.gate_reset {
        extraction
            .reset_signals
            .iter()
            .map(|rs| (rs.signal.clone(), rs.inactive_value))
            .collect()
    } else {
        Vec::new()
    };

    // 2. SV → flattened BTOR2 (then 3. augment the `__past` shadow flops, below). Lift-once
    // keystone (roadmap P2.1): the yosys lift is the dominant redundant cost when the
    // portfolio runs this body once per engine operator. The planner (`execute`) lifts ONCE
    // via `lift_sv` and threads the result in as `prelift`, so a 3-engine precision ladder
    // does ONE yosys lift instead of up to three. On the single-engine path `prelift` is
    // `None` and the lift runs inline — verdict-equivalent either way. The black-boxed
    // modules + auto-injected flop stubs are surfaced in diagnostics (a cut FSM, the
    // `prim_sparse_fsm_flop`-class root cause, is visible not silent).
    let PreLift {
        mut btor2,
        blackboxed_modules,
        auto_provided_stubs,
    } = match prelift {
        Some(p) => p,
        None => lift_sv(sources, yosys_opts, opts)?,
    };
    report.diagnostics.blackboxed_modules = blackboxed_modules;
    report.diagnostics.auto_provided_stubs = auto_provided_stubs;

    // Structural reset auto-pin. When reset-gating is on but no `disable iff (reset)`
    // guard was recognized (`reset_pins` empty — the common case for a plain-RTL
    // `@mununu_guarantee` design with no SVA), detect the async-reset input from the
    // lifted BTOR2's reset-mux structure (`next(state) = ite(reset, RESET_CONST, normal)`
    // / active-low branch-swap) and pin it inactive. Without this the reset input stays a
    // free primary input, so the transition relation carries a reset-edge from every
    // state to the reset state — and every `[]`/`AF`/box-universal property is then
    // spuriously VIOLATED (it would have to hold at the reset state too). Diamond/`EF`/
    // `AG EF` properties are immune (existential), which is why they verified correctly
    // even without this. The detected reset flows through `inject_reset_init` and
    // `pin_inputs_to_constants` exactly like a `disable iff`-recognized one, and is
    // surfaced in `diagnostics.gated_resets`. The `disable iff` path always wins (guard
    // on `reset_pins.is_empty()`), mirroring the fsm-encoding scan's reset handling.
    if opts.gate_reset
        && reset_pins.is_empty()
        && let Ok(file) = crate::adapter::btor2::parser::parse(&btor2)
    {
        let symbols = crate::adapter::btor2::parser::collect_symbols(&file);
        reset_pins.extend(crate::adapter::fsm_scan::detect_resets(&file, &symbols));
    }

    // Inject `init` lines at the post-reset state so a reset-gated async-reset
    // FSM starts in its real reset state (e.g. an OpenTitan sparse-FSM's non-zero
    // `MainSmIdle = 6'b110111`) rather than the init-less power-on default 0.
    // Zero is an illegal sparse encoding: the FSM's `default` arm traps it into
    // its error state, so without this the reset-gated model starts in a state
    // the real design never occupies, silently corrupting every reset-dependent
    // verdict (recoverability `AG EF idle`, liveness-from-reset). Runs on the
    // reset-FREE BTOR2 (before the pin below) so it can assert the reset to
    // derive the post-reset state; no-op unless reset-gating is on (`reset_pins`
    // non-empty) and the design carries no authoritative `init` line.
    btor2 = crate::adapter::btor2::reset_init::inject_reset_init(&btor2, &reset_pins)?;

    // Complete the init to the `setundef -zero` power-up for a RESET-LESS design:
    // every init-less bitvec state cell gets an explicit `init … 0`. A design with
    // no reset (`reset_pins` empty) has no reset mechanism to establish its start
    // state — its init is fixed ONLY by `initial` values plus the FPGA/`setundef
    // -zero` power-up (0) for the rest, which is exactly what the cube and exact
    // engines already assume (`state_cell_init_values` / `initial_state_bdd`). The
    // reachability portfolio (native BMC / spacer / Boolector) otherwise leaves an
    // init-less cell FREE (BTOR2's nondeterministic-init semantics), so a
    // reset-less design with a partial `initial` (a Xilinx wrapper: `initial state
    // = IDLE` but an init-less status flop) hands the portfolio a spurious
    // power-up counterexample the exact engine never sees — an engine verdict
    // disagreement. Making the 0 power-up explicit puts every engine on the same
    // start state.
    //
    // Scoped to reset-LESS designs on purpose: a RESET-based design establishes
    // its operational init through the reset (`inject_reset_init`), and the
    // portfolio's free-init for any cell the reset does not pin is the existing,
    // real-RTL-validated behavior (`e2e_btormc_confirms_sysrst_real_rtl_verdict`
    // et al.) — a free-init counterexample there is a genuine run of the model, so
    // it must not be clamped. Raw `btor2 verify` (no reset-gating) also keeps
    // BTOR2's free-init semantics.
    if opts.gate_reset && reset_pins.is_empty() {
        btor2 = crate::adapter::btor2::reset_init::inject_zero_init(&btor2)?;
    }

    // Augment only the `__past` shadows whose base resolves to a BTOR2 state
    // cell. A base the lift renamed away (the SVA name not matching the lifted
    // register) would hard-error `augment_with_past_shadows`, aborting the whole
    // run; instead we skip it here and the properties that need its `__past`
    // atom are SKIPPED below (the atom finds no state cell). Graceful per-design
    // degradation, not an abort.
    let pre_file = crate::adapter::btor2::parser::parse(&btor2).map_err(|mut e| {
        e.message = format!("verify_auto: parse lifted BTOR2: {}", e.message);
        e
    })?;
    let shadow_bases: Vec<&str> = extraction
        .required_shadows
        .iter()
        .map(|s| s.base.as_str())
        .filter(|b| crate::adapter::btor2::parser::resolve_state_by_symbol(&pre_file, b).is_some())
        .collect();
    let btor2 = augment_with_past_shadows(&btor2, &shadow_bases)?;

    // Pin recognized reset inputs inactive so the (un-guarded) bodies are
    // verified only while not in reset (the model-level half of reset-gating).
    // `gated_resets` records only the resets actually found + pinned in the
    // BTOR2 — a recognized reset that the lift renamed/optimized away is left
    // unpinned rather than misreported.
    let (btor2, pinned_resets) =
        crate::adapter::btor2::pin::pin_inputs_to_constants(&btor2, &reset_pins);
    report.diagnostics.gated_resets = pinned_resets;

    // H.J.b — pin user-supplied config inputs to constants (scope-reduced
    // concretization; same mechanism as reset-gating). Only signals that are
    // actual inputs are pinned; `pinned_configs` is the applied set, which drives
    // both the per-property formula substitution below and the
    // `config-concretization` ScopeCaveat note. Sorted for a deterministic order.
    let config_pins: Vec<(String, u64)> = {
        let mut v: Vec<(String, u64)> = opts
            .config_values
            .iter()
            .map(|(k, val)| (k.clone(), *val))
            .collect();
        v.sort();
        v
    };
    let (btor2, pinned_configs) =
        crate::adapter::btor2::pin::pin_inputs_to_constants(&btor2, &config_pins);
    let applied_config_values: Vec<(String, u64)> = pinned_configs
        .iter()
        .filter_map(|s| {
            let (n, val) = s.split_once('=')?;
            Some((n.to_string(), val.trim().parse::<u64>().ok()?))
        })
        .collect();

    // State-cell symbols of the augmented design — the seedable-atom universe.
    let file = crate::adapter::btor2::parser::parse(&btor2).map_err(|mut e| {
        e.message = format!("verify_auto: re-parse augmented BTOR2: {}", e.message);
        e
    })?;
    let symbols = crate::adapter::btor2::parser::collect_symbols(&file);
    let state_cells: HashSet<String> = file
        .lines
        .iter()
        .filter(|l| matches!(l.node, crate::adapter::btor2::ast::Node::State { .. }))
        .filter_map(|l| symbols.get(&l.nid).cloned())
        .collect();
    // Total `state` register lines (incl. any unnamed) — a zero/low count is
    // the headline signal that state was cut or optimized away.
    report.diagnostics.state_register_count = file
        .lines
        .iter()
        .filter(|l| matches!(l.node, crate::adapter::btor2::ast::Node::State { .. }))
        .count();
    // Init value of each state cell — pins the cube lift's initial cube to the
    // design's reset state (else it defaults to cube_0 = all-predicates-false).
    let init_values = state_cell_init_values(&file);

    // H.A — a name is cube-bindable if it is a state-cell symbol directly OR a
    // value-identical alias of one (`resolve_state_alias`): a `uext`/`sext`-0
    // rename, or the async-reset register mux that `async2sync` puts around the
    // state (e.g. `state_q` over the auto-stubbed flop). The strict resolver
    // rejects combinational *functions* of state (an `eq`/`or` output like
    // `main_sm_err_o`) — binding those to the state's value would be a spurious
    // verdict. The reset-mux edge is followed only when the reset is pinned
    // (gated_resets non-empty), which makes the register value equal the state.
    let reset_pinned = !report.diagnostics.gated_resets.is_empty();
    let resolves_to_state = |name: &str| -> bool {
        state_cells.contains(name)
            || crate::adapter::btor2::parser::resolve_state_alias(&file, name, reset_pinned)
                .is_some()
    };

    // H.B (free-input atoms) — primary-input symbols of the lifted design. A
    // simple atom over one of these is admitted as a *free* cube dimension (the
    // env picks any value each cycle), so the dominant real-SVA blocker — IO /
    // config antecedents like sysrst's `cfg_enable_i` / `trigger_*` / `cnt_clr`
    // — is verifiable instead of SKIPPED. `is_input` is consulted only after
    // `is_state` (state precedence), so a register that is both never
    // double-classifies. A reset input pinned by reset-gating is NOT a free
    // input (it was rewritten to a constant), so it never appears here.
    let input_symbols: HashSet<String> = file
        .lines
        .iter()
        .filter(|l| matches!(l.node, crate::adapter::btor2::ast::Node::Input { .. }))
        .filter_map(|l| symbols.get(&l.nid).cloned())
        .collect();
    let is_input = |name: &str| -> bool { input_symbols.contains(name) };

    // AR-S2 (2026-07-06) — the cube lift now always uses the SOUND `SmtAllPairs`
    // may-relation (retiring the pure-state sampling-may fallback + the A.4 ⊥-guard
    // that papered over its input-sampling incompleteness). A definite cube verdict
    // is therefore sound by over-approximation with no honest-⊥ downgrade needed.

    // H.E (combinational outputs) — named combinational nodes: an `Op` carrying
    // its OWN symbol that is neither a state cell (incl. value-alias, via
    // `resolves_to_state` precedence in the seeder) nor an input. Each is
    // classified by its CONE (`cone_reaches_input`):
    // - **input-dependent** (`trigger_active = !trigger_i`) → routed through the
    //   FREE-INPUT path (a free cube dimension, source-pin / target-free) so the
    //   may/must edges respect it — SOUND (treating it as a state-cube label is
    //   what produced the sysrst sva_6 / sva_9 spurious VIOLATED);
    // - **state-only** (`event_detected_o = f(state_q)`) → a DERIVED per-cube
    //   3-valued label (Approach B).
    // `is_state` is consulted first in the seeder, so a state value-alias (also
    // an Op with a symbol) binds as state, not combinational.
    let mut combinational_nid: std::collections::HashMap<String, crate::adapter::btor2::ast::Nid> =
        file.lines
            .iter()
            .filter_map(|l| match &l.node {
                crate::adapter::btor2::ast::Node::Op {
                    symbol: Some(s), ..
                } if !state_cells.contains(s) && !input_symbols.contains(s) => {
                    Some((s.clone(), l.nid))
                }
                _ => None,
            })
            .collect();
    // H.U.1d/H.U.2 — also map a combinational signal named only on an `output`
    // line (a top-level combinational output port whose driving Op carries no own
    // symbol — e.g. `assign bad = (state == 3)`). Mirrors the SMT view's
    // output-line registration so the seeder classifier + the cegar nid-map agree.
    // `resolves_to_state` / `is_input` are consulted first in the seeder, so an
    // output that aliases a state cell still binds as state.
    for l in &file.lines {
        if let crate::adapter::btor2::ast::Node::Output {
            symbol: Some(s),
            signal,
        } = &l.node
            && !state_cells.contains(s)
            && !input_symbols.contains(s)
        {
            combinational_nid.entry(s.clone()).or_insert(signal.nid());
        }
    }
    let combinational_kind = |name: &str| -> Option<CombKind> {
        combinational_nid.get(name).map(|&nid| {
            if crate::adapter::btor2::parser::cone_reaches_input(&file, nid) {
                CombKind::InputDependent
            } else {
                CombKind::StateOnly
            }
        })
    };
    // Slice 3 (the unwrap lever) — the raw free inputs in a combinational-of-
    // input signal's cone. Seeding these as free H.B cube dimensions refines the
    // may-relation (so a consequent box over a conditional transition governed
    // by the input becomes definite) while the combinational stays a derived
    // label that turns definite at cubes pinning the input. Empty for a
    // non-combinational name (resolves to no combinational node).
    let cone_inputs_of = |name: &str| -> Vec<String> {
        combinational_nid
            .get(name)
            .map(|&nid| crate::adapter::btor2::parser::cone_inputs(&file, nid))
            .unwrap_or_default()
    };

    // H.H — auto-derive counter upper bounds from the concretized config (a
    // counter compared against a pinned config threshold, `cnt_q >=
    // cfg_detect_timer_i` with cfg=7, yields `cnt_q <= 7`). Global across
    // properties: the property that NEEDS the bound (sva_13) doesn't reference the
    // config; the comparison lives in a sibling (sva_5). Empty without config.
    let config_inferred_bounds =
        config_inferred_counter_bounds(&extraction.translated, &applied_config_values);
    // Accumulates the applied bound descriptions for the report-level counter-bound
    // note (sorted, deduped across properties).
    let mut counter_bound_items: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();

    // 4. Per property: seed → CEGAR → verdict.
    for t in &extraction.translated {
        // H.J.b — substitute the concretized config inputs (`cfg_* → const`) so a
        // relational-with-wide-input atom becomes a decidable state-vs-constant
        // comparison. The substituted string is BOTH parsed and reported (the
        // verdict shows `cnt_q >= 7`, consistent with the config-concretization
        // note). No-op when no config was pinned.
        let formula_str = substitute_config_in_formula(&t.formula, &applied_config_values);
        let formula = match mu_parser::parse(&formula_str) {
            Ok(f) => f,
            Err(e) => {
                report.properties.push(PropertyVerdict {
                    name: t.name.clone(),
                    kind: t.kind,
                    formula: formula_str.clone(),
                    outcome: VerifyOutcome::Skipped {
                        reason: format!("formula failed to parse: {e:?}"),
                    },
                    seeded_predicates: Vec::new(),
                    counterexample: None,
                });
                continue;
            }
        };

        // D1.6 — exact full-state symbolic MC (`--engine exact-symbolic`): decide
        // the property EXACTLY over the reset-gated btor2's bit-blasted state — no
        // predicate abstraction, so a **definite** 2-valued verdict (never ⊥). Runs
        // BEFORE cube seeding: the exact engine binds the formula's atoms directly
        // (register-name resolution), so it needs no cube and must NOT be gated
        // behind cube-seeding success — deciding a property the cube path cannot
        // seed is exactly this engine's purpose. The btor2 here is reset-gated
        // (reset input pinned inactive) and `$past`-shadow-augmented; the init is
        // the modelled reset state (`initial_state_bdd`: every register pinned to
        // its `init` value or 0). Decides `AF`-liveness (and any μ-calculus
        // property) where the cube path returns Unknown; bounded by BDD size
        // (build errors above the bit cap ⇒ Skipped).
        if opts.exact_symbolic {
            use crate::adapter::btor2::symbolic_bitblast::{
                ExactVerdict, exact_symbolic_verdict_with_witness,
            };
            let (outcome, counterexample) =
                match exact_symbolic_verdict_with_witness(&btor2, &formula) {
                    Ok((ExactVerdict::Holds, _)) => (VerifyOutcome::Holds, None),
                    Ok((ExactVerdict::Violated, witness)) => {
                        // A definite full-state counterexample (not a cube tally). For a
                        // liveness/recoverability failure (`AF p` / `AG AF p` / `AG EF p`)
                        // the witness carries the concrete path (stall lasso / trap path).
                        // Failing that, a bare `EF p` (reachability) violation carries the
                        // A.4 unreachable-target witness — no trace, because the target is
                        // simply never reached (sound under the over-approximating cut).
                        (
                            VerifyOutcome::Violated { false_cells: 1 },
                            witness
                                .map(exact_counterexample_from_lasso)
                                .or_else(|| ef_unreachable_counterexample(&formula)),
                        )
                    }
                    Err(e) => {
                        // RANKING RESCUE — the bit-blaster is over-cap (a wide counter);
                        // a recoverability `AG EF <cnt == N>` may still HOLD by a
                        // well-founded datapath descent the exact engine can't fit. Route
                        // it to the scalable ranking/recoverability engine before abstaining.
                        match try_ranking_recoverability(&btor2, &formula) {
                            Some(outcome) => (outcome, None),
                            None => (
                                VerifyOutcome::Skipped {
                                    reason: format!("exact symbolic MC: {e}"),
                                },
                                None,
                            ),
                        }
                    }
                };
            report.properties.push(PropertyVerdict {
                name: t.name.clone(),
                kind: t.kind,
                formula: formula_str.clone(),
                outcome,
                seeded_predicates: Vec::new(),
                counterexample,
            });
            continue;
        }

        let mut seeded = seed_from_formula(
            &formula,
            resolves_to_state,
            is_input,
            combinational_kind,
            cone_inputs_of,
            &predicate_hints,
        );
        // H.H — seed a bound compound `X <= K` for each counter register (a state
        // cell whose `$past` shadow is also present, i.e. a self-relational
        // monotonicity / increment atom). The bound is a sound cube-dimension
        // PARTITION (not an assumption); with must-edges on it excludes the abstract
        // wraparound state that leaves `cnt_q >= $past(cnt_q)` at ⊥. Manual bounds
        // override the config-inferred ones. No-op when no counter / no bound.
        let counters = counter_registers_in(&seeded);
        for (reg, k, provenance) in seed_counter_bounds(
            &mut seeded,
            &counters,
            &opts.counter_bounds,
            &config_inferred_bounds,
        ) {
            counter_bound_items.insert(format!("{reg} <= {k} ({provenance})"));
        }
        if !seeded.unseedable.is_empty() {
            let reason = unseedable_skip_reason(&seeded.unseedable, &report.diagnostics);
            report.properties.push(PropertyVerdict {
                name: t.name.clone(),
                kind: t.kind,
                formula: formula_str.clone(),
                outcome: VerifyOutcome::Skipped { reason },
                seeded_predicates: Vec::new(),
                counterexample: None,
            });
            continue;
        }
        let predicate_count = seeded.specs.len() + seeded.compounds.len();
        if predicate_count == 0 && seeded.derived.is_empty() && seeded.derived_relational.is_empty()
        {
            report.properties.push(PropertyVerdict {
                name: t.name.clone(),
                kind: t.kind,
                formula: formula_str.clone(),
                outcome: VerifyOutcome::Skipped {
                    reason: "no state-cell / combinational predicate atoms to seed the cube"
                        .to_string(),
                },
                seeded_predicates: Vec::new(),
                counterexample: None,
            });
            continue;
        }

        let mut seeded_names: Vec<String> = seeded.specs.iter().map(|s| s.name.clone()).collect();
        seeded_names.extend(seeded.compounds.iter().map(|(n, _)| n.clone()));
        seeded_names.extend(seeded.derived.iter().map(|d| d.name.clone()));
        seeded_names.extend(seeded.derived_relational.iter().map(|(n, _)| n.clone()));

        // AR-S2 — the cube lift always uses the SmtAllPairs eager seam now, so the
        // old per-property may-policy branch (compounds / inputs / derived /
        // combinational forcing SmtAllPairs, else sampling) is gone: every property
        // gets the sound may. `cegar_refine_loop` still re-checks the compound gate.
        let cegar_opts = CegarOptions {
            max_iterations: opts.max_iterations,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
            smart_uf_cap: true,
            lift_strategy: LiftStrategy::Eager,
            // Derived-label properties now use the caller's must-edge policy like
            // any other. The H.E.r2 `must=Off`-for-derived band-aid is RETIRED:
            // its purpose was to dodge a spurious VIOLATED on a derived ⊥-label
            // safety property under `SmtHyperMust`, but the real cause was the
            // 3-valued evaluator collapsing a `KleeneBot` derived label to
            // definite-False (its negation a spurious definite-True antecedent).
            // That is fixed at the root in `mu_calculus::evaluator::predicate_bits`
            // (a `KleeneBot` label now yields `⊥`, so `¬⊥ = ⊥` and the safety
            // implication is `⊥`, never a spurious `False`). With the root fix the
            // must relation is sound for these properties too.
            must_edge_inference: opts.must_edge_inference,
            // AR-S2 — always the sound all-pairs SMT may-relation (the pure-state
            // sampling-may fallback + its A.4 ⊥-guard are retired).
            may_edge_inference: MayEdgeInference::SmtAllPairs,
            emit_ctxdsl: false,
        };
        // Every register the predicates reference — pin each to its init value
        // (config_values) so the lift's initial cube is the reset state.
        let mut referenced: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for s in &seeded.specs {
            referenced.insert(s.register.clone());
        }
        for (_, e) in &seeded.compounds {
            for r in e.registers() {
                referenced.insert(r);
            }
        }
        // H.F — pin the state operands of relational derived predicates too (the
        // input operands have no `init_values` entry, so they are silently skipped
        // for config_values — correct, a free input has no reset value).
        for (_, e) in &seeded.derived_relational {
            for r in e.registers() {
                referenced.insert(r);
            }
        }
        let adapter_options = AdapterOptions {
            sidecar_json: synth_sidecar_json(
                &seeded.compounds,
                &seeded.derived,
                &seeded.derived_relational,
                &referenced,
                &init_values,
            ),
            ..Default::default()
        };
        // Env sized to the cube space: simple specs + the sidecar compounds
        // `cegar_refine_loop` appends.
        let env = Environment::new(1usize << predicate_count);

        // The design's initial cube index(es): evaluate every predicate at the
        // reset valuation, in the lift's bit order (simple specs first, then the
        // sidecar compounds `cegar_refine_loop` appends). The verdict AT THE
        // RESET CUBE is the property's answer — sidestepping the lift's `cube_0`
        // initial-state default (cube_is_admissible can't pin a compound).
        //
        // H.B — a free-input spec's bit is NOT pinned at init (the environment
        // is free at cycle 0). Its bit position is collected in
        // `free_input_bits` and the verdict is read across ALL of its initial
        // polarities (the product of every free-input bit), then combined
        // conjunctively: the property holds at reset iff it holds under every
        // initial environment input.
        let init_val_u128: std::collections::HashMap<String, u128> = init_values
            .iter()
            .map(|(k, v)| (k.clone(), *v as u128))
            .collect();
        // H.U.2 — a combinational-of-state spec's register is neither a state cell
        // (present in `init_values`) nor a free input, so its reset-cube value is
        // not in `init_values`. OBSERVE the signal's value at the reset register
        // state (combinational-of-state has no input dependence → empty inputs)
        // and augment a local map, so the init-cube bit below is correct.
        let mut init_for_cube = init_values.clone();
        let comb_names: Vec<String> = seeded
            .specs
            .iter()
            .filter(|s| {
                !init_for_cube.contains_key(&s.register)
                    && !seeded.input_registers.contains(&s.register)
            })
            .map(|s| s.register.clone())
            .collect();
        if !comb_names.is_empty()
            && let Ok(outcome) = crate::adapter::btor2::bit_blast::simulate_one_step_observe(
                &file,
                &init_val_u128,
                &std::collections::HashMap::new(),
                &comb_names,
            )
        {
            for n in &comb_names {
                if let Some(&v) = outcome.observed.get(n) {
                    init_for_cube.insert(n.clone(), v as u64);
                }
            }
        }
        let mut base_init_cube = 0usize;
        let mut free_input_bits: Vec<u32> = Vec::new();
        let mut bit = 0u32;
        for s in &seeded.specs {
            if seeded.input_registers.contains(&s.register) {
                // Free input dimension — left unset in the base; enumerated below.
                free_input_bits.push(bit);
            } else if init_for_cube.get(&s.register).copied().unwrap_or(0) == s.value {
                base_init_cube |= 1 << bit;
            }
            bit += 1;
        }
        for (_, expr) in &seeded.compounds {
            if expr.eval(&init_val_u128) {
                base_init_cube |= 1 << bit;
            }
            bit += 1;
        }
        // All initial cubes = base ⊗ every combination of the free-input bits.
        // No free inputs ⇒ a single cube, identical to the pre-H.B single-read.
        let init_cubes = free_input_init_cubes(base_init_cube, &free_input_bits);

        // R-F5.5d — the final 3-valued verdict over the cube space, from the
        // selected engine. The explicit path runs `cegar_refine_loop`; the
        // symbolic path runs the R-F5 BDD CEGAR loop and projects its per-cube
        // verdicts into the same `TritSet` shape (over `2^|final P|`, same cube
        // indexing), so the init-cube read below is identical. (The
        // `--engine exact-symbolic` path returned early above, before seeding.)
        let final_verdict_res: Result<
            crate::mu_calculus::trit::TritSet,
            crate::adapter::AdapterError,
        > = if opts.symbolic_engine {
            crate::adapter::btor2::symbolic_engine::symbolic_cegar_refine(
                &btor2,
                &seeded.specs,
                &adapter_options,
                &formula,
                crate::adapter::btor2::symbolic_bitblast::MustSemantics::ForallExists,
                opts.max_iterations,
            )
            .map(|r| symbolic_final_verdict(&r))
        } else {
            cegar_refine_loop(
                &formula,
                &btor2,
                seeded.specs.clone(),
                &env,
                &adapter_options,
                &cegar_opts,
            )
            .map(|trace| trace.final_verdict)
        };
        let outcome = match final_verdict_res {
            Ok(v) => {
                // Combine the verdict over every initial input flavour (H.B):
                // a violation under SOME env input is a violation (False
                // dominates); else an undecided flavour makes it ⊥; else Holds.
                // With no free inputs `init_cubes == [base_init_cube]`, so this
                // is exactly the pre-H.B single-cube read.
                let any_false = init_cubes.iter().any(|&ic| v.verdict_at(ic) == Trit::False);
                let any_unknown = init_cubes
                    .iter()
                    .any(|&ic| v.verdict_at(ic) == Trit::Unknown);
                if any_false {
                    let false_cells = (0..v.len())
                        .filter(|&i| v.verdict_at(i) == Trit::False)
                        .count();
                    VerifyOutcome::Violated { false_cells }
                } else if any_unknown {
                    let unknown_cells = (0..v.len())
                        .filter(|&i| v.verdict_at(i) == Trit::Unknown)
                        .count();
                    VerifyOutcome::Unknown { unknown_cells }
                } else {
                    VerifyOutcome::Holds
                }
            }
            Err(e) => VerifyOutcome::Skipped {
                reason: format!("CEGAR error: {}", e.message),
            },
        };

        // AR-S2 — the cube lift uses the sound `SmtAllPairs` may-relation, so a
        // definite cube verdict is sound by over-approximation; the A.4 honest-⊥
        // downgrade (a stopgap for the retired sampling-may under-approximation)
        // is no longer needed.
        report.properties.push(PropertyVerdict {
            name: t.name.clone(),
            kind: t.kind,
            formula: formula_str.clone(),
            outcome,
            seeded_predicates: seeded_names,
            counterexample: None,
        });
    }

    // ⊥ RE-PLAN (verification-execution-planner P2.1b): the planner's reactive re-plan
    // step. When the main-path plan leaves a property ⊥, `planner::replan` dispatches it by
    // property_class to a sound reduction (safety→reach_rescue, box-AF→l2s, νμ
    // recoverability→exact+COI / cube+hyper-must) — the re-plan edges. Runs before the
    // notes/coverage build so the counts reflect any rescue; each edge is gated by its own
    // `rescue_bottom_*` opt and only fires on its recognized shape. (P2.1b routes the
    // re-plan entry through the planner, delegating the edge-application to the existing
    // rescue subsystem — verdict-equivalent; P2.1c adds the soundness plan-invariants.)
    let rescue_notes = crate::planner::replan(&mut report, &btor2, reset_pinned, opts);

    // Lever (b) — exact-symbolic rescue for cube-SKIPPED properties. The predicate cube
    // cannot seed an ATOM-LESS modal formula (no-deadlock `nu X.(<> true && [] X)`, a pure
    // `<>`/`[]` obligation) and reports it Skipped; the full-state exact engine needs no
    // seeding and decides it. Only fires on a cube run under reset-gating (the exact main
    // path already ran exact; A.6 requires reset gating). Sound: exact is the differential
    // oracle — a Skipped property is upgraded only on a DEFINITE Holds/Violated.
    let exact_skip_notes = crate::planner::rescue_skipped(&mut report, &btor2, opts);

    // H.J — provenance notes: surface every abstraction/scoping decision this run
    // made (config concretizations, posture, reset-gating, flop stubs, cut
    // modules, coverage).
    let counter_bound_items_vec: Vec<String> = counter_bound_items.into_iter().collect();
    // A.3 — the exact-symbolic engine (per-property branch above) and the
    // predicate-cube path both reach this single note build; pick the posture the
    // run actually used.
    let posture = if opts.exact_symbolic {
        NotePosture::Exact
    } else {
        NotePosture::Cube
    };
    report.notes = build_notes(
        &report,
        opts.must_edge_inference,
        &applied_config_values,
        &counter_bound_items_vec,
        &yosys_opts.cutpoint_signals,
        &posture,
    );
    if let Some(n) = annotation_note(&ann_scan) {
        report.notes.push(n);
    }
    report.notes.extend(rescue_notes);
    report.notes.extend(exact_skip_notes);

    // P2.2d — cost-annotated plan telemetry: one `plan-cost` note per property (the planner's
    // per-property cost prediction — class × cone-vs-cap × diameter → predicted engine + why) plus
    // one `plan-accuracy` summary (predicted-decidable vs actually-decided). Additive telemetry,
    // verdict-equivalent. Uses the verified formulas (post config-substitution) so it lines up with
    // the outcomes. (2a: computed here; the 2b refactor moves the prediction half into `plan()`.)
    let plan_cost_props: Vec<(String, String)> = report
        .properties
        .iter()
        .map(|p| (p.name.clone(), p.formula.clone()))
        .collect();
    let plan_cost_notes =
        crate::planner::routing_telemetry_notes(&btor2, &plan_cost_props, &report);
    report.notes.extend(plan_cost_notes);

    Ok(report)
}

/// Unified ⊥-escalation router (P3): for each property the cube portfolio left `⊥`,
/// dispatch by [`property_class`](crate::mu_calculus::Formula::property_class) to the
/// sound reduction matching its shape and upgrade the verdict in place —
///   - **Safety** (an AG-invariant) → the multi-engine reachability portfolio
///     ([`reach_portfolio_rescue`](crate::adapter::reach_rescue::reach_portfolio_rescue));
///   - **Liveness / box-AF response** `AG(a → AF b)` → liveness-to-safety
///     ([`response_liveness_rescue`](crate::adapter::liveness_rescue::response_liveness_rescue));
///   - **Liveness / νμ recoverability** `AG EF good` → the cube + `smt-hyper-must` path
///     ([`verify_recoverability_scalable`](crate::adapter::recoverability::verify_recoverability_scalable)).
///
/// Each reducer recognizes ONLY its own shape (returns `None`/abstains otherwise), so a
/// property is routed to at most one reduction and an un-reducible ⊥ is left untouched —
/// the roadmap's L3 `property_class()` promoted from a diagnostic label to the actual
/// escalation dispatch. The νμ recoverability arm closes the gap where a diamond-EF ⊥
/// was previously left undecided in verify_auto even though the cube + smt-hyper-must
/// path could decide it. Pure over `(report, design_btor2)`, so it is unit-testable on a
/// BTOR2 fixture without the SV toolchain. Per-arm rescues are gated by
/// `rescue_bottom_{safety,liveness,recoverability}`.
pub(crate) fn escalate_bottom(
    report: &mut AutoVerifyReport,
    design_btor2: &str,
    reset_pinned: bool,
    opts: &VerifyAutoOptions,
) -> Vec<VerificationNote> {
    use crate::adapter::btor2::predicate_expr::CmpOp;
    use crate::adapter::liveness_rescue::{
        LivenessVerdict, reduce_ag_ef_target, response_liveness_rescue,
    };
    use crate::adapter::reach_rescue::{RescueVerdict, reach_portfolio_rescue};
    use crate::adapter::recoverability::{verify_recoverability, verify_recoverability_scalable};
    use crate::mu_calculus::{PropertyClass, parser as mu_parser};
    use crate::verdict::PropertyVerdict;

    let mut notes = Vec::new();
    for prop in report.properties.iter_mut() {
        if !matches!(prop.outcome, VerifyOutcome::Unknown { .. }) {
            continue;
        }
        let Ok(formula) = mu_parser::parse(&prop.formula) else {
            continue;
        };
        match formula.property_class() {
            PropertyClass::Safety if opts.rescue_bottom_safety => {
                if let Some((verdict, reach)) =
                    reach_portfolio_rescue(design_btor2, &formula, reset_pinned)
                {
                    let engines = engines_of(&reach);
                    match verdict {
                        RescueVerdict::Holds => {
                            prop.outcome = VerifyOutcome::Holds;
                            notes.push(rescue_note(&prop.name, "HOLDS", &engines));
                        }
                        RescueVerdict::Violated => {
                            prop.outcome = VerifyOutcome::Violated { false_cells: 0 };
                            // P3 Update 2 — attach the concrete safety counterexample: native
                            // BMC extracts the `Init → ¬invariant` path from the same bad-monitor.
                            // A safety CEX is a finite path (empty `cycle`); the monitor adds no
                            // state, so the trace is over the original design registers.
                            if let Some(trace) = crate::adapter::reach_rescue::ag_invariant_witness(
                                design_btor2,
                                &formula,
                                reset_pinned,
                                60,
                            ) {
                                prop.counterexample = Some(ExactCounterexample {
                                    prefix: trace.states,
                                    cycle: Vec::new(),
                                    inputs: trace.inputs,
                                    unreachable_target: Vec::new(),
                                });
                            }
                            notes.push(rescue_note(&prop.name, "VIOLATED", &engines));
                        }
                        RescueVerdict::Inconclusive => {}
                    }
                }
            }
            PropertyClass::Liveness => {
                let mut handled = false;
                if opts.rescue_bottom_liveness
                    && let Some((verdict, reach)) =
                        response_liveness_rescue(design_btor2, &formula, reset_pinned)
                {
                    handled = true;
                    let engines = engines_of(&reach);
                    match verdict {
                        LivenessVerdict::Holds => {
                            prop.outcome = VerifyOutcome::Holds;
                            notes.push(liveness_rescue_note(&prop.name, "HOLDS", &engines));
                        }
                        LivenessVerdict::Violated => {
                            prop.outcome = VerifyOutcome::Violated { false_cells: 0 };
                            notes.push(liveness_rescue_note(&prop.name, "VIOLATED", &engines));
                        }
                        LivenessVerdict::Inconclusive => {}
                    }
                }
                if !handled
                    && opts.rescue_bottom_recoverability
                    && let Some(good) = reduce_ag_ef_target(&formula)
                    && good.op == CmpOp::Eq
                {
                    // SOUNDNESS: verify_recoverability_scalable runs the sound cube +
                    // smt-hyper-must νμ path and abstains (Unknown) if it cannot decide,
                    // so a ⊥ is only ever upgraded to a DEFINITE verdict.
                    let good_str = format!("{} == {}", good.signal, good.value);
                    // ③a routing — try the EXACT+COI recoverability engine FIRST. On a
                    // control signal whose cone-of-influence fits the bit cap (e.g. a
                    // wishbone `ack`/`err` whose cone excludes the datapath), exact+COI
                    // decides the νμ recoverability the cube + smt-hyper-must *abstains*
                    // on — `verify_recoverability` runs exact and falls to the scalable
                    // cube path only on an over-cap `Err`. Gated on `reset_pinned`: the
                    // exact engine is sound only under reset-gating (A.6); with a free
                    // reset use the cube path, whose over-approximating may-relation
                    // soundly includes the reset edge.
                    let recov = if reset_pinned {
                        verify_recoverability(design_btor2, &good_str)
                    } else {
                        verify_recoverability_scalable(design_btor2, &good_str, &[])
                    };
                    if let Ok(v) = recov {
                        match v {
                            PropertyVerdict::Holds => {
                                prop.outcome = VerifyOutcome::Holds;
                                notes.push(recoverability_rescue_note(&prop.name, "HOLDS"));
                            }
                            PropertyVerdict::Violated => {
                                prop.outcome = VerifyOutcome::Violated { false_cells: 0 };
                                notes.push(recoverability_rescue_note(&prop.name, "VIOLATED"));
                            }
                            PropertyVerdict::Unknown => {
                                // Provenance: the recoverability rescue RAN but the cube +
                                // smt-hyper-must abstained (sound — never a fabricated verdict).
                                // Recorded so a ⊥ that reached the rescue is explained, not silent.
                                notes.push(recoverability_rescue_note(&prop.name, "UNKNOWN"));
                                // Actionable-⊥: localize WHY it abstained (structural obstacles in
                                // the recovery cone) so the ⊥ is actionable, not a bare "don't know".
                                let diag =
                                    crate::adapter::recoverability::diagnose_recoverability_bot(
                                        design_btor2,
                                        &good_str,
                                    );
                                notes.push(recoverability_bot_reason_note(&prop.name, &diag));
                            }
                            PropertyVerdict::Skipped => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    notes
}

/// Lever (b) — exact-symbolic rescue for cube-SKIPPED properties.
///
/// The predicate-cube engine skips a property it cannot seed — most commonly an
/// ATOM-LESS modal formula (no-deadlock `nu X.(<> true && [] X)`, a pure `<>`/`[]`
/// obligation) for which there is no state-cell / combinational atom to build a cube
/// dimension from, so it is reported `Skipped` with "no … atoms to seed the cube". The
/// full-state exact engine needs no seeding: it bit-blasts the design and decides the
/// formula directly (the same engine `verify_auto` runs on the `--engine exact-symbolic`
/// path). This pass gives every still-`Skipped` property one exact attempt and upgrades
/// it in place on a DEFINITE verdict.
///
/// SOUNDNESS. Runs only under reset-gating (`opts.gate_reset`) — the exact engine's A.6
/// precondition (with reset freed it is unsound; see [`verify_auto`]). Gated to the cube
/// path (`!opts.exact_symbolic`); the exact main path already ran exact, so there is
/// nothing new to rescue there. The exact engine is the differential ORACLE (the 56
/// `symbolic_bitblast` parity tests), so a definite `Holds`/`Violated` it returns transfers
/// to the RTL exactly as the main exact branch's does — a Skipped property gaining a
/// definite verdict is pure precision, never a contradiction. An over-cap / intractable
/// design returns `Err` (bounded by the node/iteration budgets + catch_unwind) and the
/// property is left `Skipped`.
///
/// Pure over `(report, design_btor2)`, so it is unit-testable on a BTOR2 fixture without
/// the SV toolchain.
pub(crate) fn rescue_skipped_via_exact(
    report: &mut AutoVerifyReport,
    design_btor2: &str,
    opts: &VerifyAutoOptions,
) -> Vec<VerificationNote> {
    use crate::adapter::btor2::symbolic_bitblast::{
        ExactVerdict, exact_symbolic_verdict_with_witness,
    };
    use crate::mu_calculus::parser as mu_parser;

    let mut notes = Vec::new();
    // A.6: the exact engine is sound only under reset-gating. `opts.exact_symbolic`
    // means the exact main path already ran — nothing to rescue on a cube run here.
    if !opts.gate_reset || opts.exact_symbolic {
        return notes;
    }
    for prop in report.properties.iter_mut() {
        if !matches!(prop.outcome, VerifyOutcome::Skipped { .. }) {
            continue;
        }
        let Ok(formula) = mu_parser::parse(&prop.formula) else {
            continue;
        };
        match exact_symbolic_verdict_with_witness(design_btor2, &formula) {
            Ok((ExactVerdict::Holds, _)) => {
                prop.outcome = VerifyOutcome::Holds;
                prop.counterexample = None;
                notes.push(exact_skip_rescue_note(&prop.name, "HOLDS"));
            }
            Ok((ExactVerdict::Violated, witness)) => {
                prop.outcome = VerifyOutcome::Violated { false_cells: 1 };
                prop.counterexample = witness
                    .map(exact_counterexample_from_lasso)
                    .or_else(|| ef_unreachable_counterexample(&formula));
                notes.push(exact_skip_rescue_note(&prop.name, "VIOLATED"));
            }
            // Err (over cap / intractable design): leave the property Skipped.
            Err(_) => {}
        }
    }
    notes
}

/// Provenance note for a cube-Skipped property the exact engine rescued (lever b).
fn exact_skip_rescue_note(name: &str, verdict: &str) -> VerificationNote {
    VerificationNote {
        kind: "exact-skip-rescue".into(),
        level: NoteLevel::Info,
        summary: format!(
            "`{name}`: the predicate cube could not seed the property (no atoms); the \
             full-state exact-symbolic engine decided it {verdict}"
        ),
        detail: "An atom-less modal property (e.g. no-deadlock `nu X.(<> true && [] X)`) has \
                 no state-cell / combinational atom for the cube to build a dimension from, so \
                 the cube reports Skipped. The exact engine bit-blasts the design and decides \
                 it directly. Sound: the exact engine is the differential oracle — under \
                 reset-gating a definite verdict transfers to the RTL; the property is upgraded \
                 only on a definite Holds/Violated."
            .into(),
        items: Vec::new(),
    }
}

/// The engines that participated in a reachability-portfolio rescue (for a note).
fn engines_of(reach: &crate::adapter::reach_portfolio::ReachOutcome) -> Vec<&'static str> {
    reach
        .reachable_by
        .iter()
        .chain(reach.unreachable_by.iter())
        .copied()
        .collect()
}

/// A provenance note for a property the reachability portfolio rescued from `⊥`.
fn rescue_note(name: &str, verdict: &str, engines: &[&str]) -> VerificationNote {
    VerificationNote {
        kind: "portfolio-rescue".into(),
        level: NoteLevel::Info,
        summary: format!(
            "`{name}`: the cube abstraction left ⊥; the reachability portfolio decided it \
             {verdict} (engine(s): {})",
            if engines.is_empty() {
                "—".to_string()
            } else {
                engines.join(", ")
            }
        ),
        detail: "The safety ⊥ was reduced to a `bad`-monitor and decided symbolically by the \
                 multi-engine reachability portfolio (exact ⊕ native ⊕ spacer ⊕ btormc ⊕ Pono). \
                 Sound: every portfolio member is sound, and a disagreement would surface a \
                 contradiction alarm rather than a verdict."
            .into(),
        items: Vec::new(),
    }
}

/// A provenance note for a νμ recoverability property rescued from `⊥` — by the exact+COI
/// engine (reset-gated) if its cone fits the bit cap, else the scalable cube + smt-hyper-must
/// path.
fn recoverability_rescue_note(name: &str, verdict: &str) -> VerificationNote {
    VerificationNote {
        kind: "recoverability-rescue".into(),
        level: NoteLevel::Info,
        summary: format!(
            "`{name}`: the cube abstraction left ⊥; the νμ recoverability path (exact+COI \
             first, then cube + smt-hyper-must) decided it {verdict}"
        ),
        detail: "The AG EF (recoverability) ⊥ was escalated: exact+COI decides it directly when \
                 the property's cone fits the bit cap (a wishbone/status control signal whose \
                 cone excludes the datapath), else the sound predicate-cube + smt-hyper-must \
                 path. Definite νμ verdicts transfer (exact = oracle; cube per Bruns–Godefroid / \
                 Shoham–Grumberg); box-AF liveness and safety take their own reductions."
            .into(),
        items: Vec::new(),
    }
}

/// Actionable-⊥: an explanation of WHY a νμ recoverability property stayed ⊥ — the structural
/// obstacles in its recovery cone the predicate abstraction cannot cross (uncertified gating
/// counters; wide data/config state the recovery value rides). Purely diagnostic: it never changes
/// the ⊥ verdict, only makes it actionable. The `items` carry the machine-readable obstacle list.
fn recoverability_bot_reason_note(
    name: &str,
    diag: &crate::adapter::recoverability::RecoverabilityBotDiagnosis,
) -> VerificationNote {
    let mut items: Vec<String> = Vec::new();
    for (n, w) in &diag.uncertified_counters {
        items.push(format!("uncertified-counter: {n} ({w}-bit)"));
    }
    for (n, w) in &diag.wide_influences {
        items.push(format!("wide-influence: {n} ({w}-bit)"));
    }
    VerificationNote {
        kind: "recoverability-bot-reason".into(),
        // A localized obstacle narrows the verdict's scope (input-scoped / needs an assumption);
        // an empty diagnosis is purely informational.
        level: if diag.is_empty() {
            NoteLevel::Info
        } else {
            NoteLevel::ScopeCaveat
        },
        summary: format!("`{name}`: ⊥ because — {}", diag.hint()),
        detail: "This is a best-effort, sound explanation of the abstain (never a verdict): the \
                 structural obstacles in the recovery cone the predicate cube cannot cross. An \
                 un-certified counter needs a ranking certificate or a fairness assumption; a wide \
                 data/config influence means the recovery value is input-scoped (constrain those \
                 inputs to decide). No obstacle localized ⇒ the abstraction may be closable with \
                 additional predicates."
            .into(),
        items,
    }
}

/// A provenance note for a box-AF liveness property the l2s reduction + reachability
/// portfolio rescued from `⊥`.
fn liveness_rescue_note(name: &str, verdict: &str, engines: &[&str]) -> VerificationNote {
    VerificationNote {
        kind: "liveness-rescue".into(),
        level: NoteLevel::Info,
        summary: format!(
            "`{name}`: the cube abstraction left ⊥; the liveness-to-safety reduction + \
             reachability portfolio decided it {verdict} (engine(s): {})",
            if engines.is_empty() {
                "—".to_string()
            } else {
                engines.join(", ")
            }
        ),
        detail: "The box-response liveness ⊥ (`AG(a → AF b)`) was reduced to a reachable \
                 `b`-free-lasso `bad`-monitor (Biere–Artho–Schuppan liveness-to-safety) and \
                 decided symbolically by the reachability portfolio. Sound + complete for the \
                 box-AF shape; the diamond-EF recoverability response is not reduced here."
            .into(),
        items: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    /// A.4 — a `Violated` bare `EF p` (reachability) yields the unreachable-target
    /// witness: no path (`prefix`/`cycle` empty), the target atoms naming what the
    /// design never reaches. A recoverability (`AG EF`) shape is not a bare `EF` and
    /// keeps its concrete trap-path witness instead (no unreachable-target here).
    #[test]
    fn bare_ef_violation_yields_unreachable_target_witness() {
        use crate::mu_calculus::parser as mu_parser;
        let ef = mu_parser::parse("mu X. ((round_counter == 31) or <> X)").expect("EF parses");
        let cx = super::ef_unreachable_counterexample(&ef).expect("bare EF → unreachable witness");
        assert!(cx.prefix.is_empty() && cx.cycle.is_empty() && cx.inputs.is_empty());
        assert_eq!(cx.unreachable_target.len(), 1);
        assert!(cx.unreachable_target[0].contains("round_counter"));
        let agef = mu_parser::parse("nu Y. ((mu X. ((busy == 0) or <> X)) and [] Y)")
            .expect("AG EF parses");
        assert!(super::ef_unreachable_counterexample(&agef).is_none());
    }

    /// Ranking-wiring — a recoverability `AG EF <good>` routes to the scalable
    /// recoverability engine (ranking + cube), returning its definite verdict. `good`
    /// = `data == target` over a design where both increment from 0 (invariant → the
    /// relation always holds ⇒ recoverable ⇒ Holds); the never-equal variant ⇒ Violated.
    /// A non-recoverability shape (bare `EF`) does NOT route (`None`).
    #[test]
    fn ranking_rescue_routes_ag_ef_recoverability_verdict() {
        use crate::mu_calculus::parser as mu_parser;
        // data,target both init 0 and +1 each cycle ⇒ data==target INVARIANT ⇒ AG EF holds.
        let holds_btor2 = "1 sort bitvec 1\n2 sort bitvec 8\n4 state 2 data\n5 state 2 target\n\
             6 zero 2\n7 one 2\n13 init 2 4 6\n14 init 2 5 6\n15 add 2 4 7\n16 add 2 5 7\n\
             22 next 2 4 15\n23 next 2 5 16\n";
        // target inits 1 ≠ data(0), both +1 ⇒ never equal ⇒ AG EF (data==target) VIOLATED.
        let viol_btor2 = "1 sort bitvec 1\n2 sort bitvec 8\n4 state 2 data\n5 state 2 target\n\
             6 zero 2\n7 one 2\n13 init 2 4 6\n14 init 2 5 7\n15 add 2 4 7\n16 add 2 5 7\n\
             22 next 2 4 15\n23 next 2 5 16\n";
        let agef = mu_parser::parse("nu Y. ((mu X. ((data == target) or <> X)) and [] Y)")
            .expect("parses");
        assert_eq!(
            super::try_ranking_recoverability(holds_btor2, &agef),
            Some(VerifyOutcome::Holds),
            "invariant-relational recoverability routes to Holds via the recoverability engine",
        );
        assert_eq!(
            super::try_ranking_recoverability(viol_btor2, &agef),
            Some(VerifyOutcome::Violated { false_cells: 1 }),
            "never-equal relational target routes to Violated",
        );
        // A bare `EF` (not the recoverability νμ shape) does not route.
        let ef = mu_parser::parse("mu X. ((data == target) or <> X)").expect("EF parses");
        assert_eq!(super::try_ranking_recoverability(holds_btor2, &ef), None);
    }

    use super::*;
    use crate::mu_calculus::parser as mu_parser;

    fn cells(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn src(content: &str) -> Vec<(String, String)> {
        vec![("design.sv".to_string(), content.to_string())]
    }

    // ---- PORTFOLIO combiner (hermetic: no toolchain, pure over reports) --------------------

    /// A single-property report with the given name + outcome (+ optional counterexample).
    fn mk_report(props: &[(&str, VerifyOutcome, Option<ExactCounterexample>)]) -> AutoVerifyReport {
        AutoVerifyReport {
            properties: props
                .iter()
                .map(|(name, outcome, cx)| PropertyVerdict {
                    name: name.to_string(),
                    kind: crate::adapter::slang::translate::SvaKind::Assert,
                    formula: format!("formula::{name}"),
                    outcome: outcome.clone(),
                    seeded_predicates: Vec::new(),
                    counterexample: cx.clone(),
                })
                .collect(),
            ..Default::default()
        }
    }

    fn cx_stub() -> ExactCounterexample {
        ExactCounterexample {
            prefix: vec![vec![("st".to_string(), 0)]],
            cycle: vec![vec![("st".to_string(), 3)]],
            inputs: vec![vec![("esc".to_string(), 1)]],
            unreachable_target: Vec::new(),
        }
    }

    // T3 — the control-slice (cutpoint over-approximation) note must gate its
    // "definite HOLDS transfers (safety + over-approx = sound)" claim on the property
    // class: it holds for safety/universal properties but NOT for an alternating (νμ)
    // recoverability property, which collapses under a plain over-approximation. The
    // νμ caveat appears only when the report actually carries an alternating property.
    #[test]
    fn control_slice_note_gates_holds_transfer_claim_on_property_class() {
        use crate::adapter::slang::translate::SvaKind;
        let prop = |name: &str, formula: &str| PropertyVerdict {
            name: name.into(),
            kind: SvaKind::Assert,
            formula: formula.into(),
            outcome: VerifyOutcome::Holds,
            seeded_predicates: Vec::new(),
            counterexample: None,
        };
        let build = |report: &AutoVerifyReport| {
            build_notes(
                report,
                MustEdgeInference::Off,
                &[],
                &[],
                &["cnt".to_string()],
                &NotePosture::Exact,
            )
        };
        // νμ recoverability (`AG EF`, alternation_depth 2) under cutpoints ⇒ caveat present.
        let alternating = AutoVerifyReport {
            properties: vec![prop("recov", "nu Y. ((mu X. ((p == 1) || <> X)) && [] Y)")],
            ..Default::default()
        };
        let cs = build(&alternating);
        let note = cs
            .iter()
            .find(|n| n.kind == "control-slice")
            .expect("control-slice note present");
        assert!(
            note.detail.contains("ALTERNATING recoverability property"),
            "a νμ property under cutpoints must caveat that HOLDS is sound for the abstract model \
             only; got: {}",
            note.detail
        );

        // Only a safety property (alternation_depth 1) under cutpoints ⇒ no νμ caveat.
        let safety_only = AutoVerifyReport {
            properties: vec![prop("safe", "nu X. ((p == 1) && [] X)")],
            ..Default::default()
        };
        let cs2 = build(&safety_only);
        let note2 = cs2
            .iter()
            .find(|n| n.kind == "control-slice")
            .expect("control-slice note present");
        assert!(
            !note2.detail.contains("ALTERNATING recoverability property"),
            "a safety-only report must not carry the νμ caveat; got: {}",
            note2.detail
        );
    }

    // The safety-⊥ escalation reduces a reducible AG-invariant the cube left ⊥ to a
    // `bad`-monitor and decides it with the reachability portfolio — testable on a
    // BTOR2 fixture without the SV toolchain (exact/native/spacer run in-process).
    #[test]
    fn escalate_bottom_safety_rescues_a_reducible_invariant() {
        // `big` (4-bit) is init 0 and holds its value ⇒ AG(big == 0) HOLDS; the cube
        // is pretended to have left it ⊥.
        const DESIGN: &str =
            "1 sort bitvec 4\n2 state 1 big\n3 zero 1\n4 init 1 2 3\n5 next 1 2 2\n";
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "big_zero".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                formula: "nu X. ((big == 0) && [] X)".to_string(),
                outcome: VerifyOutcome::Unknown { unknown_cells: 1 },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };

        let notes = escalate_bottom(&mut report, DESIGN, false, &VerifyAutoOptions::default());
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Holds),
            "the ⊥ safety property should be rescued to HOLDS, got {:?}",
            report.properties[0].outcome
        );
        assert!(
            notes.iter().any(|n| n.kind == "portfolio-rescue"),
            "a portfolio-rescue provenance note should be recorded"
        );
    }

    // P3 Update 2 — a safety AG-invariant the cube left ⊥ that is actually VIOLATED is
    // rescued to Violated AND carries the concrete native-BMC counterexample: a wrapping
    // 2-bit counter `cnt` (0→1→2→3→0) violates `AG(cnt != 3)` at depth 3. The attached
    // witness is a finite path (empty cycle) over the ORIGINAL design register `cnt`,
    // ending in the violating state (cnt == 3).
    #[test]
    fn escalate_bottom_safety_attaches_violation_witness() {
        const COUNTER: &str = "1 sort bitvec 2\n2 sort bitvec 1\n3 zero 1\n4 one 1\n\
                               5 state 1 cnt\n6 init 1 5 3\n7 add 1 5 4\n8 next 1 5 7\n";
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "cnt_ne_3".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                formula: "nu X. ((cnt != 3) && [] X)".to_string(),
                outcome: VerifyOutcome::Unknown { unknown_cells: 1 },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };
        let notes = escalate_bottom(&mut report, COUNTER, false, &VerifyAutoOptions::default());
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Violated { .. }),
            "the ⊥ safety property should be rescued to VIOLATED, got {:?}",
            report.properties[0].outcome
        );
        let cx = report.properties[0]
            .counterexample
            .as_ref()
            .expect("a VIOLATED safety rescue must attach the native-BMC counterexample");
        assert!(
            !cx.prefix.is_empty(),
            "the safety CEX prefix (the finite Init→bad path) must be non-empty"
        );
        assert!(
            cx.cycle.is_empty(),
            "a safety counterexample is a finite path — no repeating cycle"
        );
        // The final state of the finite path satisfies the violation (cnt == 3).
        assert!(
            cx.prefix
                .last()
                .unwrap()
                .iter()
                .any(|(sig, v)| sig == "cnt" && *v == 3),
            "the witness's final state must satisfy the violated bad condition (cnt == 3); got {:?}",
            cx.prefix.last().unwrap()
        );
        assert!(
            notes.iter().any(|n| n.kind == "portfolio-rescue"),
            "a portfolio-rescue provenance note should be recorded"
        );
    }

    // Lever (b) — a cube-SKIPPED ATOM-LESS modal property (no-deadlock) is rescued to a
    // definite verdict by the full-state exact engine. The 2-bit counter increments every
    // cycle (0→1→2→3→0), so every state has a successor ⇒ `nu X.(<> true && [] X)` HOLDS.
    // The cube has no atom to seed (pure modal) so it reports Skipped; the exact rescue
    // upgrades it. Gated on reset-gating (`opts.gate_reset`, the exact engine's A.6 precond).
    #[test]
    fn rescue_skipped_via_exact_decides_atomless_no_deadlock() {
        const COUNTER: &str = "1 sort bitvec 2\n2 sort bitvec 1\n3 zero 1\n4 one 1\n\
                               5 state 1 cnt\n6 init 1 5 3\n7 add 1 5 4\n8 next 1 5 7\n";
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "no_deadlock".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                formula: "nu X.(<> true && [] X)".to_string(),
                outcome: VerifyOutcome::Skipped {
                    reason: "no state-cell / combinational predicate atoms to seed the cube"
                        .to_string(),
                },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };
        let opts = VerifyAutoOptions {
            gate_reset: true,
            exact_symbolic: false,
            ..Default::default()
        };
        let notes = rescue_skipped_via_exact(&mut report, COUNTER, &opts);
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Holds),
            "the cube-Skipped atom-less no-deadlock property should be rescued to HOLDS, got {:?}",
            report.properties[0].outcome
        );
        assert!(
            notes.iter().any(|n| n.kind == "exact-skip-rescue"),
            "an exact-skip-rescue provenance note should be recorded"
        );
    }

    // A.6 soundness gate — with reset-gating OFF the exact engine is unsound (it starts from
    // the illegal power-on default and does not fire the freed reset), so the rescue must NOT
    // touch the property: it stays Skipped, no note.
    #[test]
    fn rescue_skipped_via_exact_respects_reset_gate() {
        const COUNTER: &str = "1 sort bitvec 2\n2 sort bitvec 1\n3 zero 1\n4 one 1\n\
                               5 state 1 cnt\n6 init 1 5 3\n7 add 1 5 4\n8 next 1 5 7\n";
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "no_deadlock".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                formula: "nu X.(<> true && [] X)".to_string(),
                outcome: VerifyOutcome::Skipped {
                    reason: "no atoms".to_string(),
                },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };
        let opts = VerifyAutoOptions {
            gate_reset: false,
            ..Default::default()
        };
        let notes = rescue_skipped_via_exact(&mut report, COUNTER, &opts);
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Skipped { .. }),
            "with reset-gating off (A.6) the exact rescue must not fire, got {:?}",
            report.properties[0].outcome
        );
        assert!(
            notes.is_empty(),
            "no rescue note when the A.6 gate blocks the exact engine"
        );
    }

    // A liveness / recoverability ⊥ (diamond-bodied μ) is NOT a reducible AG-invariant, so
    // the SAFETY (reachability-portfolio) arm never touches it — property_class classifies
    // it Liveness, not Safety. With only the safety arm enabled (the recoverability arm that
    // WOULD legitimately decide this νμ is gated off), the router leaves it untouched: no
    // 2-valued reachability reduction is ever applied to a νμ.
    #[test]
    fn escalate_bottom_safety_ignores_non_safety_bottom() {
        const DESIGN: &str =
            "1 sort bitvec 4\n2 state 1 big\n3 zero 1\n4 init 1 2 3\n5 next 1 2 2\n";
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "recover".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                // AG EF (recoverability) — diamond inner, not an AG-invariant.
                formula: "nu Y. ((mu X. ((big == 0) || <> X)) && [] Y)".to_string(),
                outcome: VerifyOutcome::Unknown { unknown_cells: 1 },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };
        // Only the safety arm on — the νμ recoverability arm (which would decide this) is
        // gated off, isolating the property under test: the safety arm ignores a νμ ⊥.
        let opts = VerifyAutoOptions {
            rescue_bottom_liveness: false,
            rescue_bottom_recoverability: false,
            ..VerifyAutoOptions::default()
        };
        let notes = escalate_bottom(&mut report, DESIGN, false, &opts);
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Unknown { .. }),
            "a νμ ⊥ must be left untouched by the safety arm (2-valued reachability can't \
             soundly decide it)"
        );
        assert!(
            notes.is_empty(),
            "no rescue note for an un-reducible property"
        );
    }

    // 3-state responder: st 0=idle,1=req,2=grant; idle -go-> req; req -> grant; grant ->
    // idle. AG((st==1) → AF (st==2)) HOLDS — the l2s reduction + portfolio decides it.
    const RESPONDER_LIVE: &str = "\
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

    // A box-response `AG(a → AF b)` the cube left ⊥ is rescued to a definite verdict by
    // the liveness-to-safety reduction + reachability portfolio.
    #[test]
    fn escalate_bottom_liveness_rescues_a_box_response() {
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "req_grant".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                formula: "nu Z. ((!(st == 1) || mu Y. ((st == 2) || [] Y)) && [] Z)".to_string(),
                outcome: VerifyOutcome::Unknown { unknown_cells: 1 },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };
        let notes = escalate_bottom(
            &mut report,
            RESPONDER_LIVE,
            false,
            &VerifyAutoOptions::default(),
        );
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Holds),
            "the ⊥ box-AF liveness property should be rescued to HOLDS, got {:?}",
            report.properties[0].outcome
        );
        assert!(
            notes.iter().any(|n| n.kind == "liveness-rescue"),
            "a liveness-rescue provenance note should be recorded"
        );
    }

    // SOUNDNESS: a diamond-EF recoverability liveness (`AG(a → EF b)`, νμ) is NOT the
    // l2s-able box-AF shape, so the liveness escalation must leave it untouched — never
    // route a νμ property through the 2-valued l2s.
    #[test]
    fn escalate_bottom_liveness_ignores_diamond_ef() {
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "recover".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                formula: "nu Z. ((!(st == 1) || mu Y. ((st == 2) || <> Y)) && [] Z)".to_string(),
                outcome: VerifyOutcome::Unknown { unknown_cells: 1 },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };
        let notes = escalate_bottom(
            &mut report,
            RESPONDER_LIVE,
            false,
            &VerifyAutoOptions::default(),
        );
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Unknown { .. }),
            "a diamond-EF νμ ⊥ must be left untouched (l2s does not decide it)"
        );
        assert!(notes.is_empty(), "no rescue note for a non-box-AF property");
    }

    // A νμ recoverability property `AG EF (st == 0)` the cube left ⊥ is routed by
    // property_class → the Liveness arm → (l2s abstains) → the cube + smt-hyper-must νμ
    // path, which decides it on RESPONDER_LIVE (st==0 is reachable from every reachable
    // state: idle stays/returns, req→grant→idle, grant→idle).
    #[test]
    fn escalate_bottom_routes_recoverability_to_cube() {
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "recover_idle".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                formula: "nu Y. ((mu X. ((st == 0) || <> X)) && [] Y)".to_string(),
                outcome: VerifyOutcome::Unknown { unknown_cells: 1 },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };
        let notes = escalate_bottom(
            &mut report,
            RESPONDER_LIVE,
            false,
            &VerifyAutoOptions::default(),
        );
        // The router dispatches the νμ ⊥ to the cube + smt-hyper-must path, which now DECIDES it
        // `Holds`: the N1-first-increment property-directed seeding (good register's other control
        // states) splits the coarse abstraction so the recoverability is provable. (Between the
        // universal-◇ soundness fix and the seeding this abstained to a sound `Unknown`.) Soundness
        // invariant preserved throughout: agree with exact or abstain, never contradict.
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Holds),
            "the ⊥ νμ recoverability property must be rescued to Holds by the cube + smt-hyper-must \
             path (property-directed seeding), got {:?}",
            report.properties[0].outcome
        );
        assert!(
            notes.iter().any(|n| n.kind == "recoverability-rescue"),
            "a recoverability-rescue provenance note should be recorded"
        );
    }

    // ③a routing — under reset-gating, a ⊥ νμ recoverability property is routed through the
    // EXACT+COI engine (which decides control-cone recoverability the cube abstains on), not
    // only the scalable cube path. A 2-bit wrapping counter (0→1→2→3→0) satisfies
    // `AG EF (cnt == 0)` — every state returns to 0 — and exact decides it Holds.
    #[test]
    fn escalate_bottom_recoverability_uses_exact_when_reset_pinned() {
        const COUNTER: &str = "1 sort bitvec 2\n2 sort bitvec 1\n3 zero 1\n4 one 1\n\
                               5 state 1 cnt\n6 init 1 5 3\n7 add 1 5 4\n8 next 1 5 7\n";
        let mut report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "recover_to_zero".to_string(),
                kind: crate::adapter::slang::translate::SvaKind::Assert,
                formula: "nu Y.((mu X.(cnt == 0 || <> X)) && [] Y)".to_string(),
                outcome: VerifyOutcome::Unknown { unknown_cells: 1 },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            ..Default::default()
        };
        // reset_pinned = true → the ③a exact-first routing is enabled (A.6 sound).
        let notes = escalate_bottom(&mut report, COUNTER, true, &VerifyAutoOptions::default());
        assert!(
            matches!(report.properties[0].outcome, VerifyOutcome::Holds),
            "reset-pinned recoverability ⊥ must be rescued to Holds via exact+COI, got {:?}",
            report.properties[0].outcome
        );
        assert!(
            notes.iter().any(|n| n.kind == "recoverability-rescue"),
            "a recoverability-rescue provenance note should be recorded"
        );
    }

    /// A cube engine's ⊥ (Unknown) is filled by a later engine's definite verdict, and the
    /// portfolio note records which engine decided it.
    #[test]
    fn portfolio_merge_fills_bottom_with_a_definite() {
        let exact = (
            "exact-symbolic",
            Ok(mk_report(&[(
                "p",
                VerifyOutcome::Unknown { unknown_cells: 4 },
                None,
            )])),
        );
        let symbolic = (
            "symbolic",
            Ok(mk_report(&[("p", VerifyOutcome::Holds, None)])),
        );
        let runs = vec![exact, symbolic];
        let merged = merge_portfolio_reports(&runs, PortfolioMode::Parallel).expect("merge ok");
        assert_eq!(merged.properties[0].outcome, VerifyOutcome::Holds);
        assert!(
            merged
                .notes
                .iter()
                .any(|n| n.kind == "portfolio"
                    && n.items.iter().any(|i| i == "decided-by:symbolic=1")),
            "the note must attribute the verdict to the symbolic engine"
        );
    }

    /// When several engines agree on a definite, the FIRST (exact, highest-precision) engine's
    /// verdict AND its counterexample witness are retained.
    #[test]
    fn portfolio_merge_prefers_exact_witness_on_agreement() {
        let exact = (
            "exact-symbolic",
            Ok(mk_report(&[(
                "p",
                VerifyOutcome::Violated { false_cells: 1 },
                Some(cx_stub()),
            )])),
        );
        let symbolic = (
            "symbolic",
            Ok(mk_report(&[(
                "p",
                VerifyOutcome::Violated { false_cells: 9 },
                None,
            )])),
        );
        let runs = vec![exact, symbolic];
        let merged = merge_portfolio_reports(&runs, PortfolioMode::Sequential).expect("merge ok");
        // Exact's outcome (false_cells: 1) and its counterexample win.
        assert_eq!(
            merged.properties[0].outcome,
            VerifyOutcome::Violated { false_cells: 1 }
        );
        assert!(
            merged.properties[0].counterexample.is_some(),
            "exact's counterexample witness must be retained"
        );
    }

    /// The runtime soundness guard: OPPOSITE definite verdicts force ⊥ + a soundness-alarm note,
    /// never a silent pick. (The parity differential proves this never fires on the corpus.)
    #[test]
    fn portfolio_merge_contradiction_forces_bottom_and_alarms() {
        let exact = (
            "exact-symbolic",
            Ok(mk_report(&[("p", VerifyOutcome::Holds, None)])),
        );
        let symbolic = (
            "symbolic",
            Ok(mk_report(&[(
                "p",
                VerifyOutcome::Violated { false_cells: 1 },
                None,
            )])),
        );
        let runs = vec![exact, symbolic];
        let merged = merge_portfolio_reports(&runs, PortfolioMode::Parallel).expect("merge ok");
        assert!(
            matches!(merged.properties[0].outcome, VerifyOutcome::Unknown { .. }),
            "a contradiction must degrade to ⊥, not silently pick a verdict"
        );
        assert!(
            merged.properties[0].counterexample.is_none(),
            "a contradicted property carries no witness"
        );
        assert!(
            merged
                .notes
                .iter()
                .any(|n| n.kind == "portfolio-soundness-alarm"),
            "the contradiction must raise a soundness-alarm note"
        );
    }

    /// All engines ⊥: the merged ⊥ prefers an Unknown (abstraction attempted) over a Skipped
    /// (atom not cube-bindable) — the more informative cause.
    #[test]
    fn portfolio_merge_all_bottom_prefers_unknown_over_skipped() {
        let exact = (
            "exact-symbolic",
            Ok(mk_report(&[(
                "p",
                VerifyOutcome::Skipped {
                    reason: "bit cap".to_string(),
                },
                None,
            )])),
        );
        let symbolic = (
            "symbolic",
            Ok(mk_report(&[(
                "p",
                VerifyOutcome::Unknown { unknown_cells: 8 },
                None,
            )])),
        );
        let runs = vec![exact, symbolic];
        let merged = merge_portfolio_reports(&runs, PortfolioMode::Parallel).expect("merge ok");
        assert_eq!(
            merged.properties[0].outcome,
            VerifyOutcome::Unknown { unknown_cells: 8 }
        );
    }

    /// Every engine errored → the merge surfaces the first (highest-precision) error, not a panic.
    #[test]
    fn portfolio_merge_all_errors_returns_first_error() {
        let err = |m: &str| {
            Err(AdapterError {
                kind: AdapterErrorKind::StateSpaceOverflow,
                message: m.to_string(),
                location: None,
            })
        };
        let runs: Vec<(&str, Result<AutoVerifyReport, AdapterError>)> = vec![
            ("exact-symbolic", err("exact boom")),
            ("symbolic", err("sym boom")),
        ];
        let merged = merge_portfolio_reports(&runs, PortfolioMode::Sequential);
        assert!(merged.is_err());
        assert_eq!(merged.unwrap_err().message, "exact boom");
    }

    /// An errored engine contributes nothing; a surviving engine's definite still lands.
    #[test]
    fn portfolio_merge_tolerates_one_engine_error() {
        let exact = (
            "exact-symbolic",
            Err(AdapterError {
                kind: AdapterErrorKind::UnsupportedConstruct,
                message: "exact rejects free reset".to_string(),
                location: None,
            }),
        );
        let symbolic = (
            "symbolic",
            Ok(mk_report(&[("p", VerifyOutcome::Holds, None)])),
        );
        let runs = vec![exact, symbolic];
        let merged = merge_portfolio_reports(&runs, PortfolioMode::Parallel).expect("merge ok");
        assert_eq!(merged.properties[0].outcome, VerifyOutcome::Holds);
    }

    #[test]
    fn engine_selection_defaults_to_portfolio_sequential() {
        // THE DEFAULT (2026-07-06): an unspecified engine ⇒ portfolio-sequential.
        assert_eq!(
            engine_selection(None),
            (false, false, Some(PortfolioMode::Sequential))
        );
        // An unrecognised string also falls to the default (rather than silently explicit).
        assert_eq!(
            engine_selection(Some("nonsense")),
            (false, false, Some(PortfolioMode::Sequential))
        );
        // Each explicit selector maps to exactly its engine (case-insensitive).
        assert_eq!(engine_selection(Some("explicit")), (false, false, None));
        assert_eq!(engine_selection(Some("symbolic")), (true, false, None));
        assert_eq!(
            engine_selection(Some("exact-symbolic")),
            (false, true, None)
        );
        assert_eq!(
            engine_selection(Some("EXACT-SYMBOLIC")),
            (false, true, None)
        );
        assert_eq!(
            engine_selection(Some("portfolio-parallel")),
            (false, false, Some(PortfolioMode::Parallel))
        );
        assert_eq!(
            engine_selection(Some("portfolio-sequential")),
            (false, false, Some(PortfolioMode::Sequential))
        );
    }

    #[test]
    fn outcome_definite_maps_holds_violated_only() {
        assert_eq!(outcome_definite(&VerifyOutcome::Holds), Some(true));
        assert_eq!(
            outcome_definite(&VerifyOutcome::Violated { false_cells: 2 }),
            Some(false)
        );
        assert_eq!(
            outcome_definite(&VerifyOutcome::Unknown { unknown_cells: 1 }),
            None
        );
        assert_eq!(
            outcome_definite(&VerifyOutcome::Skipped {
                reason: "x".to_string()
            }),
            None
        );
    }

    /// H.5-GR1 — a `@mununu_guarantee` with a valid mu-calculus body is merged as
    /// a verifiable property whose formula is the body verbatim (the same string
    /// form the SVA translator emits and the per-property loop parses).
    #[test]
    fn h5_gr1_annotation_guarantee_merges_as_property() {
        let sv = "// @mununu_guarantee nu X. (p and [] X)\nmodule m(); endmodule";
        let scan = scan_annotation_properties(&src(sv));
        assert_eq!(scan.guarantees.len(), 1);
        assert_eq!(scan.guarantees[0].formula, "nu X. (p and [] X)");
        assert_eq!(scan.guarantees[0].kind, SvaKind::Assert);
        assert!(scan.skipped.is_empty());
        // The stored formula re-parses (it is the exact string the loop feeds mu-calc).
        assert!(mu_parser::parse(&scan.guarantees[0].formula).is_ok());
    }

    /// The box-`F` liveness the LTL translator's existential-`F` cannot express
    /// (`AG(active → AF done)`) is a plain mu-calculus body — it parses and merges.
    #[test]
    fn h5_gr1_annotation_carries_box_f_liveness() {
        let sv = r#"(* mununu_guarantee = "nu X. ((not active or mu Y. (done or [] Y)) and [] X)" *)
module uart_tx(); endmodule"#;
        let scan = scan_annotation_properties(&src(sv));
        assert_eq!(scan.guarantees.len(), 1);
        assert!(mu_parser::parse(&scan.guarantees[0].formula).is_ok());
    }

    /// A guarantee body that does not parse is recorded in `skipped`, never
    /// silently dropped, and never merged as a property.
    #[test]
    fn h5_gr1_unparsable_guarantee_is_skipped_not_dropped() {
        let sv = "// @mununu_guarantee this is )) not a formula\nmodule m(); endmodule";
        let scan = scan_annotation_properties(&src(sv));
        assert!(scan.guarantees.is_empty());
        assert_eq!(scan.skipped.len(), 1);
        assert!(scan.skipped[0].contains("did not parse"));
        // The provenance note surfaces the skip as a scope caveat.
        let note = annotation_note(&scan).expect("note present");
        assert_eq!(note.kind, "annotation-properties");
        assert!(matches!(note.level, NoteLevel::ScopeCaveat));
    }

    /// `@mununu_assume` bodies are recorded for provenance (not verified as a
    /// guarantee); a source with no `@mununu` property annotations yields no note.
    #[test]
    fn a6_exact_symbolic_with_free_reset_is_rejected() {
        // A.6 (2026-07-05) — `--engine exact-symbolic` + `--no-gate-reset` is an
        // unsound combination and MUST be rejected before any elaboration. The
        // reject fires right after the sources check, so it needs no slang (this
        // is a non-ignored unit test): the exact engine models the post-reset
        // state space and would emit a definite-looking but spurious VIOLATED on a
        // freed reset.
        let err = verify_auto(
            &src("module m(); endmodule"),
            &YosysOptions::default(),
            &VerifyAutoOptions {
                exact_symbolic: true,
                gate_reset: false,
                ..Default::default()
            },
        )
        .expect_err("exact-symbolic + no-gate-reset must be rejected");
        assert_eq!(err.kind, AdapterErrorKind::UnsupportedConstruct);
        assert!(
            err.message.contains("(A.6)") && err.message.contains("exact-symbolic"),
            "A.6 reject must name the unsound combination; got: {}",
            err.message
        );
    }

    #[test]
    fn h5_gr1_assume_recorded_and_empty_source_has_no_note() {
        let sv = "// @mununu_assume tick_baud_x16 = 1\nmodule m(); endmodule";
        let scan = scan_annotation_properties(&src(sv));
        assert!(scan.guarantees.is_empty());
        assert_eq!(scan.assumes, vec!["tick_baud_x16 = 1".to_string()]);
        assert!(annotation_note(&scan).is_some());

        let plain = scan_annotation_properties(&src("module m(); endmodule"));
        assert!(annotation_note(&plain).is_none());
    }

    #[test]
    fn scan_collects_predicate_hints_and_rejects_unparseable() {
        // W1 — `@mununu_predicate <expr>` hints are collected (literal, relational,
        // bound); a bare boolean signal is admitted; a malformed comparison is
        // surfaced in `skipped`, never silently dropped.
        let sv = "// @mununu_predicate c_state == 0\n\
                  // @mununu_predicate wptr == rptr\n\
                  // @mununu_predicate fill >= 30\n\
                  // @mununu_predicate ready\n\
                  // @mununu_predicate bogus ==\n\
                  module m(); endmodule";
        let scan = scan_annotation_properties(&src(sv));
        assert_eq!(
            scan.predicates,
            vec![
                "c_state == 0".to_string(),
                "wptr == rptr".to_string(),
                "fill >= 30".to_string(),
                "ready".to_string(),
            ]
        );
        assert!(
            scan.skipped.iter().any(|s| s.contains("bogus ==")),
            "malformed predicate surfaced in skipped: {:?}",
            scan.skipped
        );
    }

    /// R-F5.5d projection SOUNDNESS fix — an infeasible cube (absent from the
    /// symbolic feasible-cube tally) must project to ⊥, NOT a spurious
    /// definite-False. Regression for the unsound VIOLATED verify-auto returned
    /// on `AG AF` (the symbolic tally covers only feasible cubes; an infeasible
    /// `init_cube` was reading as `must=0,may=0` → `Trit::False` → VIOLATED).
    #[test]
    fn rf5_5d_symbolic_final_verdict_infeasible_cube_is_bottom_not_false() {
        use crate::adapter::btor2::symbolic_engine::{
            SymbolicCegarResult, SymbolicCegarTermination, SymbolicCubeVerdicts,
        };
        use crate::mu_calculus::trit::Trit;
        // 2 predicates → 4 cubes; only 0,1 are feasible + tallied. Cubes 2,3 are
        // infeasible and absent from the tally.
        let result = SymbolicCegarResult {
            iterations: Vec::new(),
            final_predicates: Vec::new(),
            final_verdicts: SymbolicCubeVerdicts {
                num_predicates: 2,
                cube_verdicts: vec![(0, Trit::Unknown), (1, Trit::True)],
                definite_true: 1,
                definite_false: 0,
                bottom: 1,
            },
            terminated_with: SymbolicCegarTermination::Converged,
        };
        let ts = symbolic_final_verdict(&result);
        // Feasible cubes keep their verdict.
        assert_eq!(ts.verdict_at(0), Trit::Unknown);
        assert_eq!(ts.verdict_at(1), Trit::True);
        // Infeasible (untallied) cubes are ⊥, never a spurious definite-False.
        assert_eq!(
            ts.verdict_at(2),
            Trit::Unknown,
            "infeasible cube 2 must project to ⊥, not False"
        );
        assert_eq!(
            ts.verdict_at(3),
            Trit::Unknown,
            "infeasible cube 3 must project to ⊥, not False"
        );
    }

    /// R-F5.5d — `symbolic_final_verdict` projects the symbolic engine's per-cube
    /// verdicts into a `TritSet` the verify_auto reset-cube read consumes, with
    /// identical cube indexing and the right length. Uses the symbolic CEGAR loop
    /// directly (no yosys), so it runs in `make ci`.
    #[test]
    fn rf5_5d_symbolic_final_verdict_projects_cube_verdicts() {
        use crate::adapter::AdapterOptions;
        use crate::adapter::btor2::PredicateSpec;
        use crate::adapter::btor2::symbolic_bitblast::MustSemantics;
        use crate::adapter::btor2::symbolic_engine::symbolic_cegar_refine;

        let btor2 = "\
1 sort bitvec 2
2 sort bitvec 1
3 state 1 cnt
4 input 2 en
5 one 1
6 ones 1
7 add 1 3 5
8 eq 2 3 6
9 ite 1 8 3 7
10 ite 1 4 9 3
11 next 1 3 10
";
        let options = AdapterOptions::default();
        let initial = vec![PredicateSpec {
            name: "p".to_string(),
            register: "cnt".to_string(),
            value: 0,
        }];
        let formula = mu_parser::parse("[] p").expect("formula"); // AG(cnt==0)
        let result = symbolic_cegar_refine(
            btor2,
            &initial,
            &options,
            &formula,
            MustSemantics::ForallExists,
            0,
        )
        .expect("symbolic refine");

        let ts = symbolic_final_verdict(&result);
        assert_eq!(ts.len(), 1usize << result.final_verdicts.num_predicates);
        for (cube, trit) in &result.final_verdicts.cube_verdicts {
            assert_eq!(ts.verdict_at(*cube), *trit, "cube {cube} verdict mismatch");
        }
    }

    #[test]
    fn substitute_config_in_formula_is_token_aware() {
        // H.J.b — a pinned config input is substituted with its constant, whole
        // identifiers only (so a prefix like `cfg_detect_timer` inside another
        // name is NOT touched), and the rest of the formula is preserved.
        let cfg = [("cfg_detect_timer_i".to_string(), 7u64)];
        assert_eq!(
            substitute_config_in_formula("nu X. ((cnt_q >= cfg_detect_timer_i) && [] X)", &cfg),
            "nu X. ((cnt_q >= 7) && [] X)"
        );
        // Whole-identifier: a name that merely CONTAINS the config name is not
        // substituted; a bound var / keyword is untouched.
        assert_eq!(
            substitute_config_in_formula("cfg_detect_timer_i_extra == 1", &cfg),
            "cfg_detect_timer_i_extra == 1"
        );
        // Empty config ⇒ unchanged.
        assert_eq!(
            substitute_config_in_formula("cnt_q >= cfg_detect_timer_i", &[]),
            "cnt_q >= cfg_detect_timer_i"
        );
        // The arithmetic-addend form survives + substitutes its own operands.
        let cfg2 = [("thr".to_string(), 3u64)];
        assert_eq!(
            substitute_config_in_formula("cnt == cnt_past + 1 && x >= thr", &cfg2),
            "cnt == cnt_past + 1 && x >= 3"
        );
    }

    #[test]
    fn counter_registers_detects_past_shadow() {
        // H.H — a state register whose `$past` shadow is also referenced is a
        // counter (the self-relational monotonicity/increment atom). A register
        // without its shadow present is NOT flagged.
        let mut seeded = Seeded::default();
        // sva_13 shape: `cnt_q >= cnt_q__past` (a CmpReg compound).
        seeded.compounds.push((
            "cnt_q >= cnt_q__past".to_string(),
            PredicateExpr::CmpReg {
                lhs: "cnt_q".into(),
                op: CmpOp::Ge,
                rhs: "cnt_q__past".into(),
            },
        ));
        // A plain state atom over an unrelated register — no shadow, not a counter.
        seeded.specs.push(PredicateSpec {
            name: "state_q == 0".into(),
            register: "state_q".into(),
            value: 0,
        });
        let counters = counter_registers_in(&seeded);
        assert_eq!(
            counters.into_iter().collect::<Vec<_>>(),
            vec!["cnt_q".to_string()]
        );
    }

    #[test]
    fn seed_counter_bounds_manual_overrides_config_and_dedups() {
        // H.H — manual bound wins over config-inferred; the bound becomes a `<= K`
        // compound on BOTH the counter and its `$past` shadow; a bound already
        // present as a compound is not duplicated.
        let counters: std::collections::BTreeSet<String> =
            ["cnt_q".to_string()].into_iter().collect();
        let manual: std::collections::HashMap<String, u64> =
            [("cnt_q".to_string(), 15u64)].into_iter().collect();
        let config: std::collections::HashMap<String, u64> =
            [("cnt_q".to_string(), 7u64)].into_iter().collect();

        // The monotonicity relational makes `cnt_q__past` a referenced register, so
        // the shadow is bounded alongside the counter.
        let mono = || {
            (
                "cnt_q >= cnt_q__past".to_string(),
                PredicateExpr::CmpReg {
                    lhs: "cnt_q".into(),
                    op: CmpOp::Ge,
                    rhs: "cnt_q__past".into(),
                },
            )
        };
        let mut seeded = Seeded::default();
        seeded.compounds.push(mono());
        let applied = seed_counter_bounds(&mut seeded, &counters, &manual, &config);
        // Manual 15 wins over config-inferred 7; the note reports the base once.
        assert_eq!(applied, vec![("cnt_q".to_string(), 15, "user-supplied")]);
        // BOTH operands of the relational are bounded.
        assert!(seeded.compounds.iter().any(|(n, _)| n == "cnt_q <= 15"));
        assert!(
            seeded
                .compounds
                .iter()
                .any(|(n, _)| n == "cnt_q__past <= 15")
        );

        // Re-seeding is idempotent (both `<= 15` compounds already exist).
        let again = seed_counter_bounds(&mut seeded, &counters, &manual, &config);
        assert!(again.is_empty());
        assert_eq!(
            seeded
                .compounds
                .iter()
                .filter(|(n, _)| n == "cnt_q <= 15")
                .count(),
            1
        );

        // With no manual entry, the config-inferred bound is used.
        let mut seeded2 = Seeded::default();
        seeded2.compounds.push(mono());
        let applied2 = seed_counter_bounds(
            &mut seeded2,
            &counters,
            &std::collections::HashMap::new(),
            &config,
        );
        assert_eq!(applied2, vec![("cnt_q".to_string(), 7, "config-inferred")]);
        assert!(
            seeded2
                .compounds
                .iter()
                .any(|(n, _)| n == "cnt_q__past <= 7")
        );
    }

    #[test]
    fn config_inferred_bounds_from_sibling_comparison() {
        // H.H — the bound for the counter comes from a SIBLING property's
        // comparison against a pinned config threshold (the monotonicity property
        // itself never names the config). Global scan across the property set.
        let translated = vec![
            crate::adapter::slang::translate::TranslatedAssertion {
                name: "m_sva_5".into(),
                kind: SvaKind::Assert,
                // `cnt_q >= cfg_detect_timer_i` — the comparison that reveals the bound.
                formula: "nu X. ((cnt_q >= cfg_detect_timer_i) && [] X)".into(),
                recoverability_companion: None,
            },
            crate::adapter::slang::translate::TranslatedAssertion {
                name: "m_sva_13".into(),
                kind: SvaKind::Assert,
                // The monotonicity property — needs the bound but never names cfg.
                formula: "nu X. ((cnt_q >= cnt_q__past) && [] X)".into(),
                recoverability_companion: None,
            },
        ];
        let bounds =
            config_inferred_counter_bounds(&translated, &[("cfg_detect_timer_i".into(), 7)]);
        assert_eq!(bounds.get("cnt_q"), Some(&7));
        // No config pinned ⇒ no inferred bounds.
        assert!(config_inferred_counter_bounds(&translated, &[]).is_empty());
    }

    #[test]
    fn build_notes_surfaces_every_decision() {
        // H.J.a — the provenance notes cover the coverage tally, the abstraction
        // posture, and each model-level decision (reset-gating, flop stubs, cut
        // modules), plus a config-concretization ScopeCaveat when pins are given.
        let report = AutoVerifyReport {
            properties: vec![
                PropertyVerdict {
                    name: "p_holds".into(),
                    kind: SvaKind::Assert,
                    formula: "nu X. (a && [] X)".into(),
                    outcome: VerifyOutcome::Holds,
                    seeded_predicates: vec!["a".into()],
                    counterexample: None,
                },
                PropertyVerdict {
                    name: "p_unknown".into(),
                    kind: SvaKind::Assert,
                    formula: "nu X. (b && [] X)".into(),
                    outcome: VerifyOutcome::Unknown { unknown_cells: 2 },
                    seeded_predicates: vec!["b".into()],
                    counterexample: None,
                },
            ],
            unsupported: vec![("u".into(), "reason".into())],
            diagnostics: ModelDiagnostics {
                state_register_count: 3,
                blackboxed_modules: vec!["prim_sparse_fsm_flop".into()],
                gated_resets: vec!["rst_ni=1".into()],
                auto_provided_stubs: vec!["prim_flop".into()],
            },
            notes: Vec::new(),
        };
        let notes = build_notes(
            &report,
            MustEdgeInference::SmtHyperMust,
            &[("cfg_detect_timer_i".into(), 7)],
            &["cnt_q <= 7 (config-inferred)".to_string()],
            &[],
            &NotePosture::Cube,
        );
        let kinds: Vec<&str> = notes.iter().map(|n| n.kind.as_str()).collect();
        for expected in [
            "coverage-summary",
            "config-concretization",
            "counter-bound",
            "abstraction-posture",
            "reset-gating",
            "flop-stub",
            "blackbox-cut",
        ] {
            assert!(
                kinds.contains(&expected),
                "missing note {expected}; got {kinds:?}"
            );
        }
        // The counter-bound note is Info (a HOLDS it unlocks is proven) + carries
        // the seeded bound.
        let cb = notes.iter().find(|n| n.kind == "counter-bound").unwrap();
        assert_eq!(cb.level, NoteLevel::Info);
        assert!(
            cb.items
                .contains(&"cnt_q <= 7 (config-inferred)".to_string())
        );
        // The scope caveat carries the pinned value + is a ScopeCaveat.
        let cc = notes
            .iter()
            .find(|n| n.kind == "config-concretization")
            .unwrap();
        assert_eq!(cc.level, NoteLevel::ScopeCaveat);
        assert!(cc.items.contains(&"cfg_detect_timer_i=7".to_string()));
        // The cut module is a SoundnessCaveat; coverage tallies 1 HOLDS + 1 ⊥.
        let bc = notes.iter().find(|n| n.kind == "blackbox-cut").unwrap();
        assert_eq!(bc.level, NoteLevel::SoundnessCaveat);
        let cov = notes.iter().find(|n| n.kind == "coverage-summary").unwrap();
        assert!(cov.summary.contains("1 definite") && cov.summary.contains("1 unknown"));
        // No pins / no bounds ⇒ no config-concretization or counter-bound note.
        // With an all-`SmtAllPairs` (empty sampling) cube posture, the note is the
        // sound over-approximation Info.
        let ap = notes
            .iter()
            .find(|n| n.kind == "abstraction-posture")
            .unwrap();
        assert_eq!(ap.level, NoteLevel::Info);
        assert!(
            ap.summary.contains("may-over-approximation"),
            "{}",
            ap.summary
        );
        let notes_no_pins = build_notes(
            &report,
            MustEdgeInference::Off,
            &[],
            &[],
            &[],
            &NotePosture::Cube,
        );
        assert!(
            !notes_no_pins
                .iter()
                .any(|n| n.kind == "config-concretization"
                    || n.kind == "counter-bound"
                    || n.kind == "control-slice")
        );
    }

    #[test]
    fn control_slice_note_present_when_cutpoints_given() {
        // The control-slice note is a ScopeCaveat present iff nets were cut, carrying
        // the cut list and the over-approximation posture. It documents that a HOLDS
        // is sound but a VIOLATED is sound only when guard-independent (orphan case).
        let report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "p".into(),
                kind: SvaKind::Assert,
                formula: "nu X. (a && [] X)".into(),
                outcome: VerifyOutcome::Holds,
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            unsupported: Vec::new(),
            diagnostics: ModelDiagnostics {
                state_register_count: 1,
                blackboxed_modules: Vec::new(),
                gated_resets: Vec::new(),
                auto_provided_stubs: Vec::new(),
            },
            notes: Vec::new(),
        };
        let notes = build_notes(
            &report,
            MustEdgeInference::SmtHyperMust,
            &[],
            &[],
            &["must_refresh".to_string(), "precharge_done".to_string()],
            &NotePosture::Exact,
        );
        let cs = notes
            .iter()
            .find(|n| n.kind == "control-slice")
            .expect("control-slice note present when cut points given");
        assert_eq!(cs.level, NoteLevel::ScopeCaveat);
        assert!(cs.items.contains(&"must_refresh".to_string()));
        assert!(cs.summary.contains("over-approximation"), "{}", cs.summary);
        // No cut points ⇒ no control-slice note.
        let none = build_notes(
            &report,
            MustEdgeInference::Off,
            &[],
            &[],
            &[],
            &NotePosture::Exact,
        );
        assert!(!none.iter().any(|n| n.kind == "control-slice"));
    }

    #[test]
    fn abstraction_posture_note_reflects_may_relation() {
        // A.3 — the abstraction-posture note is honest about the may-relation the
        // run actually used, rather than hardcoding "may-over-approximation".
        let report = AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: "recover".into(),
                kind: SvaKind::Assert,
                formula: "nu Y. ((mu X. (idle || <> X)) && [] Y)".into(),
                outcome: VerifyOutcome::Holds,
                seeded_predicates: vec!["idle".into()],
                counterexample: None,
            }],
            unsupported: Vec::new(),
            diagnostics: ModelDiagnostics {
                state_register_count: 1,
                blackboxed_modules: Vec::new(),
                gated_resets: Vec::new(),
                auto_provided_stubs: Vec::new(),
            },
            notes: Vec::new(),
        };
        let ap = |posture: &NotePosture| {
            build_notes(
                &report,
                MustEdgeInference::SmtHyperMust,
                &[],
                &[],
                &[],
                posture,
            )
            .into_iter()
            .find(|n| n.kind == "abstraction-posture")
            .unwrap()
        };
        // AR-S2 — the cube path always uses the sound `SmtAllPairs` may-relation,
        // so the cube posture note is the sound over-approximation Info (the
        // sampling-may SoundnessCaveat + its A.4 ⊥-guard were retired).
        let sound = ap(&NotePosture::Cube);
        assert_eq!(sound.level, NoteLevel::Info);
        assert!(sound.summary.contains("may-over-approximation"));
        // Exact engine → Info, and it does not claim a may-over-approximation.
        let exact = ap(&NotePosture::Exact);
        assert_eq!(exact.level, NoteLevel::Info);
        assert!(exact.summary.contains("Exact"), "{}", exact.summary);
        assert!(!exact.summary.contains("may-over-approximation"));
    }

    #[test]
    fn seeds_simple_state_predicate() {
        // `nu X. ((state == 5) && [] X)` over a state cell → one simple spec.
        let f = mu_parser::parse("nu X. ((state == 5) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["state"]).contains(n),
            |_| false,
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert_eq!(s.specs.len(), 1, "one simple reg==val spec");
        assert_eq!(s.specs[0].name, "state == 5");
        assert_eq!(s.specs[0].register, "state");
        assert_eq!(s.specs[0].value, 5);
        assert!(s.compounds.is_empty());
        assert!(s.unseedable.is_empty());
    }

    #[test]
    fn extra_atoms_seed_the_cube_like_formula_atoms() {
        // W1 (`@mununu_predicate`): an auxiliary predicate `aux == 3` that is NOT in
        // the formula seeds a cube dimension exactly as a formula atom would — a
        // simple state spec, plus a relational one as a compound. Dedups against the
        // formula atom.
        let f = mu_parser::parse("mu X. ((target == 1) || <> X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["target", "aux", "a", "b"]).contains(n),
            |_| false,
            |_| None,
            |_| Vec::new(),
            &[
                "aux == 3".to_string(),
                "a == b".to_string(),
                "target == 1".to_string(), // duplicate of the formula atom → deduped
            ],
        );
        assert!(
            s.specs.iter().any(|p| p.name == "target == 1"),
            "formula atom still seeded"
        );
        assert!(
            s.specs.iter().any(|p| p.name == "aux == 3"),
            "extra simple atom seeded as a spec"
        );
        assert!(
            s.compounds.iter().any(|(n, _)| n == "a == b"),
            "extra relational atom seeded as a compound"
        );
        assert_eq!(
            s.specs.iter().filter(|p| p.name == "target == 1").count(),
            1,
            "duplicate extra atom is deduped against the formula atom"
        );
    }

    #[test]
    fn extra_atoms_bound_predicate_routes_to_compound() {
        // W2 (relational/bound grammar rides the SAME annotation path): a bound
        // `cnt >= 5` over a state cell seeds a compound dimension (not a literal spec).
        let f = mu_parser::parse("mu X. ((done == 1) || <> X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["done", "cnt"]).contains(n),
            |_| false,
            |_| None,
            |_| Vec::new(),
            &["cnt >= 5".to_string()],
        );
        assert!(
            s.compounds.iter().any(|(n, _)| n == "cnt >= 5"),
            "bound hint seeded as a compound dimension"
        );
        assert!(s.unseedable.is_empty(), "a state bound is seedable");
    }

    #[test]
    fn extra_atoms_empty_is_the_baseline() {
        // No `@mununu_predicate` hints ⇒ identical to the pre-W1 behaviour.
        let f = mu_parser::parse("nu X. ((state == 5) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["state"]).contains(n),
            |_| false,
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert_eq!(s.specs.len(), 1);
        assert_eq!(s.specs[0].name, "state == 5");
    }

    #[test]
    fn seeds_relational_predicate_as_compound() {
        // `$stable` shape: `state == state__past` → a compound (REL), both state cells.
        let f = mu_parser::parse("nu X. ((state == state__past) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["state", "state__past"]).contains(n),
            |_| false,
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert!(s.specs.is_empty(), "relational atom is not a simple spec");
        assert_eq!(s.compounds.len(), 1, "one compound (relational) predicate");
        assert_eq!(s.compounds[0].0, "state == state__past");
        assert!(s.unseedable.is_empty());
        // It serialises into a sidecar cegar can read (compound + init pins).
        let referenced: std::collections::BTreeSet<String> =
            ["state".to_string(), "state__past".to_string()]
                .into_iter()
                .collect();
        let inits: std::collections::HashMap<String, u64> =
            [("state".to_string(), 0), ("state__past".to_string(), 0)]
                .into_iter()
                .collect();
        let json =
            synth_sidecar_json(&s.compounds, &[], &[], &referenced, &inits).expect("sidecar json");
        assert!(json.contains("compound_predicates"));
        assert!(json.contains("state == state__past"));
        assert!(json.contains("config_values"), "init pins present: {json}");
    }

    #[test]
    fn gates_atoms_over_non_state_signals() {
        // `gnt_o != 0` and `ready_i` over IO signals (not state cells) → unseedable.
        let f = mu_parser::parse("nu X. (((gnt_o != 0) || ready_i) && [] X)").unwrap();
        // Neither IO signal is a state cell, and (here) none is a free input
        // either → both unseedable.
        let s = seed_from_formula(
            &f,
            |n| cells(&["state_q"]).contains(n),
            |_| false,
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert!(
            s.unseedable.contains(&"gnt_o != 0".to_string()),
            "IO comparison atom must be unseedable; got {:?}",
            s.unseedable
        );
        assert!(
            s.unseedable.contains(&"ready_i".to_string()),
            "bare IO boolean atom must be unseedable; got {:?}",
            s.unseedable
        );
    }

    #[test]
    fn bare_one_bit_state_signal_seeds_eq_one() {
        let f = mu_parser::parse("nu X. (busy && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["busy"]).contains(n),
            |_| false,
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert_eq!(s.specs.len(), 1);
        assert_eq!(s.specs[0].name, "busy");
        assert_eq!(s.specs[0].value, 1, "bare boolean state signal ≡ sig == 1");
        assert!(s.input_registers.is_empty(), "no free inputs here");
    }

    #[test]
    fn h_b_admits_simple_input_atom_as_free_dimension() {
        // sysrst shape: `!cfg_enable_i |=> state == Idle` translates (after the
        // disable-iff / |=> lowering) to a formula referencing the INPUT
        // `cfg_enable_i` and the STATE `state`. Here we drive the seeder with
        // `state` as a state cell and `cfg_enable_i` as a free input.
        let f = mu_parser::parse("nu X. ((cfg_enable_i == 0) && ((state == 1) && [] X))").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["state"]).contains(n),
            |n| cells(&["cfg_enable_i"]).contains(n),
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert!(
            s.unseedable.is_empty(),
            "both atoms seed: {:?}",
            s.unseedable
        );
        assert_eq!(s.specs.len(), 2, "one state + one input simple spec");
        assert!(
            s.input_registers.contains("cfg_enable_i"),
            "the input register is recorded as a free dimension: {:?}",
            s.input_registers
        );
        assert!(
            !s.input_registers.contains("state"),
            "the state register is NOT a free dimension"
        );
    }

    #[test]
    fn h_b_bare_input_boolean_seeds_eq_one_as_free() {
        let f = mu_parser::parse("nu X. (trigger_i && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |_| false,
            |n| cells(&["trigger_i"]).contains(n),
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert_eq!(s.specs.len(), 1);
        assert_eq!(s.specs[0].name, "trigger_i");
        assert_eq!(s.specs[0].value, 1, "bare boolean input ≡ sig == 1");
        assert!(s.input_registers.contains("trigger_i"));
    }

    #[test]
    fn free_input_init_cubes_no_free_bits_is_single_base() {
        // No free inputs → exactly the base reset cube (pre-H.B behaviour).
        assert_eq!(free_input_init_cubes(0b101, &[]), vec![0b101]);
    }

    #[test]
    fn free_input_init_cubes_enumerates_every_flavour() {
        // base = bit2 set (a pinned state predicate); free input dims at bits 0
        // and 1 → 4 flavours, each keeping bit2 and toggling {0,1}.
        let cubes = free_input_init_cubes(0b100, &[0, 1]);
        let got: std::collections::HashSet<usize> = cubes.into_iter().collect();
        let want: std::collections::HashSet<usize> =
            [0b100, 0b101, 0b110, 0b111].into_iter().collect();
        assert_eq!(
            got, want,
            "all 2^|free| initial input flavours, base preserved"
        );
    }

    #[test]
    fn h_f_input_inside_compound_binds_as_derived_relational() {
        // H.F supersedes the H.B deferral: an input inside a compound (`!=`,
        // relational, boolean) is NO LONGER unseedable. `cfg_enable_i != 0` (a Ne
        // over an input, routed to the compound branch) now binds as a derived
        // per-cube 3-valued label (sound; ⊥ where the input swings it).
        let f = mu_parser::parse("nu X. ((cfg_enable_i != 0) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |_| false,
            |n| cells(&["cfg_enable_i"]).contains(n),
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert!(s.specs.is_empty());
        assert!(
            s.compounds.is_empty(),
            "not a cube dimension (input operand)"
        );
        assert!(
            s.derived_relational
                .iter()
                .any(|(n, _)| n == "cfg_enable_i != 0"),
            "input inside a compound now binds as a derived relational label: {:?}",
            s.derived_relational
        );
        assert!(
            !s.unseedable.iter().any(|a| a.contains("cfg_enable_i")),
            "no longer unseedable: {:?}",
            s.unseedable
        );
    }

    #[test]
    fn h_u_admits_combinational_bare_atom_as_cube_dim() {
        // csrng `main_sm_err_o` — a combinational-of-state output — is admitted as
        // a CUBE DIMENSION (H.U.2: the uniform image resolves it via the
        // combinational nid-map + reads its term over `(s,i)` / `(s',i')`), NOT a
        // derived per-cube label.
        let f = mu_parser::parse("nu X. (main_sm_err_o && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |_| false,
            |_| false,
            |n| {
                cells(&["main_sm_err_o"])
                    .contains(n)
                    .then_some(CombKind::StateOnly)
            },
            |_| Vec::new(),
            &[],
        );
        assert_eq!(
            s.specs.len(),
            1,
            "combinational-of-state is a cube dimension"
        );
        assert_eq!(s.specs[0].name, "main_sm_err_o");
        assert_eq!(s.specs[0].value, 1, "bare boolean ≡ == 1");
        assert!(s.compounds.is_empty());
        assert!(s.unseedable.is_empty(), "not skipped: {:?}", s.unseedable);
    }

    #[test]
    fn h_u_admits_combinational_eq_value_as_cube_dim() {
        let f = mu_parser::parse("nu X. ((err_code == 3) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |_| false,
            |_| false,
            |n| {
                cells(&["err_code"])
                    .contains(n)
                    .then_some(CombKind::StateOnly)
            },
            |_| Vec::new(),
            &[],
        );
        assert_eq!(s.specs.len(), 1);
        assert_eq!(s.specs[0].register, "err_code");
        assert_eq!(s.specs[0].value, 3);
        assert!(s.compounds.is_empty() && s.unseedable.is_empty());
    }

    #[test]
    fn h_u_state_takes_precedence_over_combinational() {
        // A name classified as BOTH state and combinational binds as STATE
        // (`is_state` is checked first) — still a cube dimension either way.
        let f = mu_parser::parse("nu X. (sig && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["sig"]).contains(n),
            |_| false,
            |n| cells(&["sig"]).contains(n).then_some(CombKind::StateOnly),
            |_| Vec::new(),
            &[],
        );
        assert_eq!(s.specs.len(), 1, "a cube dimension");
    }

    #[test]
    fn h_e_r2_input_dependent_combinational_routed_to_derived_label() {
        // `trigger_active = !trigger_i` — combinational of a FREE INPUT. H.E.r2
        // routes it to a DERIVED 3-valued label, NOT a cube dimension and NOT a
        // free dimension (both unsound for the must relation). The per-cube
        // labeller decides KleeneT/F where the cube pins it, KleeneBot where the
        // free input swings it — sound, and never a spurious VIOLATED.
        let f = mu_parser::parse("nu X. ((trigger_active && (state == 2)) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["state"]).contains(n),
            |_| false,
            |n| {
                cells(&["trigger_active"])
                    .contains(n)
                    .then_some(CombKind::InputDependent)
            },
            |_| Vec::new(),
            &[],
        );
        assert!(
            !s.input_registers.contains("trigger_active"),
            "NOT routed as a free dimension (unsound for must)"
        );
        assert!(
            !s.specs.iter().any(|p| p.register == "trigger_active"),
            "not a cube dimension"
        );
        assert!(
            !s.unseedable.contains(&"trigger_active".to_string()),
            "no longer skipped: {:?}",
            s.unseedable
        );
        assert!(
            s.derived.iter().any(|p| p.register == "trigger_active"),
            "input-dependent combinational is routed to the derived ⊥-label: {:?}",
            s.derived
        );
    }

    #[test]
    fn slice3_combinational_of_input_seeds_its_cone_input_as_free_dimension() {
        // Slice 3 (the unwrap lever) — a combinational-of-input atom
        // (`trigger_active = !trigger_i`, one cone input) binds as a derived
        // ⊥-label AND seeds its cone raw input (`trigger_i`) as a free H.B cube
        // dimension. That refines the may-relation (so a consequent box over a
        // trigger-governed transition becomes definite) and makes the derived
        // label definite at cubes that pin `trigger_i`.
        let f = mu_parser::parse("nu X. (trigger_active && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |_| false, // no state
            |_| false, // trigger_active is a combinational, not a raw input
            |n| (n == "trigger_active").then_some(CombKind::InputDependent),
            |n| {
                if n == "trigger_active" {
                    vec!["trigger_i".to_string()]
                } else {
                    Vec::new()
                }
            },
            &[],
        );
        assert!(
            s.derived.iter().any(|p| p.register == "trigger_active"),
            "combinational stays a derived label (binds the formula atom): {:?}",
            s.derived
        );
        assert!(
            s.input_registers.contains("trigger_i"),
            "the cone raw input is seeded as a free cube dimension: {:?}",
            s.input_registers
        );
        assert!(
            s.specs
                .iter()
                .any(|p| p.register == "trigger_i" && p.value == 1),
            "trigger_i is a cube dimension: {:?}",
            s.specs
        );
    }

    #[test]
    fn slice3_many_cone_inputs_left_as_bare_derived_label() {
        // The cube-blowup guard: a combinational reaching MANY (> MAX_CONE_INPUTS)
        // raw inputs is NOT unwrapped — it stays a plain derived ⊥-label with no
        // added dimensions, so the abstraction does not blow up.
        let f = mu_parser::parse("nu X. (wide && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |_| false,
            |_| false,
            |n| (n == "wide").then_some(CombKind::InputDependent),
            |n| {
                if n == "wide" {
                    vec!["i0".to_string(), "i1".to_string(), "i2".to_string()]
                } else {
                    Vec::new()
                }
            },
            &[],
        );
        assert!(
            s.derived.iter().any(|p| p.register == "wide"),
            "wide stays a derived label: {:?}",
            s.derived
        );
        assert!(
            s.input_registers.is_empty(),
            "too many cone inputs → none seeded (blowup guard): {:?}",
            s.input_registers
        );
        assert!(
            s.specs.is_empty(),
            "no cube dimensions added: {:?}",
            s.specs
        );
    }

    #[test]
    fn h_f_relational_with_input_operand_routed_to_derived_relational() {
        // `cnt_q >= cfg_detect_timer_i` — `cnt_q` state, `cfg_detect_timer_i` a
        // free input. The relational's value depends on the demonic input, so it
        // is NOT a sound cube dimension; H.F routes it to a derived per-cube
        // 3-valued label (carrying its PredicateExpr), not unseedable.
        let f = mu_parser::parse("nu X. ((cnt_q >= cfg_detect_timer_i) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["cnt_q"]).contains(n),
            |n| n == "cfg_detect_timer_i",
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert!(
            s.compounds.is_empty(),
            "not a cube dimension (has a non-state operand): {:?}",
            s.compounds
        );
        assert!(
            s.derived_relational
                .iter()
                .any(|(n, _)| n == "cnt_q >= cfg_detect_timer_i"),
            "relational-with-input routed to derived_relational: {:?}",
            s.derived_relational
        );
        assert!(
            !s.unseedable.iter().any(|a| a.contains("cnt_q")),
            "no longer skipped: {:?}",
            s.unseedable
        );
    }

    #[test]
    fn h_f_relational_with_unresolvable_operand_is_skipped() {
        // An operand that resolves to nothing in the design (not state / input /
        // combinational) → cannot label it → honest SKIP, not a derived label.
        let f = mu_parser::parse("nu X. ((cnt_q >= mystery_signal) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |n| cells(&["cnt_q"]).contains(n),
            |_| false,
            |_| None,
            |_| Vec::new(),
            &[],
        );
        assert!(
            s.derived_relational.is_empty(),
            "{:?}",
            s.derived_relational
        );
        assert!(
            s.unseedable.iter().any(|a| a.contains("mystery_signal")),
            "unresolvable operand ⇒ SKIP: {:?}",
            s.unseedable
        );
    }

    #[test]
    fn skip_reason_enriched_with_blackboxed_module_root_cause() {
        // A cut FSM (e.g. csrng's prim_sparse_fsm_flop) → the bare "non-state
        // signal" symptom is augmented with the actionable root cause.
        let diag = ModelDiagnostics {
            state_register_count: 0,
            blackboxed_modules: vec!["prim_sparse_fsm_flop".to_string()],
            gated_resets: Vec::new(),
            auto_provided_stubs: Vec::new(),
        };
        let reason = unseedable_skip_reason(&["state_q == 41".to_string()], &diag);
        assert!(reason.contains("state_q == 41"), "carries the symptom");
        assert!(
            reason.contains("prim_sparse_fsm_flop"),
            "names the cut module: {reason}"
        );
        assert!(
            reason.contains("Provide the missing module source"),
            "actionable: {reason}"
        );
    }

    #[test]
    fn skip_reason_enriched_when_no_state_registers() {
        let diag = ModelDiagnostics {
            state_register_count: 0,
            blackboxed_modules: Vec::new(),
            gated_resets: Vec::new(),
            auto_provided_stubs: Vec::new(),
        };
        let reason = unseedable_skip_reason(&["foo == 1".to_string()], &diag);
        assert!(
            reason.contains("no state registers"),
            "root cause: {reason}"
        );
    }

    #[test]
    fn skip_reason_bare_when_model_has_state() {
        // Genuine IO atom on a healthy model → no misleading root-cause hint.
        let diag = ModelDiagnostics {
            state_register_count: 3,
            blackboxed_modules: Vec::new(),
            gated_resets: Vec::new(),
            auto_provided_stubs: Vec::new(),
        };
        let reason = unseedable_skip_reason(&["gnt_o != 0".to_string()], &diag);
        assert!(reason.contains("gnt_o != 0"));
        assert!(!reason.contains("Root cause"), "no spurious hint: {reason}");
    }

    #[test]
    fn no_sidecar_when_nothing_to_emit() {
        let empty_refs = std::collections::BTreeSet::new();
        let empty_inits = std::collections::HashMap::new();
        assert!(synth_sidecar_json(&[], &[], &[], &empty_refs, &empty_inits).is_none());
    }

    #[test]
    fn empty_sources_errors() {
        let err = verify_auto(&[], &YosysOptions::default(), &VerifyAutoOptions::default())
            .expect_err("no sources");
        assert_eq!(err.kind, AdapterErrorKind::ParseError);
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3; run with --ignored"]
    fn e2e_reset_handled_by_gating_or_as_free_input() {
        // A 2-bit FSM cycling 0→1→2→0, with an active-low-reset-guarded
        // assertion `disable iff (!rst_n) state != 3`. Two SOUND paths to a
        // verdict, exercised here:
        //
        //   * Reset-gating ON (default) — the `disable iff` guard is dropped and
        //     rst_n is pinned inactive, so the body `state != 3` is a pure
        //     state-cell property → HOLDS. (`gated_resets = ["rst_n=1"]`.)
        //   * Reset-gating OFF — the guard is KEPT, so the formula carries the
        //     `!rst_n` vacuity disjunct, and (H.B) rst_n — a primary input — is
        //     admitted as a FREE cube dimension. The verdict is read across both
        //     reset flavours: rst_n low ⇒ the disjunct is vacuously true; rst_n
        //     high ⇒ `state != 3` (the FSM never reaches 3) → HOLDS, soundly,
        //     with no reset assumption. (`gated_resets` empty.)
        //
        // Pre-H.B the un-gated `!rst_n` atom was unbindable → SKIPPED; H.B's
        // free-input admission subsumes that skip with a sound verdict. Both
        // paths are sound for this property; they differ only in mechanism (pin
        // the reset vs. treat it as a free environment input), visible in the
        // diagnostics.
        let sv = "module fsm (input logic clk, input logic rst_n);\n\
                  logic [1:0] state;\n\
                  always_ff @(posedge clk) begin\n\
                    if (!rst_n) state <= 2'd0;\n\
                    else state <= (state == 2'd2) ? 2'd0 : state + 2'd1;\n\
                  end\n\
                  ok: assert property (@(posedge clk) disable iff (!rst_n) state != 2'd3);\n\
                  endmodule\n";
        let sources = vec![("fsm.sv".to_string(), sv.to_string())];
        let yopts = YosysOptions {
            top: Some("fsm".to_string()),
            use_sv2v: true,
            ..Default::default()
        };

        // Reset-gating ON (default): the guard is dropped + rst_n pinned.
        let gated = verify_auto(&sources, &yopts, &VerifyAutoOptions::default())
            .expect("verify_auto runs with reset-gating");
        assert_eq!(gated.properties.len(), 1);
        assert!(
            matches!(gated.properties[0].outcome, VerifyOutcome::Holds),
            "reset-gated `state != 3` should HOLD; got {:?}",
            gated.properties[0].outcome
        );
        assert_eq!(
            gated.diagnostics.gated_resets,
            vec!["rst_n=1".to_string()],
            "active-low reset pinned inactive"
        );

        // Reset-gating OFF: the `!rst_n` atom is a primary input, admitted by
        // H.B as a free cube dimension. The kept `disable iff` disjunct makes
        // reset cycles vacuous, so the property is verified across all reset
        // sequences → still HOLDS, soundly, with no reset assumption.
        let ungated = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                gate_reset: false,
                ..Default::default()
            },
        )
        .expect("verify_auto runs without reset-gating");
        assert!(
            matches!(ungated.properties[0].outcome, VerifyOutcome::Holds),
            "H.B admits the reset as a free input; the un-gated property still \
             HOLDS soundly (the `!rst_n` disjunct keeps reset cycles vacuous); \
             got {:?}",
            ungated.properties[0].outcome
        );
        assert!(
            ungated.diagnostics.gated_resets.is_empty(),
            "no reset was pinned in the un-gated run"
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_csrng_real_sva_verdict_breakdown() {
        // Real OpenTitan csrng_main_sm SVA end-to-end through verify-auto. Reads
        // the vendored fixtures: the csrng sources from the M.2 fixture + the
        // STANDARD prim_assert macros from the M.0 prim_arbiter fixture (the
        // csrng dir's own prim_assert.sv is the dummy variant that drops all
        // SVA — the XL.0 gotcha). Prints the verdict breakdown; this is the
        // honest measurement of "how many real csrng SVA does verify-auto
        // prove?" — see the diagnostics for the root cause of any SKIP.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
        let csrng = root.join("m2_opentitan_csrng_main_sm/source");
        let prim = root.join("m0_opentitan_prim_arbiter/source");
        let read = |p: PathBuf| {
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        let sources = vec![
            (
                "csrng_main_sm.sv".to_string(),
                read(csrng.join("csrng_main_sm.sv")),
            ),
            ("csrng_pkg.sv".to_string(), read(csrng.join("csrng_pkg.sv"))),
            (
                "prim_assert.sv".to_string(),
                read(prim.join("prim_assert.sv")),
            ),
            (
                "prim_assert_standard_macros.svh".to_string(),
                read(prim.join("prim_assert_standard_macros.svh")),
            ),
            (
                "prim_assert_sec_cm.svh".to_string(),
                read(prim.join("prim_assert_sec_cm.svh")),
            ),
            (
                "prim_flop_macros.sv".to_string(),
                read(prim.join("prim_flop_macros.sv")),
            ),
        ];
        let yopts = YosysOptions {
            top: Some("csrng_main_sm".to_string()),
            use_sv2v: true,
            // The lift (sv2v + Yosys) resolves `\`include`s and packages from
            // `additional_sources` (staged + used as the sv2v include path);
            // `sources` itself only feeds slang's extraction.
            additional_sources: sources[1..].to_vec(),
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on csrng");

        eprintln!("\n=== csrng_main_sm verify-auto breakdown ===");
        eprintln!(
            "translated: {}   unsupported: {}",
            report.properties.len(),
            report.unsupported.len()
        );
        eprintln!(
            "diagnostics: state_registers={}  blackboxed={:?}  gated_resets={:?}",
            report.diagnostics.state_register_count,
            report.diagnostics.blackboxed_modules,
            report.diagnostics.gated_resets,
        );
        let (mut holds, mut violated, mut unknown, mut skipped) = (0, 0, 0, 0);
        for p in &report.properties {
            eprintln!("  [{:?}] {}: {:?}", p.kind, p.name, p.outcome);
            eprintln!("      formula: {}", p.formula);
            eprintln!("      seeded:  {:?}", p.seeded_predicates);
            match p.outcome {
                VerifyOutcome::Holds => holds += 1,
                VerifyOutcome::Violated { .. } => violated += 1,
                VerifyOutcome::Unknown { .. } => unknown += 1,
                VerifyOutcome::Skipped { .. } => skipped += 1,
            }
        }
        eprintln!("HOLDS={holds} VIOLATED={violated} UNKNOWN={unknown} SKIPPED={skipped}");
        eprintln!(
            "auto_provided_stubs: {:?}",
            report.diagnostics.auto_provided_stubs
        );
        for (n, r) in &report.unsupported {
            eprintln!("  unsupported {n}: {r}");
        }

        assert!(
            report.properties.len() >= 2,
            "both csrng ASSERTs (CsrngMainErrorStStable_A, CsrngMainErrorOutput_A) translate"
        );
        // H.C — the FSM's `prim_sparse_fsm_flop` (no body in the source set) is
        // auto-stubbed with a behavioral model, so it is NO LONGER reported as
        // cut and the state register survives.
        assert!(
            report
                .diagnostics
                .auto_provided_stubs
                .iter()
                .any(|m| m.contains("prim_sparse_fsm_flop")),
            "prim_sparse_fsm_flop should be auto-stubbed; got stubs={:?} blackboxed={:?}",
            report.diagnostics.auto_provided_stubs,
            report.diagnostics.blackboxed_modules
        );
        assert!(
            report.diagnostics.state_register_count >= 1,
            "with the flop stubbed, the FSM state register survives; got {}",
            report.diagnostics.state_register_count
        );
        // Reset-gating fires on the macro's `disable iff (rst_ni)`.
        assert_eq!(
            report.diagnostics.gated_resets,
            vec!["rst_ni=1".to_string()]
        );
        // H.A — `state_q` (a `uext`-0 alias of the async-reset register mux)
        // now binds, so the pure-state `$stable` property reaches a verdict
        // instead of SKIP.
        assert!(
            report.properties.iter().any(|p| {
                p.formula.contains("state_q == state_q__past")
                    && !matches!(p.outcome, VerifyOutcome::Skipped { .. })
            }),
            "the $stable state property should bind + reach a verdict; got {:?}",
            report
                .properties
                .iter()
                .map(|p| &p.outcome)
                .collect::<Vec<_>>()
        );
        // `main_sm_err_o` is a combinational output whose cone structurally
        // reaches an input (enable_i / local_escalate_i), so it is classified
        // combinational-of-INPUT → a derived per-cube ⊥-label. But at the
        // `state_q == 41` error cube the labeller PROVES it constant-true
        // (semantically it is `(state_q == Error)` there, the input can't swing
        // it), so the label is definite and `state_q == 41 → main_sm_err_o`
        // reaches a DEFINITE HOLDS. Slice 3 additionally seeds its cone inputs as
        // free dimensions (harmless refinement — the verdict was already HOLDS).
        // This is the key contrast with sva_4/6/8/9, where the box consequent
        // genuinely needs the cone input as a dimension to become definite.
        let sva1 = report
            .properties
            .iter()
            .find(|p| p.name == "csrng_main_sm_sva_1")
            .expect("sva_1 present");
        assert!(
            sva1.seeded_predicates.iter().any(|s| s == "main_sm_err_o"),
            "the combinational output main_sm_err_o binds (as a derived label); got {:?}",
            sva1.seeded_predicates
        );
        assert!(
            matches!(sva1.outcome, VerifyOutcome::Holds),
            "state_q==Error → main_sm_err_o reaches a DEFINITE HOLDS (the label is \
             provably constant-true at the error cube); got {:?}",
            sva1.outcome
        );
        // SOUNDNESS — no spurious counterexample anywhere.
        assert!(
            !report
                .properties
                .iter()
                .any(|p| matches!(p.outcome, VerifyOutcome::Violated { .. })),
            "no spurious VIOLATED from over-resolving a combinational output; got {:?}",
            report
                .properties
                .iter()
                .map(|p| &p.outcome)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_cutpoint_frees_wide_counter_guard_so_exact_symbolic_fits() {
        // Control-slice cut points, end-to-end. A 3-state FSM whose INIT→RUN edge is
        // gated by a wide (48-bit) counter-derived `tick`. That counter sits in the
        // state's cone, so `AG EF (state==DONE)` under `exact-symbolic` is Skipped
        // (cone-too-wide) — UNTIL `--cutpoint tick` frees the guard to a `$anyseq`
        // input, dropping the 48-bit counter out of the cone (COI). The design then
        // fits and the goal is decided HOLDS. The netlist-level, width-agnostic
        // version of the `control_slice.py` source prototype; `cutpoint` needs no
        // width resolution or source rewriting.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        let src = r#"// @mununu_guarantee nu Y.((mu X.(state == 2 || <> X)) && [] Y)
module wide_guard_fsm(input clk, input rst, output reg [1:0] state);
  localparam INIT = 2'd0, RUN = 2'd1, DONE = 2'd2;
  reg [47:0] cnt;
  wire tick = (cnt == 48'd0);
  always @(posedge clk) begin
    if (rst) begin
      state <= INIT;
      cnt   <= 48'd100;
    end else begin
      cnt <= cnt - 48'd1;
      case (state)
        INIT:    if (tick) state <= RUN;
        RUN:     state <= DONE;
        DONE:    state <= INIT;
        default: state <= INIT;
      endcase
    end
  end
endmodule
"#;
        let sources = vec![("wide_guard_fsm.v".to_string(), src.to_string())];
        let base = YosysOptions {
            top: Some("wide_guard_fsm".to_string()),
            ..Default::default()
        };
        let vopts = || VerifyAutoOptions {
            must_edge_inference: MustEdgeInference::SmtHyperMust,
            exact_symbolic: true,
            ..Default::default()
        };

        // (1) WITHOUT cut points: the 48-bit counter is in the state cone → Skipped.
        let no_cut = verify_auto(&sources, &base, &vopts()).expect("verify_auto runs (no cut)");
        let g0 = no_cut
            .properties
            .iter()
            .find(|p| p.name.contains("guarantee"))
            .expect("annotated guarantee present");
        eprintln!("no-cut:   {} => {:?}", g0.name, g0.outcome);
        assert!(
            matches!(g0.outcome, VerifyOutcome::Skipped { .. }),
            "without cut points the wide-counter cone should be Skipped, got {:?}",
            g0.outcome
        );

        // (2) WITH `--cutpoint tick`: guard freed → counter drops out of the cone →
        // exact-symbolic FITS and returns a DEFINITE verdict (Holds or Violated) where
        // it previously refused (Skipped). Decidability — not a specific verdict — is
        // the feature's claim; and per the over-approximation posture a Violated under
        // cut points must not be over-trusted anyway (see the control-slice note). The
        // control-slice note is present and names the cut net.
        let cut = YosysOptions {
            cutpoint_signals: vec!["tick".to_string()],
            ..base.clone()
        };
        let with_cut = verify_auto(&sources, &cut, &vopts()).expect("verify_auto runs (cut)");
        let g1 = with_cut
            .properties
            .iter()
            .find(|p| p.name.contains("guarantee"))
            .expect("annotated guarantee present");
        eprintln!("with-cut: {} => {:?}", g1.name, g1.outcome);
        assert!(
            matches!(
                g1.outcome,
                VerifyOutcome::Holds | VerifyOutcome::Violated { .. }
            ),
            "cutting `tick` should let exact-symbolic reach a DEFINITE verdict (was Skipped), got {:?}",
            g1.outcome
        );
        let cs = with_cut
            .notes
            .iter()
            .find(|n| n.kind == "control-slice")
            .expect("control-slice note present when cut points given");
        assert!(
            cs.items.contains(&"tick".to_string()),
            "note items: {:?}",
            cs.items
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_structural_reset_autopin_makes_box_af_sound() {
        // Structural async-reset auto-pin, end-to-end. A plain-RTL FSM with NO SVA
        // `disable iff` — so the reset is recognized ONLY by the structural detector
        // (from the lifted BTOR2 reset-mux `next(st) = ite(rst, 0, normal)`). Without the
        // auto-pin the free `rst` input leaves a reset-edge from every state to the reset
        // state, and `AF st==2` (`mu Y.(st==2 || [] Y)`) is spuriously VIOLATED; with it,
        // `rst` is pinned inactive and AF decides correctly HOLDS. Also asserts the
        // detected reset is surfaced in `gated_resets` (same path as a `disable iff` one).
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        let src = r#"// @mununu_guarantee mu Y.(st == 2 || [] Y)
module cyc(input clk, input rst, output reg [1:0] st);
  always @(posedge clk)
    if (rst) st <= 2'd0;
    else case (st)
      2'd0: st <= 2'd1;
      2'd1: st <= 2'd2;
      2'd2: st <= 2'd0;
      default: st <= 2'd0;
    endcase
endmodule
"#;
        let sources = vec![("cyc.v".to_string(), src.to_string())];
        let yopts = YosysOptions {
            top: Some("cyc".to_string()),
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                exact_symbolic: true,
                ..Default::default()
            },
        )
        .expect("verify_auto runs");
        // The structural detector pinned the async reset even absent an SVA `disable iff`.
        assert_eq!(
            report.diagnostics.gated_resets,
            vec!["rst=0".to_string()],
            "structural reset auto-pin should pin rst inactive; got {:?}",
            report.diagnostics.gated_resets
        );
        let g = report
            .properties
            .iter()
            .find(|p| p.name.contains("guarantee"))
            .expect("annotated guarantee present");
        eprintln!("AF st==2 => {:?}", g.outcome);
        assert!(
            matches!(g.outcome, VerifyOutcome::Holds),
            "AF st==2 must HOLD once the reset is auto-pinned (was spuriously VIOLATED); got {:?}",
            g.outcome
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_sysrst_config_concretization_flips_timer_relationals() {
        // H.J.b — pinning the wide config timers to constants (the harness's
        // DEB=1, DET=7) turns `cnt_q >= cfg_*_timer_i` into a decidable
        // `cnt_q >= const`, so the config-timer ⊥ (sva_5/7/10/11/13/14) can reach
        // definite verdicts FOR THOSE VALUES. Prints the delta; asserts the
        // config-concretization note is present, the formula shows the constant,
        // and there is NO spurious VIOLATED (sound-for-that-config).
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
        let sysrst = root.join("r46_sysrst_detect_k5/source");
        let prim = root.join("m0_opentitan_prim_arbiter/source");
        let read = |p: PathBuf| {
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        let sources = vec![
            (
                "sysrst_ctrl_detect.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_detect.sv")),
            ),
            (
                "sysrst_ctrl_pkg.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_pkg.sv")),
            ),
            (
                "prim_assert.sv".to_string(),
                read(prim.join("prim_assert.sv")),
            ),
            (
                "prim_assert_standard_macros.svh".to_string(),
                read(prim.join("prim_assert_standard_macros.svh")),
            ),
            (
                "prim_assert_sec_cm.svh".to_string(),
                read(prim.join("prim_assert_sec_cm.svh")),
            ),
            (
                "prim_flop_macros.sv".to_string(),
                read(prim.join("prim_flop_macros.sv")),
            ),
        ];
        let yopts = YosysOptions {
            top: Some("sysrst_ctrl_detect".to_string()),
            use_sv2v: true,
            additional_sources: sources[1..].to_vec(),
            ..Default::default()
        };
        let mut config_values = std::collections::HashMap::new();
        config_values.insert("cfg_debounce_timer_i".to_string(), 1u64);
        config_values.insert("cfg_detect_timer_i".to_string(), 7u64);
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                config_values,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on sysrst with config concretization");

        eprintln!("\n=== sysrst config-concretization breakdown (DEB=1, DET=7) ===");
        let (mut holds, mut violated, mut unknown, mut skipped) = (0, 0, 0, 0);
        for p in &report.properties {
            eprintln!("  {}: {:?}", p.name, p.outcome);
            match p.outcome {
                VerifyOutcome::Holds => holds += 1,
                VerifyOutcome::Violated { .. } => violated += 1,
                VerifyOutcome::Unknown { .. } => unknown += 1,
                VerifyOutcome::Skipped { .. } => skipped += 1,
            }
        }
        eprintln!("HOLDS={holds} VIOLATED={violated} UNKNOWN={unknown} SKIPPED={skipped}");
        for n in &report.notes {
            eprintln!("  note [{}] {}: {}", n.kind, n.summary, n.items.join(", "));
        }

        // The config-concretization note is present, is a ScopeCaveat, and names
        // both pinned timers with their values.
        let cc = report
            .notes
            .iter()
            .find(|n| n.kind == "config-concretization")
            .expect("config-concretization note present");
        assert_eq!(cc.level, NoteLevel::ScopeCaveat);
        assert!(cc.items.contains(&"cfg_detect_timer_i=7".to_string()));
        assert!(cc.items.contains(&"cfg_debounce_timer_i=1".to_string()));
        // The concretized relational atom shows the CONSTANT (substituted), not
        // the free config name.
        let sva7 = report
            .properties
            .iter()
            .find(|p| p.name == "sysrst_ctrl_detect_sva_7")
            .expect("sva_7 present");
        assert!(
            sva7.formula.contains("cnt_q >= 7") && !sva7.formula.contains("cfg_detect_timer_i"),
            "sva_7 formula shows the concretized threshold: {}",
            sva7.formula
        );
        // SOUND — pinning is a restriction (a specific config), so no spurious
        // VIOLATED. The four config-timer relationals flip ⊥ → HOLDS (for these
        // values); HOLDS 9 → 12.
        assert_eq!(violated, 0, "no spurious VIOLATED under concretization");
        assert_eq!(
            skipped, 0,
            "every property still binds under concretization; got SKIPPED={skipped}"
        );
        for name in [
            "sysrst_ctrl_detect_sva_5",  // EnterDetectSt: cnt_q >= cfg_debounce
            "sysrst_ctrl_detect_sva_7",  // EnterStableSt: cnt_q >= cfg_detect
            "sysrst_ctrl_detect_sva_10", // DetectedOut
            "sysrst_ctrl_detect_sva_11", // DetectedPulseOut
        ] {
            let p = report
                .properties
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} present"));
            assert!(
                matches!(p.outcome, VerifyOutcome::Holds),
                "{name} (config-timer relational) reaches HOLDS once the timer is \
                 concretized; got {:?}",
                p.outcome
            );
        }
        assert_eq!(
            holds, 12,
            "config concretization lifts sysrst 9 → 12 HOLDS (sva_5/7/10/11); got {holds}"
        );
        // The residual ⊥ are the non-timer cases: sva_12 (pulse), sva_13/14
        // (counter monotonicity/reset), sva_15 (arithmetic). H.H NOTE: the
        // counter-bound `cnt_q <= 7` is auto-seeded here (the counter-bound note
        // fires, asserted below), but it does NOT flip sva_13/14/15 — their ⊥ is
        // co-dominated by the `cnt_clr` combinational-input ANTECEDENT (a separate
        // cone-unwrap lever), not the counter bound. The bound is necessary infra
        // (it excludes the abstract wraparound) but not sufficient for THIS fixture.
        // The demonstrator `e2e_counter_bound_flips_saturating_monotonicity` below
        // isolates a property where the bound alone flips ⊥ → HOLDS.
        assert_eq!(
            unknown, 4,
            "the non-timer residual ⊥ remain (bound is inert here — cnt_clr antecedent co-blocks); got UNKNOWN={unknown}"
        );
        // H.H — the counter-bound note fires (auto-inferred `cnt_q <= 7` from the
        // sibling `cnt_q >= cfg_detect_timer_i` comparison), even though it does not
        // move a verdict on this fixture.
        let cb = report
            .notes
            .iter()
            .find(|n| n.kind == "counter-bound")
            .expect("counter-bound note present (config-inferred)");
        assert_eq!(cb.level, NoteLevel::Info);
        assert!(
            cb.items.iter().any(|i| i.contains("cnt_q <= 7")),
            "counter-bound note names the inferred bound; got {:?}",
            cb.items
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_onehot0_state_invariant_holds() {
        // One-hot-support demonstrator (STATE case): a one-hot-encoded FSM whose
        // security invariant `$onehot0(state_q)` (at-most-one bit set — the
        // fault-detection property) translates to a value-set predicate over the
        // STATE register, i.e. ONE compound cube dimension. `$onehot0` (not
        // `$onehot`) because reset-gating starts `state_q` at the BTOR2 init `000`
        // (at-most-one-hot ✓; exactly-one would spuriously fail at `000`). The FSM
        // only ever drives one-hot values, so the invariant HOLDS.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        const SV: &str = r#"module onehot_fsm (input logic clk, input logic rst_ni, input logic go_i);
  localparam logic [2:0] S0 = 3'b001, S1 = 3'b010, S2 = 3'b100;
  logic [2:0] state_q, state_d;
  always_comb begin
    state_d = state_q;
    unique case (state_q)
      S0: if (go_i) state_d = S1;
      S1: state_d = S2;
      S2: state_d = S0;
      default: state_d = S0;
    endcase
  end
  always_ff @(posedge clk or negedge rst_ni) begin
    if (!rst_ni) state_q <= S0;
    else         state_q <= state_d;
  end
  OneHotState_A: assert property (@(posedge clk) disable iff (!rst_ni) $onehot0(state_q));
endmodule
"#;
        let sources = vec![("onehot_fsm.sv".to_string(), SV.to_string())];
        let yopts = YosysOptions {
            top: Some("onehot_fsm".to_string()),
            use_sv2v: true,
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on onehot_fsm");
        assert!(
            report.unsupported.is_empty(),
            "unsupported: {:?}",
            report.unsupported
        );
        let p = report
            .properties
            .iter()
            .find(|p| p.formula.contains("state_q == 1"))
            .expect("the $onehot0(state_q) property is present + translated");
        // The translation is the value-set predicate over the state register.
        assert!(
            p.formula.contains("state_q == 1")
                && p.formula.contains("state_q == 2")
                && p.formula.contains("state_q == 4"),
            "$onehot0(state_q) expands to the power-of-two value set: {}",
            p.formula
        );
        assert!(
            matches!(p.outcome, VerifyOutcome::Holds),
            "the one-hot state invariant HOLDS; got {:?}",
            p.outcome
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_opentitan_prim_onehot_check_holds() {
        // One-hot-support demonstrator (real OpenTitan, INPUT case): the hardened
        // `prim_onehot_check.sv` raises `err_o` iff its input `oh_i` is not one-hot0.
        // Its inline `Onehot0Check_A, !$onehot0(oh_i) |-> err_o` exercises the new
        // `$onehot0` translator support end-to-end and reaches a definite HOLDS —
        // `onehot0(oh_i) ∨ err_o` is a per-cube tautology the SMT labeller confirms.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        use std::path::PathBuf;
        let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/verify/opentitan_prim_onehot_check/source");
        let read = |n: &str| {
            std::fs::read_to_string(src.join(n)).unwrap_or_else(|e| panic!("read {n}: {e}"))
        };
        let sources = vec![
            (
                "prim_onehot_check_wrapper.sv".to_string(),
                read("prim_onehot_check_wrapper.sv"),
            ),
            (
                "prim_onehot_check.sv".to_string(),
                read("prim_onehot_check.sv"),
            ),
            ("prim_assert.sv".to_string(), read("prim_assert.sv")),
            (
                "prim_assert_standard_macros.svh".to_string(),
                read("prim_assert_standard_macros.svh"),
            ),
            (
                "prim_assert_sec_cm.svh".to_string(),
                read("prim_assert_sec_cm.svh"),
            ),
            (
                "prim_flop_macros.sv".to_string(),
                read("prim_flop_macros.sv"),
            ),
        ];
        let yopts = YosysOptions {
            top: Some("prim_onehot_check_wrapper".to_string()),
            use_sv2v: true,
            additional_sources: sources[1..].to_vec(),
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on prim_onehot_check");
        assert!(
            report.unsupported.is_empty(),
            "unsupported: {:?}",
            report.unsupported
        );
        let p = report
            .properties
            .iter()
            .find(|p| p.formula.contains("oh_i == 1"))
            .expect("the $onehot0(oh_i) check is present + translated");
        assert!(
            p.formula.contains("oh_i == 8"),
            "$onehot0(oh_i) expands over the 4-bit power-of-two value set: {}",
            p.formula
        );
        assert!(
            matches!(p.outcome, VerifyOutcome::Holds),
            "the OpenTitan one-hot0 check HOLDS; got {:?}",
            p.outcome
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_reduction_or_in_comparison_translates() {
        // Reduction-OR-in-comparison: `(|v_i) == nonzero` — a reduction operand
        // inside a 1-bit comparison — now TRANSLATES (→ an XNOR over `v_i != 0` and
        // `nonzero`) instead of being rejected as unsupported. It reaches the
        // abstraction and a modeled verdict (not SKIPPED). The verdict here is an
        // honest ⊥: the XNOR splits into two independent derived labels (`v_i != 0`
        // and `nonzero`, both the same function of the input `v_i`) and the per-label
        // abstraction doesn't see they are correlated — the SAME cone-completeness /
        // correlation limit as the sysrst cnt_clr residual, which the R-F5 BDD track
        // (joint symbolic evaluation) addresses. This test guards the TRANSLATION
        // (never regress to unsupported) and will tighten to HOLDS once the engine
        // resolves the correlation.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        const SV: &str = r#"module redor_demo (input logic clk, input logic rst_ni, input logic [1:0] v_i);
  logic nonzero;
  assign nonzero = |v_i;
  RedOr_A: assert property (@(posedge clk) disable iff (!rst_ni) (|v_i) == nonzero);
endmodule
"#;
        let sources = vec![("redor_demo.sv".to_string(), SV.to_string())];
        let yopts = YosysOptions {
            top: Some("redor_demo".to_string()),
            use_sv2v: true,
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on redor_demo");
        assert!(
            report.unsupported.is_empty(),
            "reduction-in-comparison translates (nothing unsupported): {:?}",
            report.unsupported
        );
        let p = report
            .properties
            .iter()
            .find(|p| p.formula.contains("v_i != 0"))
            .expect("the reduction `(|v_i)` lowered to `v_i != 0`");
        // Reaches a MODELED verdict (definite or honest ⊥), never SKIPPED/unsupported.
        assert!(
            matches!(
                p.outcome,
                VerifyOutcome::Holds | VerifyOutcome::Unknown { .. }
            ),
            "reduction-in-comparison reaches the abstraction (not SKIPPED); got {:?}\n  {}",
            p.outcome,
            p.formula
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_counter_bound_flips_saturating_monotonicity() {
        // H.H demonstrator — the bound-seeding mechanism isolated from any
        // antecedent co-blocker. A counter rises to K then HOLDS at K
        // (`cnt_d = (cnt_q == K) ? K : cnt_q + 1`). Its UNCONDITIONAL monotonicity
        // `cnt_q >= $past(cnt_q)` is concretely TRUE (the counter never decreases),
        // but ABSTRACTLY ⊥: the predicate cube `{cnt_q >= cnt_q__past}` includes the
        // UNREACHABLE high state `cnt_q = 2^W-1` (> K), whose "increment elsewhere"
        // successor wraps to 0 — a may-edge to a `cnt_q >= cnt_q__past = false` cube
        // with no must-witness, so the box is ⊥. Seeding `cnt_q <= K` (+ its `$past`
        // shadow) EXCLUDES that unreachable state from the cube, removing the
        // spurious may-edge → the verdict becomes a definite HOLDS. This is the
        // sound bound-partition lever the roadmap's H.H targets, on a property where
        // the bound alone is sufficient.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        const SV: &str = r#"module sat_hold_counter (input logic clk, input logic rst_ni);
  localparam int unsigned W = 5;
  localparam logic [W-1:0] K = 5'd7;
  logic [W-1:0] cnt_q, cnt_d;
  assign cnt_d = (cnt_q == K) ? K : (cnt_q + 1'b1);
  always_ff @(posedge clk or negedge rst_ni) begin
    if (!rst_ni) cnt_q <= '0;
    else         cnt_q <= cnt_d;
  end
  MonoCnt_A: assert property (@(posedge clk) disable iff (!rst_ni) cnt_q >= $past(cnt_q));
endmodule
"#;
        let sources = vec![("sat_hold_counter.sv".to_string(), SV.to_string())];
        let yopts = YosysOptions {
            top: Some("sat_hold_counter".to_string()),
            use_sv2v: true,
            ..Default::default()
        };
        let mono = |report: &AutoVerifyReport| -> VerifyOutcome {
            report
                .properties
                .iter()
                .find(|p| p.formula.contains("cnt_q__past"))
                .unwrap_or_else(|| {
                    panic!("monotonicity property present; got {:?}", report.properties)
                })
                .outcome
                .clone()
        };

        // Without a bound — the abstraction FAILS to prove monotonicity: the cube
        // `{cnt_q >= cnt_q__past}` includes the unreachable high state, so the
        // abstract successor can wrap. Depending on the must-edge witness this is a
        // spurious VIOLATED or a ⊥ — either way NOT a definite HOLDS.
        let base = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on the saturating counter (no bound)");
        eprintln!("no-bound monotonicity: {:?}", mono(&base));
        assert!(
            !matches!(mono(&base), VerifyOutcome::Holds),
            "without a bound the abstract wraparound prevents a HOLDS; got {:?}",
            mono(&base)
        );

        // With the bound — the spurious wraparound cube is excluded → HOLDS.
        let mut counter_bounds = std::collections::HashMap::new();
        counter_bounds.insert("cnt_q".to_string(), 7u64);
        let bounded = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                counter_bounds,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on the saturating counter (bounded)");
        eprintln!("bounded monotonicity: {:?}", mono(&bounded));
        assert!(
            matches!(mono(&bounded), VerifyOutcome::Holds),
            "the counter bound flips monotonicity ⊥ → HOLDS; got {:?}",
            mono(&bounded)
        );
        // The counter-bound note is present + names the user-supplied bound.
        let cb = bounded
            .notes
            .iter()
            .find(|n| n.kind == "counter-bound")
            .expect("counter-bound note present");
        assert!(
            cb.items.iter().any(|i| i.contains("cnt_q <= 7")),
            "counter-bound note names the bound; got {:?}",
            cb.items
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_sysrst_detect_real_sva_verdict_breakdown() {
        // Real OpenTitan sysrst_ctrl_detect — the contrast to csrng: its state
        // (`state_q` FSM, `cnt_q` counter, `trigger_active_q`) is in PLAIN
        // `always_ff` (NO prim_flop wrapper), so it SURVIVES the lift. This
        // isolates the *second* real-RTL blocker from the flop-cut: SVA whose
        // antecedents reference IO / config / combinational signals
        // (`cfg_enable_i`, `trigger_event`, `cnt_en`) — those atoms are not
        // cube-bindable. Prints the breakdown.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
        let sysrst = root.join("r46_sysrst_detect_k5/source");
        let prim = root.join("m0_opentitan_prim_arbiter/source");
        let read = |p: PathBuf| {
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        // Standard prim_assert macros (the fixture's own prim_assert.sv is the
        // dummy that drops all SVA).
        let sources = vec![
            (
                "sysrst_ctrl_detect.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_detect.sv")),
            ),
            (
                "sysrst_ctrl_pkg.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_pkg.sv")),
            ),
            (
                "prim_assert.sv".to_string(),
                read(prim.join("prim_assert.sv")),
            ),
            (
                "prim_assert_standard_macros.svh".to_string(),
                read(prim.join("prim_assert_standard_macros.svh")),
            ),
            (
                "prim_assert_sec_cm.svh".to_string(),
                read(prim.join("prim_assert_sec_cm.svh")),
            ),
            (
                "prim_flop_macros.sv".to_string(),
                read(prim.join("prim_flop_macros.sv")),
            ),
        ];
        let yopts = YosysOptions {
            top: Some("sysrst_ctrl_detect".to_string()),
            use_sv2v: true,
            additional_sources: sources[1..].to_vec(),
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on sysrst_ctrl_detect");

        eprintln!("\n=== sysrst_ctrl_detect verify-auto breakdown ===");
        eprintln!(
            "translated: {}   unsupported: {}",
            report.properties.len(),
            report.unsupported.len()
        );
        eprintln!(
            "diagnostics: state_registers={}  blackboxed={:?}  gated_resets={:?}",
            report.diagnostics.state_register_count,
            report.diagnostics.blackboxed_modules,
            report.diagnostics.gated_resets,
        );
        let (mut holds, mut violated, mut unknown, mut skipped) = (0, 0, 0, 0);
        for p in &report.properties {
            eprintln!("  [{:?}] {}: {:?}", p.kind, p.name, p.outcome);
            eprintln!("      formula: {}", p.formula);
            eprintln!("      seeded:  {:?}", p.seeded_predicates);
            match p.outcome {
                VerifyOutcome::Holds => holds += 1,
                VerifyOutcome::Violated { .. } => violated += 1,
                VerifyOutcome::Unknown { .. } => unknown += 1,
                VerifyOutcome::Skipped { .. } => skipped += 1,
            }
        }
        eprintln!("HOLDS={holds} VIOLATED={violated} UNKNOWN={unknown} SKIPPED={skipped}");
        for (n, r) in &report.unsupported {
            eprintln!("  unsupported {n}: {r}");
        }

        // The state survives the lift (no prim_flop cut) — the contrast to
        // csrng. Whatever the per-property outcomes, the model is non-empty.
        assert!(
            report.diagnostics.state_register_count >= 1,
            "plain always_ff state survives the lift (no flop-cut); got {} state registers",
            report.diagnostics.state_register_count
        );
        assert!(
            report.diagnostics.blackboxed_modules.is_empty(),
            "no prim_flop primitive to cut; got {:?}",
            report.diagnostics.blackboxed_modules
        );
        // H.A — the state atoms (`state_q == k`, `cnt_q == k`) now BIND (via the
        // value-alias resolver), so they no longer appear in any SKIP reason.
        for p in &report.properties {
            if let VerifyOutcome::Skipped { reason } = &p.outcome {
                assert!(
                    !reason.contains("state_q ==") && !reason.contains("cnt_q =="),
                    "state atoms should bind, not appear in a SKIP reason; got: {reason}"
                );
            }
        }
        // H.B (free-input atoms) — the headline real-RTL win. `sva_0` is a
        // `cfg_enable_i |=> state_q == 0`-style property: a config INPUT
        // antecedent (`cfg_enable_i`) + a state consequent (`state_q == 0`).
        // Pre-H.B the `cfg_enable_i` atom was unbindable → the property SKIPPED;
        // H.B admits it as a free cube dimension, so it reaches a sound HOLDS
        // (read across both cfg_enable_i flavours). This is the first
        // free-input-antecedent verdict on real OpenTitan RTL.
        let sva0 = report
            .properties
            .iter()
            .find(|p| p.name == "sysrst_ctrl_detect_sva_0")
            .expect("sva_0 present");
        assert!(
            sva0.formula.contains("cfg_enable_i"),
            "sva_0 references the config input cfg_enable_i; got {}",
            sva0.formula
        );
        assert!(
            sva0.seeded_predicates.iter().any(|s| s == "cfg_enable_i"),
            "the free input cfg_enable_i is admitted as a cube dimension (H.B); got {:?}",
            sva0.seeded_predicates
        );
        assert!(
            matches!(sva0.outcome, VerifyOutcome::Holds),
            "the cfg_enable_i |=> state_q==0 property reaches a sound HOLDS via the \
             free-input dimension; got {:?}",
            sva0.outcome
        );
        // After H.E.r2, the simple combinational-of-input antecedents
        // (`trigger_active`, `event_detected_*`, `cnt_clr`) BIND as derived
        // ⊥-labels (HOLDS-or-honest-⊥). H.F then closed the last residual: the
        // RELATIONAL-with-input compounds (`cnt_q >= cfg_*_timer_i`, sva_5/7/10/11
        // — an input operand inside a compound) are now admitted as derived
        // RELATIONAL labels, so they reach an honest ⊥ instead of SKIPPING. Net
        // effect: every translated property binds — SKIPPED is 0.
        assert_eq!(
            skipped, 0,
            "H.F: every translated property binds (relational-with-input compounds \
             now reach a verdict instead of SKIPPING); got SKIPPED={skipped}"
        );
        // No free-input antecedent (H.B) and no relational-with-input compound
        // (H.F) remains skipped.
        for p in &report.properties {
            if let VerifyOutcome::Skipped { reason } = &p.outcome {
                assert!(
                    !reason.contains("cfg_enable_i") && !reason.contains("cnt_q >= cfg_"),
                    "no free-input / relational-with-input atom should remain skipped \
                     after H.B + H.F; got: {reason}"
                );
            }
        }
        // H.F — sva_5/7/10/11 are the relational-with-input compounds
        // (`cnt_q >= cfg_*_timer_i`). Pre-H.F they SKIPPED ("input operand inside
        // a compound"); now they bind as derived relational labels and reach an
        // honest ⊥ (Unknown) — never a spurious verdict.
        for name in [
            "sysrst_ctrl_detect_sva_5",
            "sysrst_ctrl_detect_sva_7",
            "sysrst_ctrl_detect_sva_10",
            "sysrst_ctrl_detect_sva_11",
        ] {
            let p = report
                .properties
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} present"));
            assert!(
                matches!(p.outcome, VerifyOutcome::Unknown { .. }),
                "{name} (relational-with-input compound) binds and reaches an honest \
                 ⊥, not a SKIP; got {:?}",
                p.outcome
            );
        }
        // H.F — sva_2/3 are `trigger_i != trigger_active` where
        // `trigger_active = !trigger_i` (1-bit). `b != !b` is a tautology over the
        // combinational relation, so the derived relational label is definite-True
        // at every cube and the property reaches a sound HOLDS.
        let sva2 = report
            .properties
            .iter()
            .find(|p| p.name == "gen_low_level_sva_sva_2")
            .expect("sva_2 present");
        assert!(
            matches!(sva2.outcome, VerifyOutcome::Holds),
            "sva_2 (`trigger_i != !trigger_i` tautology) reaches a sound HOLDS; got {:?}",
            sva2.outcome
        );
        // H.U.2 — a combinational output that is a function of STATE
        // (`event_detected_o` / `event_detected_pulse_o` = f(state_q)) binds as a
        // CUBE DIMENSION and reaches a DEFINITE verdict: sva_1
        // (`cfg_enable_i → ¬event_detected_o ∧ ¬event_detected_pulse_o`) and
        // sva_12 (`event_pulse → AX ¬event_pulse`) both HOLD. The still-⊥ props
        // (sva_4..11) are ⊥ from their combinational-of-*input* antecedents
        // (`trigger_active`/`cnt_q >= cfg_*`), NOT from any combinational binding
        // gap — every combinational atom binds (SKIPPED is 0, asserted above).
        for name in ["sysrst_ctrl_detect_sva_1", "sysrst_ctrl_detect_sva_12"] {
            let p = report
                .properties
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} present"));
            assert!(
                p.seeded_predicates
                    .iter()
                    .any(|s| s.contains("event_detected")),
                "{name} seeds a combinational-of-state event_detected_* output; got {:?}",
                p.seeded_predicates
            );
            assert!(
                matches!(p.outcome, VerifyOutcome::Holds),
                "{name} reaches a DEFINITE HOLDS (event_detected_* binds as a \
                 function-of-state cube dimension); got {:?}",
                p.outcome
            );
        }
        // H.D translation widening (`signal >= signal`, 1-bit `!x === y`) + H.G
        // arithmetic-addend predicate (`cnt_q == $past(cnt_q) + 1`, CntIncr_A):
        // ALL 16 sysrst SVA now translate — 0 unsupported (pre-H.D: 7 unsupported;
        // pre-H.G: sva_15 was the lone arithmetic holdout).
        assert_eq!(
            report.properties.len(),
            16,
            "H.D + H.G translate all 16 sysrst SVA; got {} translated",
            report.properties.len()
        );
        assert!(
            report.unsupported.is_empty(),
            "H.G closes the last arithmetic holdout — 0 unsupported; got {:?}",
            report.unsupported
        );
        // H.G — sva_15 (`cnt_en && !cnt_clr |=> cnt_q == cnt_q__past + 1`) now
        // BINDS (was unsupported): its arithmetic relational lowers to a
        // `CmpRegAddend` derived label (BV `bvadd`), so it reaches a verdict — an
        // honest ⊥ here (config-timer-entangled, like sva_5/7/10/11/13/14), never
        // unsupported and never a spurious verdict.
        let sva15 = report
            .properties
            .iter()
            .find(|p| p.name == "sysrst_ctrl_detect_sva_15")
            .expect("sva_15 present (translated)");
        assert!(
            sva15.formula.contains("cnt_q__past + 1"),
            "sva_15 carries the arithmetic-addend atom; got {}",
            sva15.formula
        );
        assert!(
            !matches!(sva15.outcome, VerifyOutcome::Skipped { .. }),
            "sva_15 binds to a verdict (not SKIP/unsupported); got {:?}",
            sva15.outcome
        );
        // The negation-peel (`!trigger_i === trigger_active` → `trigger_i !=
        // trigger_active`) and the relational `signal >= signal` both reach the
        // property set now (they SKIP on combinational/input-in-compound
        // operands — the honest residual — but they TRANSLATE).
        assert!(
            report
                .properties
                .iter()
                .any(|p| p.formula.contains("trigger_i != trigger_active")),
            "H.D negation-peel produced the relational `trigger_i != trigger_active`"
        );
        assert!(
            report
                .properties
                .iter()
                .any(|p| p.formula.contains("cnt_q >= cfg_")),
            "H.D translated a `signal >= signal` (cnt_q >= cfg_*_timer_i)"
        );
        // Slice 3 (the unwrap lever, combinational-input-atoms.md §6.1 boundary
        // → resolved) — a trigger-governed conditional-transition safety
        // property `AG((state==k ∧ trigger → AX state==k'))` reaches a DEFINITE
        // HOLDS. The combinational-of-input antecedent (`trigger_active =
        // !trigger_i`) is a derived ⊥-label AND its cone raw input (`trigger_i`)
        // is seeded as a free H.B cube dimension — that refines the may-relation
        // so the consequent box `[](state==k')` becomes definite at the cube
        // pinning trigger_i, and Kleene `⊥ ∨ [] = T`. sva_4/6/8/9 are this class.
        // Pre-Slice-3 they were honest ⊥ (a bare ⊥-label did not refine the box).
        // Soundness rests on the safety + may-over-approximation argument (a
        // definite HOLDS on the KMTS may-relation transfers to the concrete);
        // NEVER a spurious VIOLATED. Independent external confirmation is partial:
        // sva_6 (DetectStDropOut) was checked SAFE by the btormc oracle (H.E/H.O
        // work); sva_4/8/9 are not yet per-property oracle-confirmed — a btormc
        // (H.O.1) sweep is the pending confirmation step.
        for name in [
            "sysrst_ctrl_detect_sva_4",
            "sysrst_ctrl_detect_sva_6",
            "sysrst_ctrl_detect_sva_8",
        ] {
            let p = report
                .properties
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} present"));
            assert!(
                p.seeded_predicates.iter().any(|s| s == "trigger_i"),
                "{name} seeds its combinational antecedent's cone input trigger_i as a \
                 free dimension (Slice 3); got {:?}",
                p.seeded_predicates
            );
            assert!(
                matches!(p.outcome, VerifyOutcome::Holds),
                "{name} (trigger-governed conditional transition) reaches a DEFINITE \
                 HOLDS once trigger_i refines the box; got {:?}",
                p.outcome
            );
        }
        // The remaining ⊥ are the RELATIONAL-with-input compounds (`cnt_q >=
        // cfg_*_timer_i`, sva_5/7/10/11) — the input operand is inside a
        // relational, which the cone-input seeder does not unwrap — plus the
        // cnt_clr cases (sva_13/14). Honest ⊥, never a spurious verdict.
        for name in ["sysrst_ctrl_detect_sva_5", "sysrst_ctrl_detect_sva_7"] {
            let p = report
                .properties
                .iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("{name} present"));
            assert!(
                matches!(p.outcome, VerifyOutcome::Unknown { .. }),
                "{name} (relational-with-input `cnt_q >= cfg_*`) stays an honest ⊥; got {:?}",
                p.outcome
            );
        }
        assert!(
            !report
                .properties
                .iter()
                .any(|p| matches!(p.outcome, VerifyOutcome::Violated { .. })),
            "no spurious VIOLATED; got {:?}",
            report
                .properties
                .iter()
                .map(|p| &p.outcome)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3; run with --ignored"]
    fn e2e_fsm_holds_and_violated_verdicts() {
        // A 2-bit FSM reset to 0, cycling 0→1→2→0. Two state-safety properties:
        //   - `state != 3` HOLDS (the unused encoding is never reached).
        //   - `state == 1` is VIOLATED (false at the reset state, state 0).
        // Exercises the full pipeline (slang → sv2v/Yosys → seed → cube → verdict)
        // and the init-cube-verdict interpretation (the verdict is read at the
        // reset cube, not the lift's cube_0 default).
        let sv = "module fsm (input logic clk, input logic rst_n, input logic go);\n\
                  logic [1:0] state;\n\
                  always_ff @(posedge clk) begin\n\
                    if (!rst_n) state <= 2'd0;\n\
                    else state <= (state == 2'd2) ? 2'd0 : state + 2'd1;\n\
                  end\n\
                  ok:  assert property (@(posedge clk) state != 2'd3);\n\
                  bad: assert property (@(posedge clk) state == 2'd1);\n\
                  endmodule\n";
        let sources = vec![("fsm.sv".to_string(), sv.to_string())];
        let yopts = YosysOptions {
            top: Some("fsm".to_string()),
            use_sv2v: true,
            ..Default::default()
        };
        let report = verify_auto(&sources, &yopts, &VerifyAutoOptions::default())
            .expect("verify_auto runs end-to-end");
        assert_eq!(report.properties.len(), 2, "two assertions verified");
        // Diagnostics: a self-contained FSM lifts to ≥ 1 state register and
        // black-boxes nothing.
        assert!(
            report.diagnostics.state_register_count >= 1,
            "FSM lifts to a real state register; got {}",
            report.diagnostics.state_register_count
        );
        assert!(
            report.diagnostics.blackboxed_modules.is_empty(),
            "self-contained design cuts nothing; got {:?}",
            report.diagnostics.blackboxed_modules
        );
        let by_formula = |needle: &str| -> &VerifyOutcome {
            &report
                .properties
                .iter()
                .find(|p| p.formula.contains(needle))
                .unwrap_or_else(|| panic!("property containing {needle:?}"))
                .outcome
        };
        assert!(
            matches!(by_formula("state != 3"), VerifyOutcome::Holds),
            "`state != 3` should HOLD; got {:?}",
            by_formula("state != 3")
        );
        assert!(
            matches!(by_formula("state == 1"), VerifyOutcome::Violated { .. }),
            "`state == 1` should be VIOLATED (false at reset); got {:?}",
            by_formula("state == 1")
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + yosys — run in the mununu-sva image"]
    fn e2e_oracle_gates_verify_auto_verdicts() {
        // H.O.0.3 — the differential on REAL SV-lifted BTOR2 (not inline). Lift
        // the inline FSM the SAME way verify_auto does, then cross-check every
        // DEFINITE verify_auto outcome against the concrete oracle via
        // `spurious_verdict`. Proves the oracle (a) binds the *lifted* register
        // through `resolve_state_alias` (the symbol survives sv2v + yosys) and
        // (b) gates the verdict on a real lift — incl. the `|=>` (AX) fragment.
        let sv = "module fsm (input logic clk, input logic rst_n, input logic go);\n\
                  logic [1:0] state;\n\
                  always_ff @(posedge clk) begin\n\
                    if (!rst_n) state <= 2'd0;\n\
                    else state <= (state == 2'd2) ? 2'd0 : state + 2'd1;\n\
                  end\n\
                  ok:  assert property (@(posedge clk) state != 2'd3);\n\
                  bad: assert property (@(posedge clk) state == 2'd1);\n\
                  imp: assert property (@(posedge clk) state == 2'd2 |=> state == 2'd0);\n\
                  endmodule\n";
        let sources = vec![("fsm.sv".to_string(), sv.to_string())];
        let yopts = YosysOptions {
            top: Some("fsm".to_string()),
            use_sv2v: true,
            ..Default::default()
        };

        let report = verify_auto(&sources, &yopts, &VerifyAutoOptions::default())
            .expect("verify_auto runs end-to-end");

        // The SAME BTOR2 verify_auto evaluates: single module, no `disable iff`
        // ⇒ no flop stubs / shadows / reset pins, so the lift output is the
        // model under test.
        let (btor2, _bb) =
            crate::adapter::yosys::sv_to_btor2_with_blackboxes(sv, &yopts).expect("sv → btor2");
        let file = crate::adapter::btor2::parser::parse(&btor2).expect("parse lifted btor2");

        let trit_of = |o: &VerifyOutcome| match o {
            VerifyOutcome::Holds => Some(Trit::True),
            VerifyOutcome::Violated { .. } => Some(Trit::False),
            VerifyOutcome::Unknown { .. } => Some(Trit::Unknown),
            VerifyOutcome::Skipped { .. } => None,
        };
        let outcome = |needle: &str| -> &VerifyOutcome {
            &report
                .properties
                .iter()
                .find(|p| p.formula.contains(needle))
                .unwrap_or_else(|| panic!("property containing {needle:?}"))
                .outcome
        };
        // Gate one property: if verify_auto returned a DEFINITE verdict, the
        // oracle must not contradict it (the H.O.0.3 e2e assertion).
        let gate = |needle: &str, oracle: &AgOracle| {
            if let Some(t) = trit_of(outcome(needle)) {
                assert!(
                    spurious_verdict(t, oracle).is_none(),
                    "verify_auto `{needle}` verdict {t:?} contradicts the concrete oracle {oracle:?}"
                );
            }
        };

        // `state != 3` — the oracle proves HOLDS on the lifted design; the
        // verify_auto verdict must agree.
        let o_safe = ag_state_atom(&file, "state", CmpOp::Ne, 3, 1024, 8, false)
            .expect("oracle binds lifted `state`");
        assert_eq!(o_safe, AgOracle::Holds, "oracle: encoding 3 unreachable");
        gate("state != 3", &o_safe);

        // `state == 1` — false at the reset state → the oracle finds the violation.
        let o_bad = ag_state_atom(&file, "state", CmpOp::Eq, 1, 1024, 8, false)
            .expect("oracle ag(state==1)");
        assert!(
            matches!(o_bad, AgOracle::Violated(_)),
            "oracle: state==1 violated at reset"
        );
        gate("state == 1", &o_bad);

        // `state == 2 |=> state == 0` — the AX fragment: from state 2 the next
        // state is always 0. The oracle proves HOLDS; verify_auto must agree.
        let ante = OracleAtom::new("state", CmpOp::Eq, 2);
        let cons = OracleAtom::new("state", CmpOp::Eq, 0);
        let o_imp = ag_implies_next(&file, &ante, &cons, 1024, 8, false)
            .expect("oracle ag(state==2 |=> 0)");
        assert_eq!(o_imp, AgOracle::Holds, "oracle: 2 always steps to 0");
        gate("state == 2", &o_imp);
    }

    #[test]
    #[ignore = "requires slang + sv2v + yosys — run in the mununu-sva image"]
    fn e2e_oracle_gates_sysrst_real_rtl_verdict() {
        // H.O.0.3b — gate the ONE definite REAL-OpenTitan verdict against the
        // concrete oracle: sysrst_ctrl_detect sva_0, `cfg_enable_i |=> state_q
        // == 0`, which verify_auto reports HOLDS (a config-INPUT antecedent + a
        // state consequent — exactly `ag_implies_next`'s fragment).
        //
        // The oracle must answer verify_auto's GATED question, so we pin the SAME
        // reset verify_auto pinned (`diagnostics.gated_resets`) before enumerating
        // — otherwise the oracle would explore reset-active states verify_auto
        // excluded and could "refute" a verdict that is correct under the gated
        // semantics. Pinning happens at the BTOR2 level (`pin_inputs_to_constants`),
        // so the oracle needs no reset awareness.
        //
        // Honest boundary: sysrst has WIDE config inputs (`cfg_detect_timer_i`)
        // the oracle cannot exhaustively enumerate, so its reachability is
        // BOUNDED → it can soundly REFUTE a (shallow) spurious HOLDS but cannot
        // CONFIRM a real-RTL HOLDS. Confirming real-RTL verdicts is H.O.1's
        // (external BTOR2 model checker) domain. This test proves the oracle
        // BINDS real-RTL atoms (state_q via value-alias resolution; cfg_enable_i
        // as a 1-bit input) and provides a sound refutation guard.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
        let sysrst = root.join("r46_sysrst_detect_k5/source");
        let prim = root.join("m0_opentitan_prim_arbiter/source");
        let read = |p: PathBuf| {
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        let sources = vec![
            (
                "sysrst_ctrl_detect.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_detect.sv")),
            ),
            (
                "sysrst_ctrl_pkg.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_pkg.sv")),
            ),
            (
                "prim_assert.sv".to_string(),
                read(prim.join("prim_assert.sv")),
            ),
            (
                "prim_assert_standard_macros.svh".to_string(),
                read(prim.join("prim_assert_standard_macros.svh")),
            ),
            (
                "prim_assert_sec_cm.svh".to_string(),
                read(prim.join("prim_assert_sec_cm.svh")),
            ),
            (
                "prim_flop_macros.sv".to_string(),
                read(prim.join("prim_flop_macros.sv")),
            ),
        ];
        let yopts = YosysOptions {
            top: Some("sysrst_ctrl_detect".to_string()),
            use_sv2v: true,
            additional_sources: sources[1..].to_vec(),
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on sysrst_ctrl_detect");

        let sva0 = report
            .properties
            .iter()
            .find(|p| p.name == "sysrst_ctrl_detect_sva_0")
            .expect("sva_0 present");
        assert!(
            matches!(sva0.outcome, VerifyOutcome::Holds),
            "precondition: verify_auto reports sva_0 HOLDS; got {:?}",
            sva0.outcome
        );

        // Lift the base BTOR2 + pin the SAME resets verify_auto pinned.
        let (btor2, _bb) =
            crate::adapter::yosys::sv_to_btor2_with_blackboxes(&sources[0].1, &yopts)
                .expect("sv → btor2");
        let pins: Vec<(String, u64)> = report
            .diagnostics
            .gated_resets
            .iter()
            .filter_map(|g| {
                let (n, v) = g.split_once('=')?;
                Some((n.to_string(), v.trim().parse::<u64>().ok()?))
            })
            .collect();
        eprintln!("\n=== H.O.0.3b sysrst gate: pinning resets {pins:?} ===");
        let (pinned, _) = crate::adapter::btor2::pin::pin_inputs_to_constants(&btor2, &pins);
        let file = crate::adapter::btor2::parser::parse(&pinned).expect("parse pinned btor2");

        // The oracle for sva_0's exact shape:
        //   formula: nu X. (((!((!(cfg_enable_i))) || [] (state_q == 0))) && [] X)
        //   = (!cfg_enable_i) |=> (state_q == 0)   — "while DISABLED, stay idle".
        // The antecedent is `cfg_enable_i == 0` (the double-negation in the
        // formula resolves to `!a = cfg_enable_i`, so the SVA antecedent
        // `a = !cfg_enable_i`). Getting this polarity wrong checks a different
        // property and yields a bogus contradiction.
        let ante = OracleAtom::new("cfg_enable_i", CmpOp::Eq, 0);
        let cons = OracleAtom::new("state_q", CmpOp::Eq, 0);
        // reset_pinned = true: rst_ni is pinned to a constant above, so following
        // the async-reset register mux to `state_q`'s state cell is sound (the mux
        // always selects the state branch) — the same condition the verify-auto
        // seeder resolves `state_q` under.
        let oracle = ag_implies_next(&file, &ante, &cons, 100_000, 8, true)
            .expect("oracle binds real-RTL state_q (resolution) + cfg_enable_i (input)");
        eprintln!("sysrst sva_0 `!cfg_enable_i |=> state_q == 0` oracle: {oracle:?}");

        // Soundness gate: verify_auto's HOLDS must not be contradicted. On a
        // genuinely-holding property the oracle returns Holds (if the design fits
        // the cap) or Inconclusive (bounded) — never Violated.
        assert!(
            spurious_verdict(Trit::True, &oracle).is_none(),
            "verify_auto sva_0 HOLDS contradicted by the concrete oracle: {oracle:?}"
        );
        assert!(
            matches!(oracle, AgOracle::Holds | AgOracle::Inconclusive),
            "no reachable violation of a genuinely-holding property; got {oracle:?}"
        );
    }

    // ---- H.O.0.2 — oracle differential vs the cube verdict --------------------
    //
    // The 2026-06-29 soundness review's headline gap: nothing independently
    // checks that a DEFINITE verify-auto verdict is correct, and a spurious
    // `HOLDS` (silently claiming a property holds) is the dangerous case. These
    // tests drive the SAME cube path verify_auto uses — `seed_from_formula` →
    // `synth_sidecar_json` → `cegar_refine_loop`, read at the reset cube — on a
    // hand-built BTOR2 FSM, and cross-check every DEFINITE cube verdict against
    // the concrete bounded-reachability oracle (`concrete_oracle::ag_state_atom`,
    // H.O.0.1).
    //
    // Invariant (the actual guard `cube_vs_oracle` enforces):
    //   cube-HOLDS    ⟹ oracle is NOT Violated   (else: SPURIOUS HOLDS)
    //   cube-VIOLATED ⟹ oracle is NOT Holds       (else: SPURIOUS VIOLATED)
    //   cube-⊥        ⟹ anything                  (sound coarsening)
    //
    // No slang: the cube path runs directly on inline BTOR2, so the differential
    // executes in `make ci` (not behind the slang `#[ignore]` gate). The seeding
    // + reset-cube logic mirrors verify_auto's per-property block (the
    // `for t in ...` loop above) — keep them in sync when that block changes.
    use crate::adapter::AdapterOptions;
    use crate::adapter::btor2::cegar::{
        CegarOptions, LiftStrategy, PredicateSource, cegar_refine_loop,
    };
    use crate::adapter::btor2::concrete_oracle::{
        AgOracle, OracleAtom, ag_implies_next, ag_state_atom,
    };
    use crate::mu_calculus::Environment;
    use crate::mu_calculus::trit::Trit;

    /// A 2-bit FSM cycling 0→1→2→0 (caps at 2, never reaches 3), reset to 0,
    /// input-free. Same design as `concrete_oracle`'s CYCLE_FSM, so the cube
    /// path and the oracle observe one model.
    const DIFF_CYCLE_FSM: &str = "\
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

    /// Drive the cube path for `AG (signal op value)` exactly as verify_auto's
    /// default does (seed → synth sidecar → `cegar_refine_loop` → verdict at the
    /// reset cube), AND the concrete oracle for the same atom; assert the
    /// soundness invariant and return both verdicts for the caller to pin.
    fn cube_vs_oracle(
        btor2: &str,
        init_values: &[(&str, u64)],
        signal: &str,
        op: CmpOp,
        value: u64,
    ) -> (Trit, AgOracle) {
        let op_s = match op {
            CmpOp::Eq => "==",
            CmpOp::Ne => "!=",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        };
        let atom = format!("{signal} {op_s} {value}");
        let formula = mu_parser::parse(&format!("nu X. (({atom}) && [] X)")).unwrap();

        // Seed as verify_auto: the atom's register is our single state cell;
        // no free inputs, no combinational signals.
        let seeded = seed_from_formula(
            &formula,
            |r| r == signal,
            |_| false,
            |_| None,
            |_| Vec::new(),
            &[],
        );

        // Mirror verify_auto's default cegar_opts (must Off; may = SmtAllPairs —
        // AR-S2 retired the sampling-may fallback).
        let cegar_opts = CegarOptions {
            max_iterations: 16,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
            smart_uf_cap: true,
            lift_strategy: LiftStrategy::Eager,
            must_edge_inference: MustEdgeInference::Off,
            may_edge_inference: MayEdgeInference::SmtAllPairs,
            emit_ctxdsl: false,
        };

        let init_u64: std::collections::HashMap<String, u64> = init_values
            .iter()
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        let mut referenced: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for s in &seeded.specs {
            referenced.insert(s.register.clone());
        }
        for (_, e) in &seeded.compounds {
            for r in e.registers() {
                referenced.insert(r);
            }
        }
        let adapter_options = AdapterOptions {
            sidecar_json: synth_sidecar_json(
                &seeded.compounds,
                &seeded.derived,
                &seeded.derived_relational,
                &referenced,
                &init_u64,
            ),
            ..Default::default()
        };

        // Reset cube index: bit order = simple specs, then compounds (the order
        // `cegar_refine_loop` assembles `current_predicates`).
        let predicate_count = seeded.specs.len() + seeded.compounds.len();
        let env = Environment::new(1usize << predicate_count);
        let init_u128: std::collections::HashMap<String, u128> = init_u64
            .iter()
            .map(|(k, v)| (k.clone(), *v as u128))
            .collect();
        let mut base_init_cube = 0usize;
        let mut free_input_bits: Vec<u32> = Vec::new();
        let mut bit = 0u32;
        for s in &seeded.specs {
            if seeded.input_registers.contains(&s.register) {
                free_input_bits.push(bit);
            } else if init_u64.get(&s.register).copied().unwrap_or(0) == s.value {
                base_init_cube |= 1 << bit;
            }
            bit += 1;
        }
        for (_, expr) in &seeded.compounds {
            if expr.eval(&init_u128) {
                base_init_cube |= 1 << bit;
            }
            bit += 1;
        }
        let init_cubes = free_input_init_cubes(base_init_cube, &free_input_bits);

        let trace = cegar_refine_loop(
            &formula,
            btor2,
            seeded.specs.clone(),
            &env,
            &adapter_options,
            &cegar_opts,
        )
        .expect("cegar succeeds");
        let v = &trace.final_verdict;
        let cube = if init_cubes.iter().any(|&ic| v.verdict_at(ic) == Trit::False) {
            Trit::False
        } else if init_cubes
            .iter()
            .any(|&ic| v.verdict_at(ic) == Trit::Unknown)
        {
            Trit::Unknown
        } else {
            Trit::True
        };

        // The oracle side (H.O.0.1) — concrete bounded reachability, no abstraction.
        let file = crate::adapter::btor2::parser::parse(btor2).expect("oracle parses btor2");
        let oracle =
            ag_state_atom(&file, signal, op, value as u128, 1024, 8, false).expect("oracle runs");

        // The spurious-verdict guard.
        if let Some(reason) = spurious_verdict(cube, &oracle) {
            panic!("{reason} for `AG {atom}` (cube={cube:?}, oracle={oracle:?})");
        }
        (cube, oracle)
    }

    /// The H.O.0.2 soundness invariant, as a pure predicate so it can be
    /// negative-control-tested. Returns `Some(reason)` iff the cube verdict and
    /// the concrete oracle *contradict* — a definite cube verdict the oracle
    /// refutes. `None` means consistent (including every cube-⊥, the sound
    /// coarsening, and every case the oracle can't conclude).
    fn spurious_verdict(cube: Trit, oracle: &AgOracle) -> Option<&'static str> {
        match (cube, oracle) {
            // cube proved HOLDS, but a reachable state violates → spurious HOLDS
            // (the dangerous case the review flagged).
            (Trit::True, AgOracle::Violated(_)) => Some("SPURIOUS HOLDS"),
            // cube refuted, but full concrete enumeration found no violation →
            // spurious VIOLATED. (A *bounded* `Inconclusive` is NOT a
            // contradiction — the oracle simply couldn't confirm.)
            (Trit::False, AgOracle::Holds) => Some("SPURIOUS VIOLATED"),
            _ => None,
        }
    }

    #[test]
    fn differential_ag_holds_agrees_with_oracle() {
        // `AG state != 3` — 3 is the unreachable encoding of the 0→1→2→0 FSM.
        // The cube proves HOLDS; the oracle confirms (full enumeration).
        let (cube, oracle) = cube_vs_oracle(DIFF_CYCLE_FSM, &[("state", 0)], "state", CmpOp::Ne, 3);
        assert_eq!(cube, Trit::True, "cube proves AG state != 3");
        assert_eq!(oracle, AgOracle::Holds, "oracle confirms 3 unreachable");
    }

    #[test]
    fn differential_ag_violated_agrees_with_oracle() {
        // `AG state == 1` — false at the reset state (state 0) → VIOLATED at the
        // reset cube, independent of edges (νX ≤ body).
        let (cube, oracle) = cube_vs_oracle(DIFF_CYCLE_FSM, &[("state", 0)], "state", CmpOp::Eq, 1);
        assert_eq!(
            cube,
            Trit::False,
            "cube refutes AG state == 1 at the reset cube"
        );
        assert!(
            matches!(oracle, AgOracle::Violated(_)),
            "oracle finds the violation"
        );
    }

    #[test]
    fn differential_ag_bottom_is_sound_no_spurious_holds() {
        // `AG state != 1` — state 1 IS reachable (0→1), so the property is FALSE.
        // The single-predicate cube is too coarse to refute it (no must-edge
        // under the default must-Off policy) → the SOUND answer is ⊥, NOT a
        // spurious HOLDS. A regression to a spurious HOLDS here trips
        // `cube_vs_oracle`'s invariant. This is the H.O.0.2 headline guard.
        let (cube, oracle) = cube_vs_oracle(DIFF_CYCLE_FSM, &[("state", 0)], "state", CmpOp::Ne, 1);
        assert_eq!(
            cube,
            Trit::Unknown,
            "coarse cube is appropriately ⊥, not a spurious HOLDS"
        );
        assert!(
            matches!(oracle, AgOracle::Violated(_)),
            "oracle: state 1 is reachable"
        );
    }

    #[test]
    fn differential_guard_catches_spurious_verdicts() {
        // Negative control: prove the invariant is LIVE (not vacuously passing).
        // The two contradictory pairs must be flagged; every consistent pair
        // (incl. cube-⊥ and the bounded-`Inconclusive` escape) must not.
        let witness: std::collections::BTreeMap<String, u128> =
            [("state".to_string(), 1u128)].into_iter().collect();
        // Contradictions → flagged.
        assert_eq!(
            spurious_verdict(Trit::True, &AgOracle::Violated(witness.clone())),
            Some("SPURIOUS HOLDS")
        );
        assert_eq!(
            spurious_verdict(Trit::False, &AgOracle::Holds),
            Some("SPURIOUS VIOLATED")
        );
        // Consistent → not flagged.
        assert_eq!(spurious_verdict(Trit::True, &AgOracle::Holds), None);
        assert_eq!(
            spurious_verdict(Trit::False, &AgOracle::Violated(witness)),
            None
        );
        assert_eq!(spurious_verdict(Trit::Unknown, &AgOracle::Holds), None);
        assert_eq!(
            spurious_verdict(Trit::Unknown, &AgOracle::Inconclusive),
            None
        );
        // A bounded oracle can't conclude true: cube-False + Inconclusive is NOT
        // a contradiction (the oracle just didn't reach the violation).
        assert_eq!(spurious_verdict(Trit::False, &AgOracle::Inconclusive), None);
        // cube-True + Inconclusive is also fine — no violation was *found*.
        assert_eq!(spurious_verdict(Trit::True, &AgOracle::Inconclusive), None);
    }

    // ---- H.O.1c — external BTOR2-MC oracle differential (btormc) ---------------
    //
    // H.O.0's concrete oracle is enumeration-bounded: on a design with wide inputs
    // it can REFUTE a spurious cube-HOLDS but is forced to `Inconclusive` and
    // cannot CONFIRM a real-RTL HOLDS (H.O.0.3b). H.O.1 adds the external SYMBOLIC
    // oracle: emit the property as a BTOR2 `bad` monitor (H.O.1b) and let
    // `btormc --kind` (H.O.1a) decide it — `unsat` ⇒ SAFE/HOLDS (a real
    // k-induction proof, no enumeration), `sat` ⇒ VIOLATED. These tests are
    // `#[ignore]`d (need btormc — run in `mununu-sva`); the pure differential
    // logic (`spurious_verdict_mc`) is make-ci unit-tested below.
    use crate::adapter::btor2::bad_monitor::{
        emit_ag_implies_next_monitor, emit_ag_state_atom_monitor,
    };
    use crate::adapter::btormc::{
        DEFAULT_KMAX, DEFAULT_TIMEOUT, McVerdict, locate_btormc, run_btormc,
    };

    /// MC analog of [`spurious_verdict`]: `Some(reason)` iff a DEFINITE cube
    /// verdict and the external model checker *contradict*. `McVerdict::Unknown`
    /// (bounded — no CEX and no proof) never contradicts.
    fn spurious_verdict_mc(cube: Trit, mc: McVerdict) -> Option<&'static str> {
        match (cube, mc) {
            (Trit::True, McVerdict::Violated) => Some("SPURIOUS HOLDS"),
            (Trit::False, McVerdict::Safe) => Some("SPURIOUS VIOLATED"),
            _ => None,
        }
    }

    #[test]
    fn mc_guard_catches_spurious_verdicts() {
        // make-ci negative control (pure logic; no btormc). Proves the MC
        // soundness invariant is LIVE: contradictions flagged, everything
        // consistent — including every `Unknown` — not flagged.
        assert_eq!(
            spurious_verdict_mc(Trit::True, McVerdict::Violated),
            Some("SPURIOUS HOLDS")
        );
        assert_eq!(
            spurious_verdict_mc(Trit::False, McVerdict::Safe),
            Some("SPURIOUS VIOLATED")
        );
        assert_eq!(spurious_verdict_mc(Trit::True, McVerdict::Safe), None);
        assert_eq!(spurious_verdict_mc(Trit::False, McVerdict::Violated), None);
        assert_eq!(spurious_verdict_mc(Trit::Unknown, McVerdict::Safe), None);
        assert_eq!(
            spurious_verdict_mc(Trit::Unknown, McVerdict::Violated),
            None
        );
        assert_eq!(spurious_verdict_mc(Trit::True, McVerdict::Unknown), None);
        assert_eq!(spurious_verdict_mc(Trit::False, McVerdict::Unknown), None);
    }

    // A btormc-compatible 2-bit FSM cycling 0→1→2→0 (caps at 2, never 3) with an
    // irrelevant WIDE (8-bit) input `w`. The wide input forces the internal
    // oracle's enumeration BOUNDED (→ Inconclusive); btormc decides symbolically
    // (w is irrelevant to the FSM). Init const (nid 5) precedes the state (nid 6)
    // so btormc's `init state_nid > val_nid` rule holds.
    const WIDE_INPUT_FSM: &str = "\
1 sort bitvec 1
2 sort bitvec 2
3 sort bitvec 8
4 input 3 w
5 zero 2
6 state 2 cnt
7 one 2
8 constd 2 2
9 eq 1 6 8
10 add 2 6 7
11 ite 2 9 5 10
12 next 2 6 11
13 init 2 6 5
";

    #[test]
    #[ignore = "requires btormc (MUNUNU_BTORMC_PATH or $PATH); run with --ignored in mununu-sva"]
    fn e2e_btormc_decides_beyond_the_internal_oracle_cap() {
        // The H.O.1 payoff, deterministic: the internal oracle is Inconclusive
        // (the wide input caps enumeration), btormc returns a DEFINITE Safe.
        let file = crate::adapter::btor2::parser::parse(WIDE_INPUT_FSM).unwrap();
        let internal = ag_state_atom(&file, "cnt", CmpOp::Ne, 3, 256, 8, false).unwrap();
        assert_eq!(
            internal,
            AgOracle::Inconclusive,
            "wide input ⇒ bounded enumeration ⇒ internal oracle cannot confirm"
        );

        let bin = locate_btormc().expect("btormc present");
        let monitor =
            emit_ag_state_atom_monitor(WIDE_INPUT_FSM, "cnt", CmpOp::Ne, 3, false).unwrap();
        let mc = run_btormc(&bin, &monitor, DEFAULT_KMAX, DEFAULT_TIMEOUT).unwrap();
        assert_eq!(
            mc,
            McVerdict::Safe,
            "btormc proves AG(cnt != 3) symbolically, beyond the enumeration cap"
        );
        // A cube-HOLDS on this property is CONFIRMED (no spurious-HOLDS).
        assert!(spurious_verdict_mc(Trit::True, mc).is_none());
    }

    #[test]
    #[ignore = "requires slang+sv2v+yosys+btormc (mununu-sva); run with --ignored"]
    fn e2e_btormc_confirms_sysrst_real_rtl_verdict() {
        // H.O.1c real-RTL payoff: the external MC CONFIRMS the one definite
        // real-OpenTitan verdict (sysrst sva_0, `!cfg_enable_i |=> state_q == 0`,
        // which verify_auto reports HOLDS) where the internal oracle is forced to
        // `Inconclusive` (sysrst's wide `cfg_detect_timer_i` exceeds the cap;
        // H.O.0.3b). Same lift + reset-pin as `e2e_oracle_gates_sysrst_real_rtl_verdict`.
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
        let sysrst = root.join("r46_sysrst_detect_k5/source");
        let prim = root.join("m0_opentitan_prim_arbiter/source");
        let read = |p: PathBuf| {
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        let sources = vec![
            (
                "sysrst_ctrl_detect.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_detect.sv")),
            ),
            (
                "sysrst_ctrl_pkg.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_pkg.sv")),
            ),
            (
                "prim_assert.sv".to_string(),
                read(prim.join("prim_assert.sv")),
            ),
            (
                "prim_assert_standard_macros.svh".to_string(),
                read(prim.join("prim_assert_standard_macros.svh")),
            ),
            (
                "prim_assert_sec_cm.svh".to_string(),
                read(prim.join("prim_assert_sec_cm.svh")),
            ),
            (
                "prim_flop_macros.sv".to_string(),
                read(prim.join("prim_flop_macros.sv")),
            ),
        ];
        let yopts = YosysOptions {
            top: Some("sysrst_ctrl_detect".to_string()),
            use_sv2v: true,
            additional_sources: sources[1..].to_vec(),
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                must_edge_inference: MustEdgeInference::SmtHyperMust,
                ..Default::default()
            },
        )
        .expect("verify_auto runs on sysrst_ctrl_detect");
        let sva0 = report
            .properties
            .iter()
            .find(|p| p.name == "sysrst_ctrl_detect_sva_0")
            .expect("sva_0 present");
        assert!(
            matches!(sva0.outcome, VerifyOutcome::Holds),
            "precondition: verify_auto reports sva_0 HOLDS; got {:?}",
            sva0.outcome
        );

        // Lift the base BTOR2 + pin the SAME resets verify_auto pinned.
        let (btor2, _bb) =
            crate::adapter::yosys::sv_to_btor2_with_blackboxes(&sources[0].1, &yopts)
                .expect("sv → btor2");
        let pins: Vec<(String, u64)> = report
            .diagnostics
            .gated_resets
            .iter()
            .filter_map(|g| {
                let (n, v) = g.split_once('=')?;
                Some((n.to_string(), v.trim().parse::<u64>().ok()?))
            })
            .collect();
        let (pinned, _) = crate::adapter::btor2::pin::pin_inputs_to_constants(&btor2, &pins);

        // sva_0 = `(!cfg_enable_i) |=> (state_q == 0)`. Emit its `bad` monitor on
        // the pinned BTOR2 (reset_pinned = true: the async-reset mux to state_q is
        // sound under the pin). btormc decides it symbolically.
        let ante = OracleAtom::new("cfg_enable_i", CmpOp::Eq, 0);
        let cons = OracleAtom::new("state_q", CmpOp::Eq, 0);
        let monitor = emit_ag_implies_next_monitor(&pinned, &ante, &cons, true)
            .expect("emit |=> monitor on real-RTL (binds state_q + cfg_enable_i)");

        let bin = locate_btormc().expect("btormc present");
        let mc = run_btormc(&bin, &monitor, 60, DEFAULT_TIMEOUT)
            .expect("btormc runs on the sysrst monitor");
        eprintln!("\n=== H.O.1c sysrst sva_0 external-MC verdict: {mc:?} ===");

        // Soundness gate (always): verify_auto's HOLDS (Trit::True) must not be
        // contradicted — btormc must NOT return Violated.
        assert!(
            spurious_verdict_mc(Trit::True, mc).is_none(),
            "verify_auto sva_0 HOLDS contradicted by btormc: {mc:?}"
        );
        assert_ne!(
            mc,
            McVerdict::Violated,
            "no reachable violation of a genuinely-holding property"
        );
        // The H.O.1 payoff: btormc reaches a DEFINITE Safe where the internal
        // oracle is Inconclusive — the real-RTL CONFIRMATION H.O.0 could not give.
        // Observed `Safe` in the mununu-sva image (btormc 3.2.4, k-induction closes
        // within kmax=60); this asserts that confirmation does not regress.
        assert_eq!(
            mc,
            McVerdict::Safe,
            "btormc confirms sysrst sva_0 HOLDS (the real-RTL confirmation beyond H.O.0's cap)"
        );
    }

    /// R-F5.5d — run the sysrst_ctrl_detect breakdown through BOTH the explicit
    /// and the R-F5 **symbolic** engine on real OpenTitan RTL. The symbolic
    /// engine supports the cube-dimension-predicate + bare-modality fragment (it
    /// Skips derived combinational per-cube-label predicates the explicit path
    /// binds, and its `∀∃` must differs from the explicit `SmtHyperMust`), so a
    /// full verdict match is not expected — this validates that the symbolic
    /// path *runs end-to-end on real RTL*, completes, and yields definite
    /// verdicts. Prints the per-property explicit-vs-symbolic comparison + the
    /// wall-clock of each engine for inspection.
    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3 (use the mununu-sva docker image); run with --ignored"]
    fn e2e_sysrst_symbolic_engine_runs_on_real_rtl() {
        use crate::adapter::btor2::kmts_lift::MustEdgeInference;
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
        let sysrst = root.join("r46_sysrst_detect_k5/source");
        let prim = root.join("m0_opentitan_prim_arbiter/source");
        let read = |p: PathBuf| {
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        let sources = vec![
            (
                "sysrst_ctrl_detect.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_detect.sv")),
            ),
            (
                "sysrst_ctrl_pkg.sv".to_string(),
                read(sysrst.join("sysrst_ctrl_pkg.sv")),
            ),
            (
                "prim_assert.sv".to_string(),
                read(prim.join("prim_assert.sv")),
            ),
            (
                "prim_assert_standard_macros.svh".to_string(),
                read(prim.join("prim_assert_standard_macros.svh")),
            ),
            (
                "prim_assert_sec_cm.svh".to_string(),
                read(prim.join("prim_assert_sec_cm.svh")),
            ),
            (
                "prim_flop_macros.sv".to_string(),
                read(prim.join("prim_flop_macros.sv")),
            ),
        ];
        let yopts = YosysOptions {
            top: Some("sysrst_ctrl_detect".to_string()),
            use_sv2v: true,
            additional_sources: sources[1..].to_vec(),
            ..Default::default()
        };
        let run = |symbolic: bool| {
            let start = std::time::Instant::now();
            let r = verify_auto(
                &sources,
                &yopts,
                &VerifyAutoOptions {
                    must_edge_inference: MustEdgeInference::SmtHyperMust,
                    symbolic_engine: symbolic,
                    ..Default::default()
                },
            )
            .expect("verify_auto runs");
            (r, start.elapsed())
        };

        let (explicit, dt_x) = run(false);
        let (symbolic, dt_s) = run(true);

        eprintln!("\n=== R-F5.5d sysrst: explicit vs symbolic ===");
        eprintln!("explicit engine: {dt_x:?}   symbolic engine: {dt_s:?}");
        // The full sysrst design exceeds the symbolic bit-blast cap (no COI yet;
        // R-F5.6), so every property degrades to Skipped with the bit-cap reason
        // — GRACEFULLY, not a mid-construction OoM panic. That the run completes
        // (no panic) + reaches this assertion is the wiring + graceful-degradation
        // validation on real RTL.
        let mut sym_skipped_bitcap = 0;
        for (px, ps) in explicit.properties.iter().zip(symbolic.properties.iter()) {
            assert_eq!(px.name, ps.name, "property order aligns across engines");
            eprintln!(
                "  {}: explicit={:?}  symbolic={:?}",
                px.name, px.outcome, ps.outcome
            );
            if let VerifyOutcome::Skipped { reason } = &ps.outcome
                && reason.contains("register+input bits")
            {
                sym_skipped_bitcap += 1;
            }
        }
        eprintln!("symbolic bit-cap Skips: {sym_skipped_bitcap}");
        assert!(
            sym_skipped_bitcap >= 1,
            "the full sysrst design exceeds the symbolic bit-blast cap → every property \
             degrades to a Skipped (bit-cap) verdict GRACEFULLY (no OoM panic); the R-F5.6 \
             COI restriction is what lets the symbolic engine scale to real designs"
        );
    }

    /// D1.6 — the `--engine exact-symbolic` **surface route**, end-to-end through
    /// `verify_auto` on real OpenTitan RTL. A `@mununu_guarantee` annotation carries
    /// the headline `AG AF (bit_cnt_q == 0)` liveness property; `verify_auto` with
    /// `exact_symbolic: true` must route it through the exact full-state ROBDD MC
    /// (`exact_symbolic_verdict`) — the same engine `e2e_d1_uart_tx_exact_liveness_verdict`
    /// exercises directly — and return a **definite Violated** (with a stall lasso),
    /// where the predicate-cube path answers this ⊥ (no ranking for `AF`).
    ///
    /// This validates the CLI/API `--engine exact-symbolic` wiring, not the D1 thesis
    /// itself (that is the direct test's job): the annotation guarantee flows through
    /// scan → merge → the hoisted exact branch (which runs BEFORE cube seeding, so it
    /// is never gated behind a seeding skip). Regression for the frozen-register fix
    /// (2026-07-05): this asserted `Holds` while `bit_cnt_q` (a `uext`-aliased register)
    /// was silently frozen by the `next_funcs` keying bug; the true verdict is
    /// `Violated` (the counter can be held non-zero forever by a persistent write or a
    /// stalled tick).
    #[test]
    #[ignore = "requires slang + sv2v + Yosys (use the mununu-sva docker image); run with --ignored"]
    fn e2e_d1_6_verify_auto_exact_symbolic_route_uart_tx() {
        use std::path::PathBuf;
        let src_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/verify/m1_opentitan_uart_tx/source/uart_tx.sv");
        let uart_tx = std::fs::read_to_string(&src_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", src_path.display()));
        // Carry the AG AF liveness property as a source annotation — verify_auto's
        // scan_annotation_properties reads the ORIGINAL source (not the elaborated
        // output), parses the body via the mu-calculus parser, and merges it as
        // `ann_guarantee_0`.
        let annotated = format!(
            "// @mununu_guarantee nu X. ((mu Y. ((bit_cnt_q == 0) or [] Y)) and [] X)\n{uart_tx}"
        );
        let sources = vec![("uart_tx.sv".to_string(), annotated)];
        let yopts = YosysOptions {
            top: Some("uart_tx".to_string()),
            use_sv2v: true,
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                exact_symbolic: true,
                ..Default::default()
            },
        )
        .expect("verify_auto runs with --engine exact-symbolic");

        let guarantee = report
            .properties
            .iter()
            .find(|p| p.name.contains("ann_guarantee"))
            .unwrap_or_else(|| {
                panic!(
                    "the @mununu_guarantee property must appear in the report; got: {:?}",
                    report
                        .properties
                        .iter()
                        .map(|p| &p.name)
                        .collect::<Vec<_>>()
                )
            });
        eprintln!(
            "\n=== D1.6 verify_auto --engine exact-symbolic on uart_tx ===\n  {} ({}): {:?}",
            guarantee.name, guarantee.formula, guarantee.outcome
        );
        assert!(
            matches!(guarantee.outcome, VerifyOutcome::Violated { .. }),
            "AG AF (bit_cnt_q==0) must be decided Violated by the exact-symbolic route \
             through verify_auto (the counter can be held non-zero forever) — a false \
             Holds means the frozen-register `next_funcs` alias-keying bug regressed; \
             got {:?}",
            guarantee.outcome
        );
    }

    #[test]
    #[ignore = "requires slang + sv2v + yosys (mununu-sva image)"]
    fn e2e_reset_gated_sparse_fsm_inits_at_reset_value() {
        // Regression for the reset-gated async-reset init fix (reset_init.rs).
        // OpenTitan csrng_main_sm's FSM resets — via prim_sparse_fsm_flop's
        // ResetValue — to MainSmIdle = 6'b110111 = 55, a NON-ZERO sparse
        // encoding. Yosys `async2sync` lifts the async reset to a next-state mux
        // with no BTOR2 `init` line, and verify-auto pins the reset inactive.
        // Without `inject_reset_init` the model would power up at 0 (an ILLEGAL
        // sparse encoding → the FSM's `default` arm traps to MainSmError), so the
        // init-state probe `(state_q == 55)` would be VIOLATED. With the fix the
        // reset-gated model starts at MainSmIdle, so it HOLDS — the faithful
        // post-reset state, decided exactly by the exact-symbolic engine (which
        // reads the injected `init` line).
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
        let csrng = root.join("m2_opentitan_csrng_main_sm/source");
        let prim = root.join("m0_opentitan_prim_arbiter/source");
        let read = |p: PathBuf| {
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        // The @mununu_guarantee carries the init-state probe; the standard
        // prim_assert macros come from the M.0 fixture (the csrng dir's own
        // prim_assert.sv is the dummy variant).
        let annotated = format!(
            "// @mununu_guarantee (state_q == 55)\n{}",
            read(csrng.join("csrng_main_sm.sv"))
        );
        let sources = vec![
            ("csrng_main_sm.sv".to_string(), annotated),
            ("csrng_pkg.sv".to_string(), read(csrng.join("csrng_pkg.sv"))),
            (
                "prim_assert.sv".to_string(),
                read(prim.join("prim_assert.sv")),
            ),
            (
                "prim_assert_standard_macros.svh".to_string(),
                read(prim.join("prim_assert_standard_macros.svh")),
            ),
            (
                "prim_assert_sec_cm.svh".to_string(),
                read(prim.join("prim_assert_sec_cm.svh")),
            ),
            (
                "prim_flop_macros.sv".to_string(),
                read(prim.join("prim_flop_macros.sv")),
            ),
        ];
        let yopts = YosysOptions {
            top: Some("csrng_main_sm".to_string()),
            use_sv2v: true,
            additional_sources: sources[1..].to_vec(),
            ..Default::default()
        };
        let report = verify_auto(
            &sources,
            &yopts,
            &VerifyAutoOptions {
                exact_symbolic: true,
                ..Default::default()
            },
        )
        .expect("verify_auto runs exact-symbolic on csrng_main_sm");
        let probe = report
            .properties
            .iter()
            .find(|p| p.name.contains("ann_guarantee"))
            .expect("the @mununu_guarantee init probe is present");
        assert!(
            matches!(probe.outcome, VerifyOutcome::Holds),
            "reset-gated csrng must init at MainSmIdle (state_q==55), not the \
             power-on default 0 — the reset-init fix; got {:?} for `{}`",
            probe.outcome,
            probe.formula
        );
    }

    /// H.5-GR1 — the assume-guarantee liveness showcase on real OpenTitan RTL,
    /// decided by the exact-symbolic engine. The recoverability property
    /// `AG EF MainSmIdle` (`nu Y. ((mu X. ((state_q == 55) or <> X)) and [] Y)`) —
    /// "from every reachable state, can the FSM get back to idle?" — is a
    /// there-exists-a-path (`EF`) property SVA structurally cannot phrase. Its
    /// verdict flips on a single explicit environment assumption. With
    /// `local_escalate_i` FREE the verdict is VIOLATED — a local security escalation
    /// latches the FSM in the terminal `MainSmError` trap (SEC_CM design;
    /// `CsrngMainErrorStStable_A`), so idle is unreachable. Under the assumption
    /// `G !local_escalate_i` (`=0`) the verdict is HOLDS — with no escalation the FSM
    /// always cycles back to idle. Both verdicts are 2-valued definite (the exact
    /// engine has no ⊥). The example at
    /// `examples/verify/v8_csrng_escalation_recoverability/` reproduces this via the
    /// CLI. Regression-critical: the frozen-register bug (fixed) made BOTH sides a
    /// vacuous HOLDS, erasing the flip.
    #[test]
    #[ignore = "requires slang + sv2v + yosys (mununu-sva image); run with --ignored"]
    fn e2e_h5gr1_csrng_recoverability_escalation_flip() {
        use std::collections::HashMap;
        use std::path::PathBuf;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/verify");
        let csrng = root.join("m2_opentitan_csrng_main_sm/source");
        let prim = root.join("m0_opentitan_prim_arbiter/source");
        let read = |p: PathBuf| {
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
        };
        let outcome = |cfg: &[(&str, u64)]| -> VerifyOutcome {
            let sm = read(csrng.join("csrng_main_sm.sv"));
            let annotated = format!(
                "// @mununu_guarantee nu Y. ((mu X. ((state_q == 55) or <> X)) and [] Y)\n{sm}"
            );
            let sources = vec![
                ("csrng_main_sm.sv".to_string(), annotated),
                ("csrng_pkg.sv".to_string(), read(csrng.join("csrng_pkg.sv"))),
                (
                    "prim_assert.sv".to_string(),
                    read(prim.join("prim_assert.sv")),
                ),
                (
                    "prim_assert_standard_macros.svh".to_string(),
                    read(prim.join("prim_assert_standard_macros.svh")),
                ),
                (
                    "prim_assert_sec_cm.svh".to_string(),
                    read(prim.join("prim_assert_sec_cm.svh")),
                ),
                (
                    "prim_flop_macros.sv".to_string(),
                    read(prim.join("prim_flop_macros.sv")),
                ),
            ];
            let yopts = YosysOptions {
                top: Some("csrng_main_sm".to_string()),
                use_sv2v: true,
                additional_sources: sources[1..].to_vec(),
                ..Default::default()
            };
            let config_values: HashMap<String, u64> =
                cfg.iter().map(|(k, v)| (k.to_string(), *v)).collect();
            let report = verify_auto(
                &sources,
                &yopts,
                &VerifyAutoOptions {
                    exact_symbolic: true,
                    config_values,
                    ..Default::default()
                },
            )
            .expect("verify_auto exact-symbolic on csrng_main_sm");
            report
                .properties
                .iter()
                .find(|p| p.name.contains("ann_guarantee"))
                .map(|p| p.outcome.clone())
                .expect("the @mununu_guarantee recoverability property is present")
        };

        // No assumption: a security escalation wedges the FSM in MainSmError.
        let free = outcome(&[]);
        assert!(
            matches!(free, VerifyOutcome::Violated { .. }),
            "AG EF idle must be VIOLATED with local_escalate_i free (escalation → \
             MainSmError trap); got {free:?}"
        );
        // Under the no-escalation assumption: the FSM always recovers to idle.
        let assumed = outcome(&[("local_escalate_i", 0)]);
        assert!(
            matches!(assumed, VerifyOutcome::Holds),
            "AG EF idle must HOLD under the assumption G !local_escalate_i; got {assumed:?}"
        );
    }
}
