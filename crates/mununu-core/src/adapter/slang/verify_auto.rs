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
//! cells** only. An atom over a non-state signal (a primary input / pure
//! combinational output — e.g. an arbiter's `req_i` / `gnt_o`) has no cube
//! predicate; binding it would fall through to the evaluator's "unknown ⇒ false"
//! under-approx and silently produce a vacuous verdict. So a property whose atoms
//! are not all state-cell-resolvable is **Skipped** with a reason — never given a
//! misleading verdict. (Those combinational/IO properties want the bit-blast
//! path; a future increment routes them there.)

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
}

impl Default for VerifyAutoOptions {
    fn default() -> Self {
        Self {
            max_iterations: 16,
            must_edge_inference: MustEdgeInference::Off,
            gate_reset: true,
        }
    }
}

/// Auto-seeded predicates for one formula: simple `reg == value` specs, compound
/// `(name, expr)` pairs (relational / `!=` / boolean combinations), and the atom
/// strings that could NOT be seeded (reference a non-state signal).
#[derive(Debug, Clone, Default)]
struct Seeded {
    specs: Vec<PredicateSpec>,
    compounds: Vec<(String, PredicateExpr)>,
    unseedable: Vec<String>,
}

/// The minimal H.1 — derive cube predicates from a formula's `Node::Predicate`
/// atoms. Each predicate is named exactly the atom string (so the evaluator's
/// name-match binds the atom to its cube bit). An atom whose registers are not
/// all in `state_cells` is recorded as unseedable.
fn seed_from_formula(formula: &Formula, state_cells: &HashSet<String>) -> Seeded {
    let mut out = Seeded::default();
    let mut seen: HashSet<&str> = HashSet::new();
    for node in formula.nodes() {
        let Node::Predicate(atom) = node else {
            continue;
        };
        if !seen.insert(atom.as_str()) {
            continue;
        }
        match parse_predicate_expr(atom) {
            Ok(expr) => {
                if expr.registers().iter().any(|r| !state_cells.contains(r)) {
                    out.unseedable.push(atom.clone());
                    continue;
                }
                match &expr {
                    // Simple `reg == value` → a direct PredicateSpec cube bit.
                    PredicateExpr::Cmp {
                        register,
                        op: CmpOp::Eq,
                        value,
                    } => out.specs.push(PredicateSpec {
                        name: atom.clone(),
                        register: register.clone(),
                        value: *value,
                    }),
                    // Relational / `!=` / boolean combination → compound predicate.
                    _ => out.compounds.push((atom.clone(), expr)),
                }
            }
            // A bare identifier atom (`parse_predicate_expr` needs an operator):
            // a 1-bit boolean signal `sig` ≡ `sig == 1`. Seedable iff a state cell.
            Err(_) => {
                if state_cells.contains(atom) {
                    out.specs.push(PredicateSpec {
                        name: atom.clone(),
                        register: atom.clone(),
                        value: 1,
                    });
                } else {
                    out.unseedable.push(atom.clone());
                }
            }
        }
    }
    out
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
    referenced: &std::collections::BTreeSet<String>,
    init_values: &std::collections::HashMap<String, u64>,
) -> Option<String> {
    let compound_decls: Vec<serde_json::Value> = compounds
        .iter()
        // `expr` == `name` == the atom string, which `sidecar_compound_predicates`
        // re-parses via `parse_predicate_expr` (REL handles `reg == reg`).
        .map(|(name, _)| serde_json::json!({ "name": name, "expr": name }))
        .collect();
    let signals: Vec<serde_json::Value> = referenced
        .iter()
        .filter_map(|r| {
            init_values
                .get(r)
                .map(|v| serde_json::json!({ "name": r, "config_values": [v] }))
        })
        .collect();
    if compound_decls.is_empty() && signals.is_empty() {
        return None;
    }
    serde_json::to_string(&serde_json::json!({
        "module": "cegar",
        "source": "verify_auto.btor2",
        "compound_predicates": compound_decls,
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

/// Verify every translated SVA property in `sources` against the model, with no
/// sidecar. `sources` is `(file_name, content)`, the first being the primary.
pub fn verify_auto(
    sources: &[(String, String)],
    yosys_opts: &YosysOptions,
    opts: &VerifyAutoOptions,
) -> Result<AutoVerifyReport, AdapterError> {
    use crate::adapter::AdapterOptions;
    use crate::adapter::btor2::cegar::{
        CegarOptions, LiftStrategy, PredicateSource, cegar_refine_loop,
    };
    use crate::adapter::btor2::shadow::augment_with_past_shadows;
    use crate::adapter::yosys::sv_to_btor2_with_blackboxes;
    use crate::mu_calculus::Environment;
    use crate::mu_calculus::parser as mu_parser;
    use crate::mu_calculus::trit::Trit;

    let (primary_name, primary_content) = sources.first().ok_or_else(|| AdapterError {
        kind: AdapterErrorKind::ParseError,
        message: "adapter/slang/verify_auto: no SV sources provided".to_string(),
        location: None,
    })?;
    let _ = primary_name;

    // 1. Extract + translate the SVA. When reset-gating, the `disable iff`
    // guards are dropped from the formulas and the recognized reset signals are
    // reported (we pin them inactive in the lift below).
    let extraction = extract_sva_with_options(
        sources,
        &TranslateOptions {
            gate_reset: opts.gate_reset,
        },
    )?;

    let mut report = AutoVerifyReport {
        unsupported: extraction
            .unsupported
            .iter()
            .map(|u| (u.name.clone(), u.reason.clone()))
            .collect(),
        ..Default::default()
    };
    if extraction.translated.is_empty() {
        return Ok(report);
    }

    // Reset inputs to pin inactive (reset-gating) — applied to the lifted BTOR2
    // below via `pin_inputs_to_constants`. Empty when gating is off or no
    // `disable iff` reset was recognized.
    let reset_pins: Vec<(String, u64)> = if opts.gate_reset {
        extraction
            .reset_signals
            .iter()
            .map(|rs| (rs.signal.clone(), rs.inactive_value))
            .collect()
    } else {
        Vec::new()
    };

    // 2. SV → flattened BTOR2, then 3. augment the `__past` shadow flops. The
    // lift also reports modules it black-boxed (instantiated, no body) — those
    // are surfaced in `diagnostics.blackboxed_modules` so a cut FSM (the
    // `prim_sparse_fsm_flop`-class root cause) is visible, not silent.
    let (btor2, blackboxed_modules) = sv_to_btor2_with_blackboxes(primary_content, yosys_opts)
        .map_err(|mut e| {
            e.message = format!("verify_auto: SV → BTOR2: {}", e.message);
            e
        })?;
    report.diagnostics.blackboxed_modules = blackboxed_modules;
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

    // 4. Per property: seed → CEGAR → verdict.
    for t in &extraction.translated {
        let formula = match mu_parser::parse(&t.formula) {
            Ok(f) => f,
            Err(e) => {
                report.properties.push(PropertyVerdict {
                    name: t.name.clone(),
                    kind: t.kind,
                    formula: t.formula.clone(),
                    outcome: VerifyOutcome::Skipped {
                        reason: format!("formula failed to parse: {e:?}"),
                    },
                    seeded_predicates: Vec::new(),
                });
                continue;
            }
        };

        let seeded = seed_from_formula(&formula, &state_cells);
        if !seeded.unseedable.is_empty() {
            let reason = unseedable_skip_reason(&seeded.unseedable, &report.diagnostics);
            report.properties.push(PropertyVerdict {
                name: t.name.clone(),
                kind: t.kind,
                formula: t.formula.clone(),
                outcome: VerifyOutcome::Skipped { reason },
                seeded_predicates: Vec::new(),
            });
            continue;
        }
        let predicate_count = seeded.specs.len() + seeded.compounds.len();
        if predicate_count == 0 {
            report.properties.push(PropertyVerdict {
                name: t.name.clone(),
                kind: t.kind,
                formula: t.formula.clone(),
                outcome: VerifyOutcome::Skipped {
                    reason: "no state-cell predicate atoms to seed the cube".to_string(),
                },
                seeded_predicates: Vec::new(),
            });
            continue;
        }

        let mut seeded_names: Vec<String> = seeded.specs.iter().map(|s| s.name.clone()).collect();
        seeded_names.extend(seeded.compounds.iter().map(|(n, _)| n.clone()));

        // Compounds present ⇒ force the SmtAllPairs eager lift (the only
        // compound-aware path); `cegar_refine_loop` re-checks this.
        let has_compounds = !seeded.compounds.is_empty();
        let cegar_opts = CegarOptions {
            max_iterations: opts.max_iterations,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
            smart_uf_cap: true,
            lift_strategy: LiftStrategy::Eager,
            must_edge_inference: opts.must_edge_inference,
            may_edge_inference: if has_compounds {
                MayEdgeInference::SmtAllPairs
            } else {
                MayEdgeInference::Off
            },
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
        let adapter_options = AdapterOptions {
            sidecar_json: synth_sidecar_json(&seeded.compounds, &referenced, &init_values),
            ..Default::default()
        };
        // Env sized to the cube space: simple specs + the sidecar compounds
        // `cegar_refine_loop` appends.
        let env = Environment::new(1usize << predicate_count);

        // The design's initial cube index: evaluate every predicate at the
        // reset valuation, in the lift's bit order (simple specs first, then the
        // sidecar compounds `cegar_refine_loop` appends). The verdict AT THIS
        // cube is the property's answer — sidestepping the lift's `cube_0`
        // initial-state default (cube_is_admissible can't pin a compound).
        let init_val_u128: std::collections::HashMap<String, u128> = init_values
            .iter()
            .map(|(k, v)| (k.clone(), *v as u128))
            .collect();
        let mut init_cube = 0usize;
        let mut bit = 0u32;
        for s in &seeded.specs {
            if init_values.get(&s.register).copied().unwrap_or(0) == s.value {
                init_cube |= 1 << bit;
            }
            bit += 1;
        }
        for (_, expr) in &seeded.compounds {
            if expr.eval(&init_val_u128) {
                init_cube |= 1 << bit;
            }
            bit += 1;
        }

        let outcome = match cegar_refine_loop(
            &formula,
            &btor2,
            seeded.specs.clone(),
            &env,
            &adapter_options,
            &cegar_opts,
        ) {
            Ok(trace) => {
                let v = &trace.final_verdict;
                // The verdict at the reset cube is the property's verdict.
                match v.verdict_at(init_cube) {
                    Trit::True => VerifyOutcome::Holds,
                    Trit::False => {
                        let false_cells = (0..v.len())
                            .filter(|&i| v.verdict_at(i) == Trit::False)
                            .count();
                        VerifyOutcome::Violated { false_cells }
                    }
                    Trit::Unknown => {
                        let unknown_cells = (0..v.len())
                            .filter(|&i| v.verdict_at(i) == Trit::Unknown)
                            .count();
                        VerifyOutcome::Unknown { unknown_cells }
                    }
                }
            }
            Err(e) => VerifyOutcome::Skipped {
                reason: format!("CEGAR error: {}", e.message),
            },
        };

        report.properties.push(PropertyVerdict {
            name: t.name.clone(),
            kind: t.kind,
            formula: t.formula.clone(),
            outcome,
            seeded_predicates: seeded_names,
        });
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mu_calculus::parser as mu_parser;

    fn cells(names: &[&str]) -> HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn seeds_simple_state_predicate() {
        // `nu X. ((state == 5) && [] X)` over a state cell → one simple spec.
        let f = mu_parser::parse("nu X. ((state == 5) && [] X)").unwrap();
        let s = seed_from_formula(&f, &cells(&["state"]));
        assert_eq!(s.specs.len(), 1, "one simple reg==val spec");
        assert_eq!(s.specs[0].name, "state == 5");
        assert_eq!(s.specs[0].register, "state");
        assert_eq!(s.specs[0].value, 5);
        assert!(s.compounds.is_empty());
        assert!(s.unseedable.is_empty());
    }

    #[test]
    fn seeds_relational_predicate_as_compound() {
        // `$stable` shape: `state == state__past` → a compound (REL), both state cells.
        let f = mu_parser::parse("nu X. ((state == state__past) && [] X)").unwrap();
        let s = seed_from_formula(&f, &cells(&["state", "state__past"]));
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
        let json = synth_sidecar_json(&s.compounds, &referenced, &inits).expect("sidecar json");
        assert!(json.contains("compound_predicates"));
        assert!(json.contains("state == state__past"));
        assert!(json.contains("config_values"), "init pins present: {json}");
    }

    #[test]
    fn gates_atoms_over_non_state_signals() {
        // `gnt_o != 0` and `ready_i` over IO signals (not state cells) → unseedable.
        let f = mu_parser::parse("nu X. (((gnt_o != 0) || ready_i) && [] X)").unwrap();
        let s = seed_from_formula(&f, &cells(&["state_q"])); // neither IO signal is a state cell
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
        let s = seed_from_formula(&f, &cells(&["busy"]));
        assert_eq!(s.specs.len(), 1);
        assert_eq!(s.specs[0].name, "busy");
        assert_eq!(s.specs[0].value, 1, "bare boolean state signal ≡ sig == 1");
    }

    #[test]
    fn skip_reason_enriched_with_blackboxed_module_root_cause() {
        // A cut FSM (e.g. csrng's prim_sparse_fsm_flop) → the bare "non-state
        // signal" symptom is augmented with the actionable root cause.
        let diag = ModelDiagnostics {
            state_register_count: 0,
            blackboxed_modules: vec!["prim_sparse_fsm_flop".to_string()],
            gated_resets: Vec::new(),
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
        };
        let reason = unseedable_skip_reason(&["gnt_o != 0".to_string()], &diag);
        assert!(reason.contains("gnt_o != 0"));
        assert!(!reason.contains("Root cause"), "no spurious hint: {reason}");
    }

    #[test]
    fn no_sidecar_when_nothing_to_emit() {
        let empty_refs = std::collections::BTreeSet::new();
        let empty_inits = std::collections::HashMap::new();
        assert!(synth_sidecar_json(&[], &empty_refs, &empty_inits).is_none());
    }

    #[test]
    fn empty_sources_errors() {
        let err = verify_auto(&[], &YosysOptions::default(), &VerifyAutoOptions::default())
            .expect_err("no sources");
        assert_eq!(err.kind, AdapterErrorKind::ParseError);
    }

    #[test]
    #[ignore = "requires slang + sv2v + Yosys + z3; run with --ignored"]
    fn e2e_reset_gating_turns_a_skip_into_a_verdict() {
        // A 2-bit FSM cycling 0→1→2→0, with an active-low-reset-guarded
        // assertion `disable iff (!rst_n) state != 3`. With reset-gating ON
        // (default) the guard is dropped, rst_n is pinned inactive, and the
        // body `state != 3` is a state-cell property → HOLDS. With gating OFF
        // the `!rst_n` IO atom is unbindable → SKIPPED. This is the headline
        // before/after that the reset-gating fix delivers.
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

        // Reset-gating OFF: the `!rst_n` IO atom is unbindable → SKIPPED.
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
            matches!(ungated.properties[0].outcome, VerifyOutcome::Skipped { .. }),
            "without gating the reset atom forces a SKIP; got {:?}",
            ungated.properties[0].outcome
        );
        assert!(ungated.diagnostics.gated_resets.is_empty());
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

        // Honest invariant: the real OpenTitan SVA is in-fragment and
        // translates (the `$stable` / `|=>` / `|->` + enum + disable-iff forms).
        // Whether a property reaches a DEFINITE verdict depends on whether its
        // state survives the lift — the diagnostics above attribute any SKIP.
        assert!(
            report.properties.len() >= 2,
            "both csrng ASSERTs (CsrngMainErrorStStable_A, CsrngMainErrorOutput_A) translate"
        );
        // The csrng FSM is wrapped in `prim_sparse_fsm_flop` (body not in the
        // source set), so it is cut and the lift has no state registers. The
        // diagnostic must NAME the cut module (undefined-module-cell detection)
        // rather than only reporting "no state registers" — that is the
        // actionable root cause (provide the flop's source).
        assert!(
            report
                .diagnostics
                .blackboxed_modules
                .iter()
                .any(|m| m.contains("prim_sparse_fsm_flop")),
            "the cut prim_sparse_fsm_flop should be named in diagnostics; got {:?}",
            report.diagnostics.blackboxed_modules
        );
        // Reset-gating fires on the macro's `disable iff (rst_ni)`.
        assert_eq!(
            report.diagnostics.gated_resets,
            vec!["rst_ni=1".to_string()]
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
}
