//! Verification execution planner (verification-execution-planner Phase 3 / roadmap P2.1).
//!
//! **P2.1a — the planner seam.** A [`VerificationTask`] is turned by [`plan`] into a
//! [`PhysicalPlan`] (an ordered set of engine operators + a scheduling mode), which
//! [`execute`] drives — delegating each operator to the existing single-engine
//! [`verify_auto`](crate::adapter::slang::verify_auto::verify_auto) pass and merging under
//! the existing runtime soundness guard
//! ([`merge_portfolio_reports`](crate::adapter::slang::verify_auto::merge_portfolio_reports)).
//!
//! This re-expresses today's `verify_auto` portfolio dispatch as an explicit plan/execute
//! pair **without changing any verdict** — it is verdict-equivalent by construction (same
//! operators, same precision order, same merge, same early-exit; the driver body is
//! relocated verbatim from the former `verify_auto_portfolio`). What it buys is the *seam*:
//! one place where the "which engines, in what order/mode" decision lives, ready for the
//! later phases to make cost-based.
//!
//! Not yet here (deliberately): cost-based operator selection off `ModelFacts` (P2.2), the
//! ⊥-reactive re-plan edges that generalise `escalate_bottom` (P2.1b), and the soundness
//! plan-invariants — cube-νμ exact-corroboration, property-class transfer gating (P2.1c).
//!
//! See `.claude/plans/verification-execution-planner.md` §4.2/§4.5.

use crate::adapter::slang::verify_auto::{
    AutoVerifyReport, NoteLevel, PortfolioMode, VerificationNote, VerifyAutoOptions, VerifyOutcome,
    escalate_bottom, merge_portfolio_reports, outcome_definite, rescue_skipped_via_exact,
    verify_auto,
};
use crate::adapter::yosys::YosysOptions;
use crate::adapter::{AdapterError, AdapterErrorKind};

/// One physical engine operator — a single-engine [`verify_auto`] pass.
///
/// P2.1a covers the μ-calculus engine leaves (the precision ladder). Abstraction transforms
/// (cutpoint / predicate-seed) and the ⊥-reactive rescue edges become operators in later
/// increments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineOp {
    /// Mirrors the CLI/API `--engine` label (the value the merge keys provenance on).
    pub label: &'static str,
    /// Select the may/must cube ("symbolic") engine.
    pub symbolic_engine: bool,
    /// Select the full-state exact BDD ("exact-symbolic") engine.
    pub exact_symbolic: bool,
}

/// The physical plan — the ordered engine operators plus how to schedule them.
#[derive(Debug, Clone)]
pub struct PhysicalPlan {
    /// Operators in precision order (exact first).
    pub ops: Vec<EngineOp>,
    /// `Sequential` = run in order, merge after each, stop when no ⊥ property remains (the
    /// budget win); `Parallel` = run all concurrently, merge once. A single-op plan runs the
    /// same under either mode.
    pub mode: PortfolioMode,
}

/// The logical verification task — the planner's input.
///
/// **P2.1a form:** the *pre-lift bundle* the SV `verify-auto` entry already holds (sources +
/// lift options + verify options). It migrates to the plan doc's
/// `(Sts, MuFormula, posture, ModelFacts, hints, budget)` form when the common-IR hub lands
/// (Phase 2); the seam is introduced now so later phases extend this struct, not the call
/// graph. Borrows its inputs — cheap to build at the dispatch point.
pub struct VerificationTask<'a> {
    /// `(file_name, content)`, the first being the primary source.
    pub sources: &'a [(String, String)],
    /// The SV → BTOR2 lift options.
    pub yosys_opts: &'a YosysOptions,
    /// The verify-auto options (engine intent, budgets, rescue gates, hints).
    pub opts: &'a VerifyAutoOptions,
}

/// The μ-calculus engine precision ladder (exact → symbolic-cube → explicit-CEGAR) — the
/// order the portfolio has always used. Exact first: 2-valued, never-⊥ within its bit cap,
/// richest witness; the two cube engines are complementary fallbacks the parity differential
/// proved never contradict it. (Relocated verbatim from `verify_auto`'s `PORTFOLIO_ENGINES`.)
const PRECISION_LADDER: [EngineOp; 3] = [
    EngineOp {
        label: "exact-symbolic",
        symbolic_engine: false,
        exact_symbolic: true,
    },
    EngineOp {
        label: "symbolic",
        symbolic_engine: true,
        exact_symbolic: false,
    },
    EngineOp {
        label: "explicit",
        symbolic_engine: false,
        exact_symbolic: false,
    },
];

/// Rule-based planner (P2.1a): reproduce today's fixed engine selection. A portfolio intent
/// emits the full precision ladder in the requested mode; a single-engine intent emits just
/// that operator. (P2.2 makes this cost-based off `ModelFacts` — cone-vs-cap, property-class
/// → verdict requirement.)
pub fn plan(task: &VerificationTask) -> PhysicalPlan {
    match task.opts.portfolio {
        Some(mode) => PhysicalPlan {
            ops: PRECISION_LADDER.to_vec(),
            mode,
        },
        None => PhysicalPlan {
            ops: vec![EngineOp {
                label: single_engine_label(task.opts),
                symbolic_engine: task.opts.symbolic_engine,
                exact_symbolic: task.opts.exact_symbolic,
            }],
            mode: PortfolioMode::Sequential,
        },
    }
}

/// The `--engine` label a single-engine option pair resolves to (exact takes precedence, as
/// in `engine_selection`).
fn single_engine_label(opts: &VerifyAutoOptions) -> &'static str {
    match (opts.symbolic_engine, opts.exact_symbolic) {
        (_, true) => "exact-symbolic",
        (true, false) => "symbolic",
        (false, false) => "explicit",
    }
}

/// Execute the plan: run each engine operator as a single-engine [`verify_auto`] pass and
/// merge under the runtime soundness guard. Relocated verbatim from the former
/// `verify_auto_portfolio` — behaviour-identical, so the portfolio's verdicts are unchanged.
pub fn execute(
    plan: &PhysicalPlan,
    task: &VerificationTask,
) -> Result<AutoVerifyReport, AdapterError> {
    // A single-engine option set derived from `task.opts` with the portfolio disabled and the
    // two engine flags forced to this operator's pair (so the inner call runs exactly one
    // engine and does not recurse).
    let mk_opts = |op: &EngineOp| {
        let mut o = task.opts.clone();
        o.portfolio = None;
        o.symbolic_engine = op.symbolic_engine;
        o.exact_symbolic = op.exact_symbolic;
        o
    };

    match plan.mode {
        PortfolioMode::Sequential => {
            let mut runs: Vec<(&str, Result<AutoVerifyReport, AdapterError>)> = Vec::new();
            for op in &plan.ops {
                runs.push((
                    op.label,
                    verify_auto(task.sources, task.yosys_opts, &mk_opts(op)),
                ));
                // Early-exit as soon as the MERGE so far leaves no ⊥ property (the budget win).
                let merged = merge_portfolio_reports(&runs, plan.mode);
                if let Ok(rep) = &merged
                    && rep
                        .properties
                        .iter()
                        .all(|p| outcome_definite(&p.outcome).is_some())
                {
                    return merged;
                }
            }
            merge_portfolio_reports(&runs, plan.mode)
        }
        PortfolioMode::Parallel => {
            // Scoped threads borrow the task's sources / yosys_opts; each owns its option clone.
            let runs: Vec<(&str, Result<AutoVerifyReport, AdapterError>)> =
                std::thread::scope(|scope| {
                    let handles: Vec<(&str, _)> = plan
                        .ops
                        .iter()
                        .map(|op| {
                            let o = mk_opts(op);
                            (
                                op.label,
                                scope.spawn(move || verify_auto(task.sources, task.yosys_opts, &o)),
                            )
                        })
                        .collect();
                    handles
                        .into_iter()
                        .map(|(label, h)| {
                            let r = h.join().unwrap_or_else(|_| {
                                Err(AdapterError {
                                    kind: AdapterErrorKind::IrConsistencyError,
                                    message: format!("portfolio: engine `{label}` panicked"),
                                    location: None,
                                })
                            });
                            (label, r)
                        })
                        .collect()
                });
            merge_portfolio_reports(&runs, plan.mode)
        }
    }
}

/// The planner's reactive **re-plan** step (verification-execution-planner §4.5 step 4 /
/// roadmap P2.1b) — when the main-path plan ([`execute`]) leaves a property `⊥`, feed the
/// reason back and route it to a sound reduction. The re-plan **edges**, dispatched by
/// property class (each gated by its own `rescue_bottom_*` opt, firing only on its shape):
///
/// - **safety** `AG`-invariant → the reachability portfolio (reduce to a `bad`-monitor);
/// - **box-AF response** `AG(a→AF b)` → liveness-to-safety (l2s) → the portfolio;
/// - **νμ recoverability** `AG EF good` → exact+COI (reset-pinned) or cube+`smt-hyper-must`.
///
/// P2.1b routes the re-plan **entry** through the planner (so both halves of the
/// orchestration — main-path [`plan`]/[`execute`] and this reactive step — are planner-owned)
/// while **delegating the edge application** to the existing rescue subsystem
/// ([`escalate_bottom`]). Verdict-equivalent. P2.1c adds the soundness plan-invariants here
/// (cube-νμ exact-corroboration; property-class-aware transfer reporting).
pub fn replan(
    report: &mut AutoVerifyReport,
    design_btor2: &str,
    reset_pinned: bool,
    opts: &VerifyAutoOptions,
) -> Vec<VerificationNote> {
    escalate_bottom(report, design_btor2, reset_pinned, opts)
}

/// A **composed** recoverability plan — the point of a *planner*: mechanisms combine. It runs the
/// SOUND, transferable levers as one plan on the (caller-scoped) model:
///
///   exact-first (COI-shrunk) → cube + ranking + guard-atoms + **Craig** on the same model.
///
/// Every verdict it returns TRANSFERS to the design as given: a definite exact verdict, or a
/// definite 3-valued-KMTS verdict (sound at every alternation depth incl. νμ, Bruns–Godefroid).
///
/// **No auto-input-pinning (removed 2026-07-26 — it was unsound).** An earlier version config-pinned
/// the in-cone free inputs to 0 as a "combination component" and returned the pinned verdict. That is
/// UNSOUND for `AG EF`: recoverability is **not monotone under input-restriction**. Pinning an input
/// to a constant restricts BOTH the reachable states (weakening the outer `AG`) AND the available
/// recovery paths (strengthening the inner `EF`), so neither direction transfers —
/// - a pinned **HOLDS** omits every other input valuation (holds only for the pins), and
/// - a pinned **VIOLATED** can be spurious, because pinning removes the very recovery transitions the
///   inputs would steer through.
///
/// Measured on the wall-class set: the only ⊥→"decided" case the input-pin produced was `staller` —
/// a genuinely VIOLATED design whose stall the input-pin *hid*, yielding a non-transferable scoped
/// HOLDS that (had the scope been dropped) is a spurious verdict. Auto-config-value was already 0
/// full-space payoff. A caller wanting an operational sub-question pins RESET itself and passes the
/// scoped model in — reset-gating is sound and is the caller's scoping to make, not this plan's to
/// apply silently to free inputs.
pub fn solve_recoverability_combined(
    design_btor2: &str,
    target: &str,
) -> Result<crate::verdict::PropertyVerdict, String> {
    use crate::adapter::btor2::cegar::PredicateSource;
    use crate::adapter::recoverability::{
        verify_recoverability, verify_recoverability_scalable_with_source,
    };
    use crate::verdict::PropertyVerdict;

    // exact-first (with COI) — the strongest sound oracle; then the scalable cube path, which itself
    // composes cube + ranking + guard-atoms + Craig. Both transfer to the model as given.
    let exact_first = verify_recoverability(design_btor2, target);
    if matches!(
        exact_first,
        Ok(PropertyVerdict::Holds | PropertyVerdict::Violated)
    ) {
        return exact_first;
    }
    verify_recoverability_scalable_with_source(
        design_btor2,
        target,
        &[],
        PredicateSource::CraigInterpolation,
    )
}

/// The companion re-plan edge for cube-**Skipped** (not `⊥`) properties: an atom-less modal
/// formula the cube cannot seed gets one full-state exact-symbolic attempt, upgraded on a
/// definite verdict (lever b). Gated to the cube path under reset-gating. Delegates to the
/// existing [`rescue_skipped_via_exact`] — verdict-equivalent.
pub fn rescue_skipped(
    report: &mut AutoVerifyReport,
    design_btor2: &str,
    opts: &VerifyAutoOptions,
) -> Vec<VerificationNote> {
    let mut notes = rescue_skipped_via_exact(report, design_btor2, opts);
    // P2.2a — the first ModelFacts consumer: an actionable diagnostic for what remains
    // Skipped. Verdict-equivalent (a note, never a verdict change); P2.2b will *act* on the
    // same cone-vs-cap facts to auto-pin and re-decide.
    notes.extend(skip_over_cap_notes(report, design_btor2));
    notes
}

/// P2.2a/b — **residual-register-aware** actionable Skip diagnostic (the `ModelFacts`
/// consumer). For each property STILL Skipped after the exact attempt, use `ModelFacts` to
/// check whether the exact engine bailed because the property's cone exceeds the bit cap
/// (`cone_bits > cap`); if so, attach a note naming the cone width and the cap.
///
/// The **key fact** (P2.2b, measure-first 2026-07-26) is the *residual* — the register bits
/// that remain even after pinning **every** in-cone free input
/// (`residual = cone_bits − Σ pinnable_input_widths`). This is what decides whether
/// `--config-value` pinning can actually help:
///
/// - `residual ≤ cap` — **input-inflated** cone: pinning the in-cone inputs drops the cone to
///   ~`residual` bits ≤ cap, so `--config-value SIGNAL=VALUE` (a scoped verdict) decides it.
///   The note lists the pinnable inputs and how far pinning gets.
/// - `residual > cap` — **register-dominated** cone: even pinning ALL in-cone inputs leaves
///   `residual` register bits over the cap, so `--config-value` **cannot** decide it. The note
///   says so and points at the real levers (structural compression: one-hot re-encode /
///   counter abstraction; or cutpoint, posture-permitting).
///
/// A measure-first sweep (i2c 122 / gost 360 / present_cipher 144 residual register bits, all
/// above the 64-bit cap) showed the corpus's over-cap cones are register-dominated — so the
/// earlier "just pin the wide inputs" advice over-promised. This diagnostic tells the truth
/// per property instead of assuming input-inflation. Verdict-equivalent — a diagnostic, never
/// a verdict change.
fn skip_over_cap_notes(report: &AutoVerifyReport, design_btor2: &str) -> Vec<VerificationNote> {
    use crate::adapter::btor2::model_facts::ModelFacts;
    use crate::adapter::btor2::symbolic_bitblast::formula_seed_atoms;

    let Ok(file) = crate::adapter::btor2::parser::parse(design_btor2) else {
        return Vec::new();
    };
    let facts = ModelFacts::new(&file);
    let mut notes = Vec::new();
    for prop in &report.properties {
        if !matches!(prop.outcome, VerifyOutcome::Skipped { .. }) {
            continue;
        }
        let Ok(formula) = crate::mu_calculus::parser::parse(&prop.formula) else {
            continue;
        };
        let atoms = formula_seed_atoms(&formula);
        let (cone_bits, cap) = facts.cone_vs_cap(&atoms);
        if cone_bits <= cap {
            // Skipped for another reason (atom-less cube, unsupported op) — not a cap issue;
            // leave it to the existing Skip provenance.
            continue;
        }
        let inputs = facts.pinnable_cone_inputs(&atoms);
        let input_bits: u32 = inputs.iter().map(|i| i.width).sum();
        // The register bits that remain even after pinning EVERY in-cone input — the true test
        // of whether config-value pinning can bring the cone under the cap.
        let residual = cone_bits.saturating_sub(input_bits);
        let items: Vec<String> = inputs
            .iter()
            .take(8)
            .map(|i| format!("{}({} bits)", i.name, i.width))
            .collect();
        let detail = if residual <= cap {
            // Input-inflated: pinning these inputs fits the cap → config-value decides.
            format!(
                "The exact engine bit-blasts a property's cone-of-influence; this cone exceeds \
                 the bit cap, so it bailed to Skipped. The cone is INPUT-inflated: pinning the \
                 {} in-cone free input(s) below ({input_bits} bits) to representative constants \
                 (`--config-value SIGNAL=VALUE`) drops the cone to ~{residual} register bits (≤ \
                 the {cap}-bit cap), so exact decides — a verdict scoped to the pinned \
                 configuration.",
                inputs.len()
            )
        } else {
            // Register-dominated: even pinning all inputs leaves > cap register bits.
            format!(
                "The exact engine bit-blasts a property's cone-of-influence; this cone exceeds \
                 the bit cap, so it bailed to Skipped. This cone is REGISTER-dominated: even \
                 pinning ALL {} in-cone free input(s) ({input_bits} bits) leaves ~{residual} \
                 register bits (still > the {cap}-bit cap), so `--config-value` CANNOT bring it \
                 under the cap. The decidable lever here is sound register reduction — \
                 structural compression (one-hot re-encode, counter abstraction) or a cutpoint \
                 (safety-posture only; unsound for νμ) — not input concretization.",
                inputs.len()
            )
        };
        notes.push(VerificationNote {
            kind: "skip-over-cap-reason".into(),
            level: NoteLevel::ScopeCaveat,
            summary: format!(
                "`{}`: Skipped — the exact cone is {cone_bits} register+input bits (> the \
                 {cap}-bit cap); {} even fully input-pinned ({residual} residual register \
                 bits {} {cap}).",
                prop.name,
                if residual <= cap {
                    "config-value decidable"
                } else {
                    "register-dominated"
                },
                if residual <= cap { "≤" } else { ">" },
            ),
            detail,
            items,
        });
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts_with(portfolio: Option<PortfolioMode>, sym: bool, exact: bool) -> VerifyAutoOptions {
        VerifyAutoOptions {
            portfolio,
            symbolic_engine: sym,
            exact_symbolic: exact,
            ..Default::default()
        }
    }

    #[test]
    fn plan_portfolio_sequential_emits_the_precision_ladder() {
        let o = opts_with(Some(PortfolioMode::Sequential), false, false);
        let sources: Vec<(String, String)> = vec![];
        let yopts = YosysOptions::default();
        let task = VerificationTask {
            sources: &sources,
            yosys_opts: &yopts,
            opts: &o,
        };
        let p = plan(&task);
        assert_eq!(p.mode, PortfolioMode::Sequential);
        assert_eq!(
            p.ops.iter().map(|e| e.label).collect::<Vec<_>>(),
            vec!["exact-symbolic", "symbolic", "explicit"],
            "portfolio must plan the exact-first precision ladder"
        );
    }

    #[test]
    fn plan_portfolio_parallel_keeps_the_ladder_and_mode() {
        let o = opts_with(Some(PortfolioMode::Parallel), false, false);
        let sources: Vec<(String, String)> = vec![];
        let yopts = YosysOptions::default();
        let task = VerificationTask {
            sources: &sources,
            yosys_opts: &yopts,
            opts: &o,
        };
        let p = plan(&task);
        assert_eq!(p.mode, PortfolioMode::Parallel);
        assert_eq!(p.ops.len(), 3);
    }

    #[test]
    fn plan_single_engine_emits_one_op() {
        // exact-symbolic
        let sources: Vec<(String, String)> = vec![];
        let yopts = YosysOptions::default();
        for (sym, exact, label) in [
            (false, true, "exact-symbolic"),
            (true, false, "symbolic"),
            (false, false, "explicit"),
        ] {
            let o = opts_with(None, sym, exact);
            let task = VerificationTask {
                sources: &sources,
                yosys_opts: &yopts,
                opts: &o,
            };
            let p = plan(&task);
            assert_eq!(p.ops.len(), 1, "single-engine intent → one operator");
            assert_eq!(p.ops[0].label, label);
        }
    }

    // ---- P2.2b: residual-register-aware over-cap Skip diagnostic --------------------------

    fn skipped_report(name: &str, formula: &str) -> AutoVerifyReport {
        use crate::adapter::slang::translate::SvaKind;
        use crate::adapter::slang::verify_auto::PropertyVerdict;
        AutoVerifyReport {
            properties: vec![PropertyVerdict {
                name: name.into(),
                kind: SvaKind::Assert,
                formula: formula.into(),
                outcome: VerifyOutcome::Skipped {
                    reason: "cone too wide".into(),
                },
                seeded_predicates: Vec::new(),
                counterexample: None,
            }],
            unsupported: Vec::new(),
            diagnostics: Default::default(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn skip_note_register_dominated_says_config_value_cannot_help() {
        // A 70-bit self-incrementing register: the property cone is 70 bits, ALL register
        // (no in-cone inputs), so residual = 70 > the 64-bit cap — pinning inputs can't help.
        let btor2 = "\
1 sort bitvec 70
2 sort bitvec 1
3 state 1 big
4 one 1
5 add 1 3 4
6 next 1 3 5
";
        let report = skipped_report("p_reg", "mu X.(big == 0 || <> X)");
        let notes = skip_over_cap_notes(&report, btor2);
        assert_eq!(
            notes.len(),
            1,
            "an over-cap Skipped property gets one diagnostic"
        );
        let n = &notes[0];
        assert!(
            n.summary.contains("register-dominated"),
            "summary must flag register-domination: {}",
            n.summary
        );
        assert!(
            n.detail.contains("CANNOT") && n.detail.contains("register reduction"),
            "detail must say config-value cannot help + point at register reduction: {}",
            n.detail
        );
    }

    #[test]
    fn skip_note_input_inflated_points_at_config_value() {
        // A 70-bit INPUT feeding a 1-bit register: cone = 71 (1 register + 70 input), so
        // residual = 1 ≤ the 64-bit cap — pinning the input via --config-value decides it.
        let btor2 = "\
1 sort bitvec 70
2 sort bitvec 1
3 input 1 wide
4 state 2 flag
5 zero 1
6 eq 2 3 5
7 next 2 4 6
";
        let report = skipped_report("p_io", "mu X.(flag == 1 || <> X)");
        let notes = skip_over_cap_notes(&report, btor2);
        assert_eq!(
            notes.len(),
            1,
            "an over-cap Skipped property gets one diagnostic"
        );
        let n = &notes[0];
        assert!(
            n.summary.contains("config-value decidable"),
            "summary must flag config-value decidability: {}",
            n.summary
        );
        assert!(
            n.detail.contains("--config-value") && n.detail.contains("INPUT-inflated"),
            "detail must point at --config-value on an input-inflated cone: {}",
            n.detail
        );
    }
}
