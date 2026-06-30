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
//! What is still **Skipped** (never given a misleading verdict): an atom over a
//! pure **combinational output** (an `eq`/`or` function of state like csrng's
//! `main_sm_err_o` — case 4 of the design doc; needs the signal-node mapping a
//! follow-up adds), and an input inside a *compound* (the compound SMT branch
//! reads state BVs, not `view.inputs`). Those properties are reported Skipped
//! with a reason — binding them would fall through to the evaluator's
//! "unknown ⇒ false" under-approx and silently produce a vacuous verdict.

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
    /// Flop-primitive modules that were cut (instantiated with no body) and for
    /// which verify-auto auto-injected a behavioral stub so the register
    /// survives the lift (H.C — e.g. OpenTitan's `prim_sparse_fsm_flop`). The
    /// stub is an exact behavioral model of the flop datapath; auto-injection is
    /// reported here so it is never silent.
    pub auto_provided_stubs: Vec<String>,
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
    /// When `true` (default), if the first lift cuts a known flop primitive
    /// (instantiated with no body — e.g. OpenTitan's `prim_sparse_fsm_flop`),
    /// verify-auto injects a behavioral stub for it and re-lifts, so the
    /// register survives and its state atoms become bindable (H.C). The stub is
    /// an exact behavioral model of the flop datapath; the auto-injection is
    /// reported in `diagnostics.auto_provided_stubs`. Set `false` to leave cut
    /// flops cut (reported in `blackboxed_modules`).
    pub auto_stub_flops: bool,
}

impl Default for VerifyAutoOptions {
    fn default() -> Self {
        Self {
            max_iterations: 16,
            must_edge_inference: MustEdgeInference::Off,
            gate_reset: true,
            auto_stub_flops: true,
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
) -> Seeded {
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
                // DEFERRED (sound SKIP): a combinational function of a FREE INPUT
                // (`trigger_active = !trigger_i`). Its next-cycle value is
                // `f(state_next, next_input)`, so as a cube TARGET it needs the
                // nested `∀ i'` the uniform must does not yet build (H.U.0 finding);
                // a free cube dimension would fabricate must-edges. SKIP until that
                // lands, never a misleading verdict.
                Some(CombKind::InputDependent) | None => out.unseedable.push(atom.to_string()),
            }
        }
    };
    for node in formula.nodes() {
        let Node::Predicate(atom) = node else {
            continue;
        };
        if !seen.insert(atom.as_str()) {
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
                // Relational / `!=` / boolean combination → compound predicate.
                // The compound SMT branch reads state BVs only, so every
                // referenced register must be a state cell.
                _ => {
                    if expr.registers().iter().all(|r| is_state(r)) {
                        out.compounds.push((atom.clone(), expr));
                    } else {
                        out.unseedable.push(atom.clone());
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
    referenced: &std::collections::BTreeSet<String>,
    init_values: &std::collections::HashMap<String, u64>,
) -> Option<String> {
    let compound_decls: Vec<serde_json::Value> = compounds
        .iter()
        // `expr` == `name` == the atom string, which `sidecar_compound_predicates`
        // re-parses via `parse_predicate_expr` (REL handles `reg == reg`).
        .map(|(name, _)| serde_json::json!({ "name": name, "expr": name }))
        .collect();
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
    let (mut btor2, mut blackboxed_modules) =
        sv_to_btor2_with_blackboxes(primary_content, yosys_opts).map_err(|mut e| {
            e.message = format!("verify_auto: SV → BTOR2: {}", e.message);
            e
        })?;
    // H.C — if the first lift cut a known flop primitive, inject a behavioral
    // stub for it and re-lift so the register survives (e.g. OpenTitan's
    // `prim_sparse_fsm_flop`). Only stubs ACTUALLY-cut modules (no collision
    // with designs that provide their own body).
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
            report.diagnostics.auto_provided_stubs = stubs
                .iter()
                .map(|(name, _)| name.trim_end_matches(".sv").to_string())
                .collect();
        }
    }
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

        let seeded = seed_from_formula(&formula, resolves_to_state, is_input, combinational_kind);
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
        if predicate_count == 0 && seeded.derived.is_empty() {
            report.properties.push(PropertyVerdict {
                name: t.name.clone(),
                kind: t.kind,
                formula: t.formula.clone(),
                outcome: VerifyOutcome::Skipped {
                    reason: "no state-cell / combinational predicate atoms to seed the cube"
                        .to_string(),
                },
                seeded_predicates: Vec::new(),
            });
            continue;
        }

        let mut seeded_names: Vec<String> = seeded.specs.iter().map(|s| s.name.clone()).collect();
        seeded_names.extend(seeded.compounds.iter().map(|(n, _)| n.clone()));
        seeded_names.extend(seeded.derived.iter().map(|d| d.name.clone()));

        // Compounds OR free inputs ⇒ force the SmtAllPairs eager lift.
        // Compounds: the only compound-aware may path. Free inputs (H.B): the
        // sampling may path is state-oriented (it builds a canonical
        // representative over state registers and ignores `view.inputs`), so it
        // cannot realise the source-pin / target-free input shape — only the
        // SmtAllPairs seam (over `build_register_nid_map_with_inputs`) can.
        // `cegar_refine_loop` re-checks the compound gate.
        let has_compounds = !seeded.compounds.is_empty();
        let has_inputs = !seeded.input_registers.is_empty();
        // H.E — derived combinational predicates are labeled per cube by the
        // SMT `combinational_labels` pass, which also requires the SmtAllPairs
        // eager lift (precise state edges + the encoded view).
        let has_derived = !seeded.derived.is_empty();
        // H.U.2 — a combinational-of-state spec (register present in
        // `combinational_nid`, not a state cell) is a cube dimension over a
        // *determined function of state*. The sampling may-path is state-register
        // oriented (its canonical representative cannot realise a combinational
        // node's value), so it MUST use the SmtAllPairs seam — exactly like
        // compounds and free inputs.
        let has_combinational = seeded
            .specs
            .iter()
            .any(|s| combinational_nid.contains_key(&s.register));
        let cegar_opts = CegarOptions {
            max_iterations: opts.max_iterations,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
            smart_uf_cap: true,
            lift_strategy: LiftStrategy::Eager,
            must_edge_inference: opts.must_edge_inference,
            may_edge_inference: if has_compounds || has_inputs || has_derived || has_combinational {
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
            sidecar_json: synth_sidecar_json(
                &seeded.compounds,
                &seeded.derived,
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
        let s = seed_from_formula(&f, |n| cells(&["state"]).contains(n), |_| false, |_| None);
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
        let s = seed_from_formula(
            &f,
            |n| cells(&["state", "state__past"]).contains(n),
            |_| false,
            |_| None,
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
            synth_sidecar_json(&s.compounds, &[], &referenced, &inits).expect("sidecar json");
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
        let s = seed_from_formula(&f, |n| cells(&["state_q"]).contains(n), |_| false, |_| None);
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
        let s = seed_from_formula(&f, |n| cells(&["busy"]).contains(n), |_| false, |_| None);
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
    fn h_b_input_inside_compound_is_unseedable() {
        // An input inside a compound (`!=`, relational, boolean) is NOT admitted
        // — the compound SMT branch reads state BVs only, not `view.inputs`. So
        // `cfg_enable_i != 0` (a Ne, routed to the compound branch) over an
        // input is unseedable even though the same atom as `== 0` would seed.
        let f = mu_parser::parse("nu X. ((cfg_enable_i != 0) && [] X)").unwrap();
        let s = seed_from_formula(
            &f,
            |_| false,
            |n| cells(&["cfg_enable_i"]).contains(n),
            |_| None,
        );
        assert!(s.specs.is_empty());
        assert!(s.compounds.is_empty());
        assert!(
            s.unseedable.contains(&"cfg_enable_i != 0".to_string()),
            "input inside a compound is unseedable: {:?}",
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
        );
        assert_eq!(s.specs.len(), 1, "a cube dimension");
    }

    #[test]
    fn h_e_input_dependent_combinational_is_skipped_soundly() {
        // `trigger_active = !trigger_i` — combinational of a FREE INPUT. DEFERRED:
        // neither a derived state-cube label nor a free dimension is sound for the
        // MUST relation (the next-cycle value f(state_next, next_input) isn't
        // freely both flavours → target-free fabricates must-edges). So it is
        // SKIPPED (never a misleading verdict) until the next-cycle-cache fix.
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
            s.unseedable.contains(&"trigger_active".to_string()),
            "input-dependent combinational is soundly SKIPPED: {:?}",
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
        assert!(synth_sidecar_json(&[], &[], &empty_refs, &empty_inits).is_none());
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
        // instead of SKIP. (Here ⊥: the auto-seeded predicate set is too coarse
        // to decide `state_q==Error |=> $stable(state_q)`.)
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
        // SOUNDNESS — no spurious counterexample. A combinational output like
        // `main_sm_err_o` must NOT be mis-bound to the state register (which
        // would wrongly report VIOLATED); the strict alias resolver excludes it,
        // so it is honestly SKIPPED instead.
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
        // The only remaining SKIPs are pure combinational signals (case 4 of the
        // design doc — `event_detected_*`, `trigger_*`, `cnt_clr`), never a
        // primary input: H.B closed the free-input antecedents; combinational
        // outputs await the signal-node mapping follow-up.
        for p in &report.properties {
            if let VerifyOutcome::Skipped { reason } = &p.outcome {
                assert!(
                    !reason.contains("cfg_enable_i"),
                    "no free-input antecedent should remain skipped after H.B; got: {reason}"
                );
            }
        }
        // H.D (translation widening) — `signal >= signal` and 1-bit `!x === y`
        // now translate (pre-H.D: 7 unsupported; now only the arithmetic-RHS
        // `cnt == cnt + 1` form remains, which needs predicate-arithmetic).
        // sva_15 is the lone arithmetic holdout.
        assert!(
            report.properties.len() >= 15,
            "H.D translates ≥15 of 16 (was 9); got {} translated",
            report.properties.len()
        );
        assert!(
            report.unsupported.len() <= 2,
            "only the arithmetic-RHS form(s) remain unsupported after H.D; got {:?}",
            report.unsupported
        );
        assert!(
            report
                .unsupported
                .iter()
                .all(|(_, r)| r.contains("arithmetic")),
            "every remaining unsupported is the arithmetic-operand case; got {:?}",
            report.unsupported
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
        // SOUNDNESS — no spurious counterexample. This is the H.E soundness
        // anchor: an INPUT-DEPENDENT combinational antecedent (`trigger_active =
        // !trigger_i`, etc.) is SKIPPED (its register reaches a free input via
        // `cone_reaches_input`, so it is deferred to the next-cycle-cache
        // must-edge fix), NEVER bound as a state-cube label or free dimension —
        // both of which fabricated a VIOLATED on these shipped OpenTitan
        // assertions. `sva_6` (`DetectStDropOut`: `state==DetectSt && !trigger_active
        // && cfg |=> state==IdleSt`) is the canonical case it must not VIOLATE.
        let sva6 = report
            .properties
            .iter()
            .find(|p| p.name == "sysrst_ctrl_detect_sva_6")
            .expect("sva_6 present");
        assert!(
            matches!(sva6.outcome, VerifyOutcome::Skipped { .. }),
            "sva_6's input-dependent combinational `trigger_active` is soundly \
             SKIPPED (deferred), not a spurious VIOLATED; got {:?}",
            sva6.outcome
        );
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
        let seeded = seed_from_formula(&formula, |r| r == signal, |_| false, |_| None);

        // Mirror verify_auto's default cegar_opts (must Off; may = SmtAllPairs
        // iff a compound / input / derived predicate is present).
        let has_smt_seam = !seeded.compounds.is_empty()
            || !seeded.input_registers.is_empty()
            || !seeded.derived.is_empty();
        let cegar_opts = CegarOptions {
            max_iterations: 16,
            predicate_source: PredicateSource::WeakestPrecondition,
            max_cube_count: 1024,
            capture_approximants: false,
            enable_approximant_reuse: false,
            smart_uf_cap: true,
            lift_strategy: LiftStrategy::Eager,
            must_edge_inference: MustEdgeInference::Off,
            may_edge_inference: if has_smt_seam {
                MayEdgeInference::SmtAllPairs
            } else {
                MayEdgeInference::Off
            },
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
}
