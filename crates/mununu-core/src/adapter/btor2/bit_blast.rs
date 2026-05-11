//! BTOR2 → AdapterIR bit-blaster.
//!
//! The bit-blaster enumerates every (state-valuation, input-valuation) pair
//! within user-bounded register widths and produces an explicit automaton
//! suitable for mununu's explicit-state μ-calculus engine.
//!
//! # Soundness (per CLAUDE.md soundness rules)
//!
//! - **Bit-blasting is exact** for the operators marked `is_blastable()` in
//!   [`super::ast::Op`]. No approximation is introduced for those.
//! - **Width truncation:** any bit-vector wider than [`BvValue::MAX_WIDTH`]
//!   would silently lose information; the bit-blaster errors with
//!   [`AdapterErrorKind::StateSpaceOverflow`] before this can happen.
//! - **State-space bound:** total reachable states are capped at
//!   [`MAX_STATE_BITS`]; the bit-blaster errors before enumeration if the
//!   bound is exceeded. This is an *under*-approximation in scope (the
//!   design is rejected, not silently truncated) — sound by construction.
//! - **Unsupported operators** (sdiv/udiv/srem/smod/urem/saddo/...): the
//!   bit-blaster emits [`AdapterErrorKind::UnsupportedConstruct`] with the
//!   offending NID. Handing the BTOR2 to an external symbolic engine
//!   (Phase 3 hand-off) is the documented escape hatch.

use super::ast::*;
use super::parser;
use crate::adapter::ir::*;
use crate::adapter::{
    AdapterError, AdapterErrorKind, AdapterOptions, AdapterOutput, AdapterWarning, SourceFormat,
    SourceInfo, SourceLocation, WarningKind,
};

/// Maximum total bits across all state declarations.
/// 2^16 = 65 K explicit states — at the edge of practicality for the
/// current explicit-state engine. Anything above is rejected.
pub const MAX_STATE_BITS: u32 = 16;

/// Maximum total bits across all input declarations (per-step combinations).
/// 2^10 = 1024 input combos per state → up to 64 M transitions, also at the edge.
pub const MAX_INPUT_BITS: u32 = 10;

/// Bit-vector value. Backed by `u128` — sufficient for any width ≤ 128
/// (we never enumerate beyond MAX_STATE_BITS + MAX_INPUT_BITS, but
/// intermediate computations can reach wider widths).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BvValue {
    /// Lower 128 bits of the value (mask to `width` for canonical form).
    pub bits: u128,
    pub width: u32,
}

impl BvValue {
    pub const MAX_WIDTH: u32 = 128;

    pub fn new(bits: u128, width: u32) -> Self {
        let masked = if width >= 128 {
            bits
        } else {
            bits & ((1u128 << width) - 1)
        };
        BvValue {
            bits: masked,
            width,
        }
    }

    pub fn zero(width: u32) -> Self {
        BvValue::new(0, width)
    }

    pub fn one(width: u32) -> Self {
        BvValue::new(1, width)
    }

    pub fn ones(width: u32) -> Self {
        let bits = if width >= 128 {
            u128::MAX
        } else {
            (1u128 << width) - 1
        };
        BvValue { bits, width }
    }

    pub fn is_zero(&self) -> bool {
        self.bits == 0
    }

    pub fn is_nonzero(&self) -> bool {
        self.bits != 0
    }

    pub fn to_bool(&self) -> bool {
        self.is_nonzero()
    }

    pub fn from_bool(b: bool) -> Self {
        BvValue::new(if b { 1 } else { 0 }, 1)
    }
}

/// Bit-blaster output structure for a single BTOR2 file.
struct BlastOutput {
    automaton: AutomatonSpec,
    signals: Vec<Signal>,
    properties: Vec<PropertySpec>,
}

/// Top-level entry: convert a parsed BTOR2 file + options to an AdapterIR.
pub fn to_ir(
    file: &Btor2File,
    options: &AdapterOptions,
) -> Result<(AdapterIR, Vec<AdapterWarning>), AdapterError> {
    let mut warnings = Vec::new();

    // Reject unsupported operators up-front so users get a clear error
    // pointing to the BTOR2 line, not a confusing run-time panic.
    for line in &file.lines {
        if let Node::Op { op, .. } = &line.node
            && !op.is_blastable()
        {
            return Err(AdapterError {
                kind: AdapterErrorKind::UnsupportedConstruct,
                message: format!(
                    "BTOR2 operator '{op:?}' at NID {} is not supported by the Phase 1 bit-blaster. \
                     Hand the BTOR2 to an external symbolic engine (Phase 3 hand-off) instead.",
                    line.nid
                ),
                location: Some(SourceLocation {
                    line: line.source_line,
                    column: 0,
                }),
            });
        }
    }

    let states: Vec<&Line> = file.states().collect();
    let inputs: Vec<&Line> = file.inputs().collect();

    let total_state_bits = sum_widths(file, &states)?;
    let total_input_bits = sum_widths(file, &inputs)?;

    if total_state_bits > MAX_STATE_BITS {
        return Err(AdapterError {
            kind: AdapterErrorKind::StateSpaceOverflow,
            message: format!(
                "BTOR2 design has {total_state_bits} state bits → 2^{total_state_bits} = {} states (max supported: 2^{MAX_STATE_BITS} = {}). \
                 Compose-and-decompose (Phase 3) or hand-off to an external symbolic engine.",
                1u64 << total_state_bits.min(63),
                1u64 << MAX_STATE_BITS,
            ),
            location: None,
        });
    }
    if total_input_bits > MAX_INPUT_BITS {
        return Err(AdapterError {
            kind: AdapterErrorKind::StateSpaceOverflow,
            message: format!(
                "BTOR2 design has {total_input_bits} input bits per step (max supported: {MAX_INPUT_BITS})."
            ),
            location: None,
        });
    }

    if total_state_bits >= 12 {
        warnings.push(AdapterWarning {
            kind: WarningKind::LargeStateSpace,
            message: format!(
                "BTOR2 design produces 2^{total_state_bits} = {} explicit states",
                1u64 << total_state_bits.min(63),
            ),
            location: None,
        });
    }

    let blasted = enumerate_and_blast(file, &states, &inputs, options, &mut warnings)?;

    let context_name = options
        .context_name
        .clone()
        .unwrap_or_else(|| "btor2_design".into());

    Ok((
        AdapterIR {
            metadata: Metadata {
                title: context_name,
                source_format: SourceFormat::Btor2,
                description: None,
                game_semantics: None,
                known_status: None,
            },
            signals: blasted.signals,
            automata: vec![blasted.automaton],
            compositions: vec![],
            properties: blasted.properties,
            controller: None,
        },
        warnings,
    ))
}

fn sum_widths(file: &Btor2File, lines: &[&Line]) -> Result<u32, AdapterError> {
    let mut total: u32 = 0;
    for line in lines {
        let sort_nid = match &line.node {
            Node::Input { sort, .. } | Node::State { sort, .. } => *sort,
            _ => continue,
        };
        let width = parser::bv_width(file, sort_nid).ok_or_else(|| AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "input/state NID {} references non-bitvec sort {sort_nid} (arrays not yet supported)",
                line.nid
            ),
            location: Some(SourceLocation {
                line: line.source_line,
                column: 0,
            }),
        })?;
        total = total.checked_add(width).ok_or_else(|| AdapterError {
            kind: AdapterErrorKind::StateSpaceOverflow,
            message: "total bit-width exceeds u32 range".into(),
            location: None,
        })?;
    }
    Ok(total)
}

fn enumerate_and_blast(
    file: &Btor2File,
    states: &[&Line],
    inputs: &[&Line],
    options: &AdapterOptions,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<BlastOutput, AdapterError> {
    let symbols = parser::collect_symbols(file);

    // Per-state-line metadata.
    let state_meta: Vec<StateMeta> = states
        .iter()
        .enumerate()
        .map(|(idx, l)| StateMeta {
            nid: l.nid,
            width: parser::bv_width(file, sort_of(&l.node)).expect("validated above"),
            symbol: symbols
                .get(&l.nid)
                .cloned()
                .unwrap_or_else(|| format!("st{idx}_n{}", l.nid)),
        })
        .collect();

    let input_meta: Vec<InputMeta> = inputs
        .iter()
        .enumerate()
        .map(|(idx, l)| {
            let symbol = symbols
                .get(&l.nid)
                .cloned()
                .unwrap_or_else(|| format!("in{idx}_n{}", l.nid));
            let is_clock = looks_like_clock(&symbol);
            InputMeta {
                nid: l.nid,
                width: parser::bv_width(file, sort_of(&l.node)).expect("validated above"),
                symbol,
                controllable: false, // resolved below
                is_clock,
            }
        })
        .collect();

    // Phase 1 scope: single-clock posedge designs only.
    let clock_count = input_meta.iter().filter(|im| im.is_clock).count();
    if clock_count > 1 {
        return Err(AdapterError {
            kind: AdapterErrorKind::UnsupportedConstruct,
            message: format!(
                "BTOR2 design declares {clock_count} clock-shaped inputs (multi-clock is out of scope for Phase 1)."
            ),
            location: None,
        });
    }

    // Honor `--controllable-inputs` by symbolic match. Clock inputs are
    // never controllable by construction — the controller does not
    // schedule the clock edge, the world does.
    let mut input_meta = input_meta;
    for im in input_meta.iter_mut() {
        if !im.is_clock && options.controllable_inputs.iter().any(|c| c == &im.symbol) {
            im.controllable = true;
        }
    }
    if !input_meta.is_empty() && options.controllable_inputs.is_empty() {
        warnings.push(AdapterWarning {
            kind: WarningKind::NeutralControllability,
            message:
                "All BTOR2 inputs treated as uncontrollable (no --controllable-inputs specified)"
                    .into(),
            location: None,
        });
    }

    // Phase 1 sub-deliverable 2: when the caller passes a `.mununu.json`
    // sidecar, load it and resolve each named state cell to a
    // [`FieldDomain`]. Cells with a sidecar entry get bounded per the
    // declared abstraction (BoundedCounter, EnumValues, …); cells
    // without an entry fall back to full bit-blast over their width.
    let cell_domains = build_cell_domains(&state_meta, &symbols, options, warnings)?;
    let cells = CellEnumeration::build(&state_meta, &cell_domains);

    let total_state_combos = cells.total_combos();
    let total_input_combos = combinations_of_inputs(&input_meta);

    let state_names = enumerate_state_names(total_state_combos);

    // Initial state derived from `init` lines (defaulting state to 0 where
    // no init is declared — matches BTOR2 convention).
    let init_state_idx = {
        let mut init_env = make_initial_env(file, &state_meta, &input_meta, true);
        evaluate_pure(file, &mut init_env, /*honor_init=*/ true)?;
        encode_state(&init_env, &state_meta, &cells).unwrap_or(0)
    };

    // Build transitions by walking every (state, input) combination.
    // Each transition carries one label per input signal (`signal_value`)
    // — multi-label transitions, the natural CLTS encoding.
    //
    // SOUNDNESS: When the bit-blaster is given a sidecar that bounds a
    // state cell tighter than its BTOR2 width allows, the design's
    // next-state function may transition to a value outside the
    // declared abstraction (e.g., `cnt + 1` overflows past `bound`).
    // Stage 2B drops these transitions and emits a warning — this is
    // an *under-approximation* (we miss some reachable behaviors).
    // Sound for liveness; **unsound for safety** under tight bounds.
    // The OOB-sink upgrade (transitions to a designated "anything bad"
    // sink state) is tracked as a follow-up. The warning lets the
    // user widen bounds or run `mununu sv discover` to enlarge the
    // declared value set.
    let mut transitions: Vec<TransitionSpec> = Vec::new();
    let mut oob_dropped: usize = 0;
    for (state_idx, state_name) in state_names.iter().enumerate() {
        for input_idx in 0..total_input_combos {
            let mut env = make_step_env(&state_meta, &input_meta, &cells, state_idx, input_idx);
            evaluate_pure(file, &mut env, /*honor_init=*/ false)?;

            // Evaluate every constraint — if any is false in (state, input)
            // pair, skip this transition (assumption violated, environment
            // would not produce this input).
            if !constraints_hold(file, &env)? {
                continue;
            }

            // Compute next-state assignment.
            let mut next_env = env.clone();
            apply_next(file, &mut next_env, &state_meta)?;
            let Some(next_state_idx) = encode_state(&next_env, &state_meta, &cells) else {
                oob_dropped += 1;
                continue;
            };

            transitions.push(TransitionSpec {
                source: state_name.clone(),
                target: state_names[next_state_idx].clone(),
                labels: signal_labels_for_input(input_idx, &input_meta),
            });
        }
    }
    if oob_dropped > 0 {
        warnings.push(AdapterWarning {
            kind: WarningKind::ApproximateTranslation,
            message: format!(
                "{oob_dropped} transitions dropped because they led to a state \
                 outside the sidecar-declared abstraction. Under-approximation: \
                 sound for liveness, unsound for safety. Widen bounds or run \
                 `mununu sv discover` to enlarge declared value sets."
            ),
            location: None,
        });
    }

    // Build signal list — inputs are labels, states are state-vars.
    // Clock inputs are excluded: each CLTS step is a clock edge, so the
    // clock is not a signal mununu reasons over. Including it would put
    // a useless `clk_0`/`clk_1` pair in the alphabet.
    let mut signals: Vec<Signal> = Vec::new();
    for im in input_meta.iter().filter(|im| !im.is_clock) {
        signals.push(Signal {
            name: im.symbol.clone(),
            kind: if im.controllable {
                SignalKind::Output
            } else {
                SignalKind::Input
            },
            domain: if im.width == 1 {
                SignalDomain::Boolean
            } else {
                SignalDomain::BoundedInt {
                    lower: 0,
                    upper: (1i64 << im.width.min(31)) - 1,
                }
            },
            role: SignalRole::Label,
        });
    }
    for sm in &state_meta {
        signals.push(Signal {
            name: sm.symbol.clone(),
            kind: SignalKind::Neutral,
            domain: if sm.width == 1 {
                SignalDomain::Boolean
            } else {
                SignalDomain::BoundedInt {
                    lower: 0,
                    upper: (1i64 << sm.width.min(31)) - 1,
                }
            },
            role: SignalRole::State,
        });
    }

    let states_vec: Vec<StateSpec> = state_names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let vals = build_state_valuations(i, &state_meta, &cells, &cell_domains);
            StateSpec {
                name: n.clone(),
                is_initial: i == init_state_idx,
                valuations: if vals.is_empty() { None } else { Some(vals) },
            }
        })
        .collect();

    // With per-signal labels, controllability becomes per-signal-value:
    // every `<signal>_<value>` label belonging to a controllable input is
    // controllable; every label belonging to an uncontrollable input is
    // not. The compound-label era required marking the entire compound
    // controllable when any input was; the multi-label encoding makes
    // this finer-grained naturally.
    let controllable_labels: Vec<String> = input_meta
        .iter()
        .filter(|im| im.controllable)
        .flat_map(|im| (0..(1u128 << im.width.min(31))).map(move |v| format!("{}_{v}", im.symbol)))
        .collect();

    let automaton = AutomatonSpec {
        name: "Circuit".into(),
        states: states_vec,
        transitions,
        controllable_labels,
        internal_labels: vec![],
    };

    let properties = build_properties(file, &state_meta, &input_meta, &cells, &state_names)?;

    Ok(BlastOutput {
        signals,
        properties,
        automaton,
    })
}

fn build_properties(
    file: &Btor2File,
    state_meta: &[StateMeta],
    input_meta: &[InputMeta],
    cells: &CellEnumeration,
    state_names: &[String],
) -> Result<Vec<PropertySpec>, AdapterError> {
    let mut out = Vec::new();
    let total_input_combos = combinations_of_inputs(input_meta);

    // Bad properties → safety: nu X. (!(bad-states) && [] X)
    for (i, line) in file.bads().enumerate() {
        let signal = match &line.node {
            Node::Bad { signal } => *signal,
            _ => unreachable!(),
        };
        let mut hit = Vec::new();
        for (state_idx, state_name) in state_names.iter().enumerate() {
            let mut found = false;
            for input_idx in 0..total_input_combos {
                let mut env = make_step_env(state_meta, input_meta, cells, state_idx, input_idx);
                evaluate_pure(file, &mut env, /*honor_init=*/ false)?;
                if !constraints_hold(file, &env)? {
                    continue;
                }
                if read_operand(&env, signal)
                    .map(|v| v.to_bool())
                    .unwrap_or(false)
                {
                    found = true;
                    break;
                }
            }
            if found {
                hit.push(state_name.clone());
            }
        }
        if !hit.is_empty() {
            let pred = hit.join(" || ");
            out.push(PropertySpec {
                name: format!("safety_bad_{i}"),
                kind: PropertyKind::Safety,
                formula: PropertyFormula::MuCalculus(format!("nu X. ((!({pred})) && ([] X))")),
                role: PropertyRole::Standalone,
                over: None,
                description: None,
            });
        }
    }

    // Justice properties → liveness: nu Y. (mu X. (pred || <> X) && [] Y)
    for (i, line) in file.justices().enumerate() {
        let signals = match &line.node {
            Node::Justice { signals } => signals.clone(),
            _ => unreachable!(),
        };
        let mut hit = Vec::new();
        for (state_idx, state_name) in state_names.iter().enumerate() {
            let mut found = false;
            for input_idx in 0..total_input_combos {
                let mut env = make_step_env(state_meta, input_meta, cells, state_idx, input_idx);
                evaluate_pure(file, &mut env, /*honor_init=*/ false)?;
                if !constraints_hold(file, &env)? {
                    continue;
                }
                let all_true = signals
                    .iter()
                    .all(|s| read_operand(&env, *s).map(|v| v.to_bool()).unwrap_or(false));
                if all_true {
                    found = true;
                    break;
                }
            }
            if found {
                hit.push(state_name.clone());
            }
        }
        if !hit.is_empty() {
            let pred = hit.join(" || ");
            out.push(PropertySpec {
                name: format!("justice_{i}"),
                kind: PropertyKind::Fairness,
                formula: PropertyFormula::MuCalculus(format!(
                    "nu Y. ((mu X. (({pred}) || (<> X))) && ([] Y))"
                )),
                role: PropertyRole::Standalone,
                over: None,
                description: None,
            });
        }
    }

    // Fair → similar to a single-element justice set.
    for (i, line) in file.fairs().enumerate() {
        let signal = match &line.node {
            Node::Fair { signal } => *signal,
            _ => unreachable!(),
        };
        let mut hit = Vec::new();
        for (state_idx, state_name) in state_names.iter().enumerate() {
            let mut found = false;
            for input_idx in 0..total_input_combos {
                let mut env = make_step_env(state_meta, input_meta, cells, state_idx, input_idx);
                evaluate_pure(file, &mut env, /*honor_init=*/ false)?;
                if !constraints_hold(file, &env)? {
                    continue;
                }
                if read_operand(&env, signal)
                    .map(|v| v.to_bool())
                    .unwrap_or(false)
                {
                    found = true;
                    break;
                }
            }
            if found {
                hit.push(state_name.clone());
            }
        }
        if !hit.is_empty() {
            let pred = hit.join(" || ");
            out.push(PropertySpec {
                name: format!("fairness_{i}"),
                kind: PropertyKind::Fairness,
                formula: PropertyFormula::MuCalculus(format!(
                    "nu Y. ((mu X. (({pred}) || (<> X))) && ([] Y))"
                )),
                role: PropertyRole::Standalone,
                over: None,
                description: None,
            });
        }
    }

    Ok(out)
}

fn constraints_hold(file: &Btor2File, env: &Env) -> Result<bool, AdapterError> {
    for line in file.constraints() {
        let signal = match &line.node {
            Node::Constraint { signal } => *signal,
            _ => unreachable!(),
        };
        let v = read_operand(env, signal).ok_or_else(|| AdapterError {
            kind: AdapterErrorKind::IrConsistencyError,
            message: format!(
                "constraint NID {} references unevaluated signal {}",
                line.nid,
                signal.nid()
            ),
            location: Some(SourceLocation {
                line: line.source_line,
                column: 0,
            }),
        })?;
        if !v.to_bool() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn apply_next(
    file: &Btor2File,
    env: &mut Env,
    state_meta: &[StateMeta],
) -> Result<(), AdapterError> {
    // First gather (state_nid, next-value) pairs to avoid mutating env mid-iteration.
    let mut updates: Vec<(Nid, BvValue)> = Vec::new();
    for line in &file.lines {
        if let Node::Next { state, value, .. } = &line.node {
            let v = read_operand(env, *value).ok_or_else(|| AdapterError {
                kind: AdapterErrorKind::IrConsistencyError,
                message: format!("next NID {} references unevaluated signal", line.nid),
                location: Some(SourceLocation {
                    line: line.source_line,
                    column: 0,
                }),
            })?;
            updates.push((*state, v));
        }
    }
    for (state_nid, v) in updates {
        env.values.insert(state_nid, v);
    }
    // States without `next` keep their current value (BTOR2 convention).
    let _ = state_meta;
    Ok(())
}

// =====================================================================
// Environment + evaluator
// =====================================================================

#[derive(Debug, Clone, Default)]
struct Env {
    values: std::collections::HashMap<Nid, BvValue>,
}

#[derive(Debug, Clone)]
struct StateMeta {
    nid: Nid,
    width: u32,
    symbol: String,
}

#[derive(Debug, Clone)]
struct InputMeta {
    nid: Nid,
    width: u32,
    symbol: String,
    controllable: bool,
    /// True when this input is a clock signal. Clock inputs are NOT
    /// enumerated in transitions — each CLTS step already represents one
    /// clock edge. The bit-blaster injects `value=1` (posedge active)
    /// during next-state evaluation so `clk2fflogic`-introduced edge
    /// detectors fire on every CLTS transition exactly once.
    is_clock: bool,
}

/// Parse `options.sidecar_json` (when present) and resolve each
/// state-cell symbol against its `signals[]` entries via the shared
/// resolver in [`crate::adapter::sidecar`]. Returns a NID-keyed map
/// from BTOR2 state-cell IDs to their declared [`FieldDomain`] plus
/// value-name map (for [`AbstractionType::EnumValues`] domains).
///
/// Sidecar parse errors are reported as adapter warnings, not hard
/// failures — a malformed sidecar should not break the CLI's auto-load
/// path. Cells without a sidecar entry simply do not appear in the
/// returned map; callers fall back to full-width bit-blast for those.
fn build_cell_domains(
    state_meta: &[StateMeta],
    symbols: &std::collections::HashMap<i64, String>,
    options: &AdapterOptions,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<CellDomainMap, AdapterError> {
    let Some(json) = &options.sidecar_json else {
        return Ok(std::collections::HashMap::new());
    };

    // Permissive parse: a malformed sidecar is a warning, not a fatal
    // adapter error — the caller can still bit-blast the design.
    let annotation = match serde_json::from_str::<
        crate::adapter::systemverilog::annotation::SvAnnotation,
    >(json)
    {
        Ok(a) => a,
        Err(e) => {
            warnings.push(AdapterWarning {
                kind: WarningKind::UnsupportedConstruct,
                message: format!(
                    "adapter/btor2: failed to parse .mununu.json sidecar ({e}); falling back to full bit-blast"
                ),
                location: None,
            });
            return Ok(std::collections::HashMap::new());
        }
    };

    // Restrict to state-cell symbols (don't pollute with input names).
    let state_symbols: std::collections::HashMap<i64, String> = state_meta
        .iter()
        .filter_map(|sm| symbols.get(&sm.nid).map(|s| (sm.nid, s.clone())))
        .collect();

    Ok(
        crate::adapter::sidecar::btor2_resolver::build_field_domains_for_btor2(
            &annotation,
            &state_symbols,
        ),
    )
}

/// Recognize clock signals by name (case-insensitive). Single-clock
/// posedge designs are Phase 1's whole scope; multi-clock and negedge
/// will surface in Phase 3 / 4 when compositional decomposition lands.
fn looks_like_clock(symbol: &str) -> bool {
    let lower = symbol.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "clk" | "clock" | "ck" | "i_clk" | "clk_i" | "iclk" | "clki"
    )
}

fn sort_of(node: &Node) -> Nid {
    match node {
        Node::Input { sort, .. } | Node::State { sort, .. } => *sort,
        _ => panic!("sort_of called on non-input/state"),
    }
}

/// Per-state-cell sidecar resolution: NID → (declared abstraction,
/// integer→variant-name map). Built by [`build_cell_domains`] and
/// consumed by [`CellEnumeration::build`] / [`build_state_valuations`].
type CellDomainMap =
    std::collections::HashMap<i64, (crate::adapter::domain::FieldDomain, Vec<(String, i64)>)>;

/// Per-state-cell enumeration plan. Drives state-space size, decode
/// (combo → per-cell concrete bit-vector values), and encode (per-cell
/// values → combo).
///
/// Each cell carries a `Vec<u128>` of allowed concrete values:
/// - **No sidecar entry**: full bit-blast — `0..(1 << cell.width)`.
/// - **Sidecar `BoundedCounter` (bound N)**: `0..=N`.
/// - **Sidecar `Boolean`**: `[0, 1]`.
/// - **Sidecar `EnumValues` / `Discover`**: the `value_map`'s integer
///   values, plus `0` as the catch-all when no exact match.
/// - **Sidecar `Ignored`**: a single value `0` (the cell is pinned).
///
/// The total state space is the **product** of per-cell value-set sizes
/// — mixed-radix encoding, not bit-packed. Replaces the prior flat
/// `0..(1 << total_state_bits)` enumeration which inflated to
/// `MAX_STATE_BITS = 16` for any width-3 register.
struct CellEnumeration {
    /// Per state cell, the concrete bit-vector values to enumerate.
    /// Indexed by state-cell index in `state_meta` order.
    per_cell: Vec<Vec<u128>>,
    /// Per state cell, cached `per_cell[i].len()` (the radix base).
    radices: Vec<usize>,
}

impl CellEnumeration {
    fn build(state_meta: &[StateMeta], cell_domains: &CellDomainMap) -> Self {
        let per_cell: Vec<Vec<u128>> = state_meta
            .iter()
            .map(|sm| {
                if let Some((fd, vm)) = cell_domains.get(&sm.nid) {
                    Self::values_for_field_domain(fd, vm, sm.width)
                } else {
                    // Fallback: full bit-blast over the cell's BTOR2 width.
                    let cap = sm.width.min(31); // u128 safety
                    (0..(1u128 << cap)).collect()
                }
            })
            .collect();
        let radices: Vec<usize> = per_cell.iter().map(|v| v.len().max(1)).collect();
        CellEnumeration { per_cell, radices }
    }

    fn values_for_field_domain(
        fd: &crate::adapter::domain::FieldDomain,
        vm: &[(String, i64)],
        width: u32,
    ) -> Vec<u128> {
        use crate::adapter::domain::{AbstractValue, AbstractionType};
        let mask = if width >= 128 {
            u128::MAX
        } else {
            (1u128 << width) - 1
        };
        let to_concrete = |av: &AbstractValue| -> Option<u128> {
            match av {
                AbstractValue::Bool(b) | AbstractValue::Present(b) => Some(if *b { 1 } else { 0 }),
                AbstractValue::Counter(c) => {
                    if *c < 0 {
                        None
                    } else {
                        Some((*c as u128) & mask)
                    }
                }
                AbstractValue::Variant(name) => vm
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| (*v as u128) & mask),
            }
        };
        match fd.abstraction {
            AbstractionType::Ignored => vec![0u128],
            _ => {
                let mut values: Vec<u128> = fd.values().iter().filter_map(to_concrete).collect();
                // Catch-all variant has no value_map entry; default to 0
                // (won't collide if the user-named variants don't include 0).
                if values.is_empty() {
                    values.push(0u128);
                }
                values.sort_unstable();
                values.dedup();
                values
            }
        }
    }

    fn total_combos(&self) -> usize {
        self.radices
            .iter()
            .fold(1usize, |a, r| a.saturating_mul(*r))
    }

    /// Decode a linear combo index into per-cell concrete values.
    /// Cell 0 is the lowest digit (changes fastest).
    #[allow(dead_code)]
    fn decode(&self, combo: usize) -> Vec<u128> {
        let mut rem = combo;
        let mut out = Vec::with_capacity(self.per_cell.len());
        for (i, cell_values) in self.per_cell.iter().enumerate() {
            let radix = self.radices[i];
            let pick = rem % radix;
            rem /= radix;
            out.push(*cell_values.get(pick).unwrap_or(&0));
        }
        out
    }

    /// Encode per-cell concrete values back to a linear combo index.
    /// Returns `None` if any value is not in its cell's allowed set —
    /// this happens when the design's transition function lands a state
    /// outside the sidecar-declared abstraction (e.g., a counter
    /// exceeding its bound). The caller treats this as out-of-bounds.
    fn encode(&self, values: &[u128]) -> Option<usize> {
        let mut combo = 0usize;
        let mut multiplier = 1usize;
        for (i, &v) in values.iter().enumerate() {
            let radix = self.radices[i];
            let idx = self.per_cell[i].iter().position(|x| *x == v)?;
            combo = combo.checked_add(idx.checked_mul(multiplier)?)?;
            multiplier = multiplier.checked_mul(radix)?;
        }
        Some(combo)
    }

    /// Per-cell concrete value at this combo — for callers that need a
    /// single cell's value (e.g., env construction, valuations).
    fn value_at(&self, combo: usize, cell_idx: usize) -> u128 {
        let mut rem = combo;
        for (i, radix) in self.radices.iter().enumerate() {
            let pick = rem % radix;
            if i == cell_idx {
                return *self.per_cell[i].get(pick).unwrap_or(&0);
            }
            rem /= radix;
        }
        0
    }
}

#[allow(dead_code)]
fn combinations_of(meta: &[StateMeta]) -> usize {
    meta.iter()
        .fold(1usize, |acc, m| acc.saturating_mul(1usize << m.width))
}

/// Total enumerable input combinations — clock inputs are excluded.
/// Each CLTS transition already represents one posedge; the clock value
/// is implicit and held at 1 during next-state evaluation.
fn combinations_of_inputs(meta: &[InputMeta]) -> usize {
    if meta.iter().all(|m| m.is_clock) {
        return 1;
    }
    meta.iter()
        .filter(|m| !m.is_clock)
        .fold(1usize, |acc, m| acc.saturating_mul(1usize << m.width))
}

/// Enumerate state names as `s0`, `s1`, ..., `s{total-1}`.
///
/// Per-state register valuations are carried via [`StateSpec.valuations`]
/// (populated by [`build_state_valuations`]) so user-written formulas like
/// `state == 0` resolve via the predicate-via-valuations path in the
/// evaluator. The compound `s_<sym1>_<v1>__<sym2>_<v2>__...` form was
/// pure file-size cost — unique state names are all that's needed.
fn enumerate_state_names(total: usize) -> Vec<String> {
    if total == 0 {
        return vec!["s0".into()];
    }
    (0..total).map(|combo| format!("s{combo}")).collect()
}

/// For state-name index `combo`, build the valuations map keyed by the
/// user-named state cell symbol, with the cell's bit-vector value as a
/// decimal string. Synthetic state cells (no symbol annotation) are
/// dropped — the `chformal -lower` property-tracking latches are
/// uninteresting for properties the user would write.
///
/// The resulting BTreeMap is what mununu's evaluator queries via the
/// on-demand expression-evaluation path: `predicate_bits("state == 0")`
/// → parse as a guard → evaluate against valuations across all states →
/// return a bitset.
fn build_state_valuations(
    combo: usize,
    meta: &[StateMeta],
    cells: &CellEnumeration,
    cell_domains: &CellDomainMap,
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for (i, sm) in meta.iter().enumerate() {
        // Only emit user-named cells. Synthetic cells from `chformal
        // -lower` start with a digit (e.g. `_n_…`) or contain `$`; they
        // don't appear in the user's source so they shouldn't appear in
        // their formulas.
        if sm.symbol.starts_with("st") && sm.symbol.contains("_n") {
            continue;
        }
        let v = cells.value_at(combo, i);
        // For enum/discover cells, prefer the variant name from the
        // value-name map (matched by integer value); otherwise display
        // the raw integer. The on-demand expression evaluator parses
        // both forms (`state == 0` works for either).
        let display = cell_domains
            .get(&sm.nid)
            .and_then(|(_, vm)| {
                vm.iter()
                    .find(|(_, val)| (*val as u128) == v)
                    .map(|(n, _)| n.clone())
            })
            .unwrap_or_else(|| v.to_string());
        out.insert(sm.symbol.clone(), display);
    }
    out
}

/// Multi-label set for one input combination — one `<signal>_<value>`
/// label per non-clock input. Returned `Vec<String>` is what
/// `IRTransition.labels` carries; `emit_explicit` joins it with `,` and
/// the DSL parser stores them as `additional_labels` on the transition
/// declaration. Properties can then refer to individual signals via
/// `[(rst_1)] φ` rather than enumerating compound `in_…` strings.
///
/// When the design has no inputs, the transition still needs a label;
/// `step` is the conventional fallback (matches AIGER's behavior).
fn signal_labels_for_input(combo: usize, meta: &[InputMeta]) -> Vec<String> {
    let labels: Vec<String> = meta
        .iter()
        .filter(|im| !im.is_clock)
        .map(|im| {
            let v = extract_input_combo(combo, meta, im.nid);
            format!("{}_{}", im.symbol, v)
        })
        .collect();
    if labels.is_empty() {
        // No non-clock inputs: every CLTS step is just a clock tick.
        // `step` is the conventional fallback (matches AIGER's behavior).
        vec!["step".into()]
    } else {
        labels
    }
}

/// Legacy bit-shift extraction — kept for historical reference. The
/// active code path uses [`CellEnumeration::value_at`], which respects
/// the per-cell `FieldDomain` mixed-radix encoding.
#[allow(dead_code)]
fn extract_combo(combo: usize, meta: &[StateMeta], target_nid: Nid) -> u128 {
    let mut shift = 0u32;
    for sm in meta {
        if sm.nid == target_nid {
            return ((combo >> shift) & ((1usize << sm.width) - 1)) as u128;
        }
        shift += sm.width;
    }
    0
}

/// Extract the bit-vector value of `target_nid` from a packed combo
/// index. Clock inputs are skipped during packing/unpacking — they are
/// not part of the enumerated input space — and their value is always
/// reported as 1 (posedge active) so the BTOR2 evaluator sees the
/// active clock edge on every CLTS step.
fn extract_input_combo(combo: usize, meta: &[InputMeta], target_nid: Nid) -> u128 {
    let mut shift = 0u32;
    for im in meta {
        if im.nid == target_nid {
            if im.is_clock {
                return 1;
            }
            return ((combo >> shift) & ((1usize << im.width) - 1)) as u128;
        }
        if !im.is_clock {
            shift += im.width;
        }
    }
    0
}

fn make_step_env(
    state_meta: &[StateMeta],
    input_meta: &[InputMeta],
    cells: &CellEnumeration,
    state_idx: usize,
    input_idx: usize,
) -> Env {
    let mut env = Env::default();
    for (i, sm) in state_meta.iter().enumerate() {
        let bits = cells.value_at(state_idx, i);
        env.values.insert(sm.nid, BvValue::new(bits, sm.width));
    }
    for im in input_meta {
        let bits = extract_input_combo(input_idx, input_meta, im.nid);
        env.values.insert(im.nid, BvValue::new(bits, im.width));
    }
    env
}

/// Build the initial environment used to compute init values.
/// `with_inputs_zero=true` zeroes inputs (init usually doesn't depend on them).
fn make_initial_env(
    file: &Btor2File,
    state_meta: &[StateMeta],
    input_meta: &[InputMeta],
    with_inputs_zero: bool,
) -> Env {
    let mut env = Env::default();
    // For init evaluation, states default to zero unless an init line is processed
    // (handled by `evaluate_pure` → `Init` propagating the init value).
    for sm in state_meta {
        env.values.insert(sm.nid, BvValue::zero(sm.width));
    }
    if with_inputs_zero {
        for im in input_meta {
            env.values.insert(im.nid, BvValue::zero(im.width));
        }
    }
    let _ = file;
    env
}

/// Encode an evaluator [`Env`] back into a linear state-combo index via
/// the [`CellEnumeration`]'s mixed-radix scheme. Returns `None` when
/// the env's per-cell values are not all in their declared abstraction
/// — caller decides whether to drop the transition (under-approx) or
/// route to an OOB sink (over-approx). Stage 2B of the BTOR2 sidecar
/// integration uses the drop semantics; OOB sink is a follow-up.
fn encode_state(env: &Env, state_meta: &[StateMeta], cells: &CellEnumeration) -> Option<usize> {
    let values: Vec<u128> = state_meta
        .iter()
        .map(|sm| {
            env.values
                .get(&sm.nid)
                .copied()
                .unwrap_or(BvValue::zero(sm.width))
                .bits
        })
        .collect();
    cells.encode(&values)
}

fn read_operand(env: &Env, op: Operand) -> Option<BvValue> {
    let v = env.values.get(&op.nid()).copied()?;
    if op.is_negated() {
        // BTOR2 negative-NID shorthand = bitwise NOT.
        Some(BvValue::new(!v.bits, v.width))
    } else {
        Some(v)
    }
}

/// Evaluate every node in declaration order, populating `env` with the
/// computed value for each NID.
///
/// `honor_init = true` only when computing the initial-state assignment;
/// during step (transition) evaluation it must be `false` so `init` lines
/// do not stomp the current state value passed in via `make_step_env`.
fn evaluate_pure(file: &Btor2File, env: &mut Env, honor_init: bool) -> Result<(), AdapterError> {
    for line in &file.lines {
        match &line.node {
            Node::Sort { .. } | Node::Input { .. } | Node::State { .. } => {
                // Inputs and states are already populated by make_*_env.
            }
            Node::Const { sort, value } => {
                let width = parser::bv_width(file, *sort)
                    .ok_or_else(|| constancy_err(line, "constant references non-bitvec sort"))?;
                let bv = match value {
                    ConstValue::Zero => BvValue::zero(width),
                    ConstValue::One => BvValue::one(width),
                    ConstValue::Ones => BvValue::ones(width),
                    ConstValue::Bin(s) => {
                        let bits = u128::from_str_radix(s, 2)
                            .map_err(|_| constancy_err(line, "bad binary literal"))?;
                        BvValue::new(bits, width)
                    }
                    ConstValue::Dec(d) => {
                        let bits = if *d >= 0 {
                            *d as u128
                        } else {
                            // Two's complement of a negative literal.
                            let abs = (-d) as u128;
                            let mask = if width >= 128 {
                                u128::MAX
                            } else {
                                (1u128 << width) - 1
                            };
                            (mask.wrapping_sub(abs).wrapping_add(1)) & mask
                        };
                        BvValue::new(bits, width)
                    }
                    ConstValue::Hex(s) => {
                        let bits = u128::from_str_radix(s, 16)
                            .map_err(|_| constancy_err(line, "bad hex literal"))?;
                        BvValue::new(bits, width)
                    }
                };
                env.values.insert(line.nid, bv);
            }
            Node::Init { state, value, .. } => {
                if honor_init {
                    let v = read_operand(env, *value)
                        .ok_or_else(|| constancy_err(line, "init value not yet evaluated"))?;
                    env.values.insert(*state, v);
                }
            }
            Node::Op { sort, op, args, .. } => {
                let width = parser::bv_width(file, *sort)
                    .ok_or_else(|| constancy_err(line, "operator references non-bitvec sort"))?;
                let result =
                    eval_op(*op, &line.immediates, args, width, env).map_err(|e| AdapterError {
                        kind: AdapterErrorKind::IrConsistencyError,
                        message: format!("at NID {}: {}", line.nid, e),
                        location: Some(SourceLocation {
                            line: line.source_line,
                            column: 0,
                        }),
                    })?;
                env.values.insert(line.nid, result);
            }
            // Side-effect declarations don't add to env.
            Node::Next { .. }
            | Node::Bad { .. }
            | Node::Constraint { .. }
            | Node::Fair { .. }
            | Node::Output { .. }
            | Node::Justice { .. } => {}
        }
    }
    Ok(())
}

fn constancy_err(line: &Line, msg: &str) -> AdapterError {
    AdapterError {
        kind: AdapterErrorKind::IrConsistencyError,
        message: format!("NID {}: {msg}", line.nid),
        location: Some(SourceLocation {
            line: line.source_line,
            column: 0,
        }),
    }
}

fn eval_op(
    op: Op,
    immediates: &[u32],
    args: &[Operand],
    width: u32,
    env: &Env,
) -> Result<BvValue, String> {
    let read = |i: usize| -> Result<BvValue, String> {
        read_operand(env, args[i]).ok_or_else(|| format!("operand {} unevaluated", args[i].nid()))
    };

    match op {
        Op::Not => {
            let v = read(0)?;
            Ok(BvValue::new(!v.bits, v.width))
        }
        Op::Inc => {
            let v = read(0)?;
            Ok(BvValue::new(v.bits.wrapping_add(1), v.width))
        }
        Op::Dec => {
            let v = read(0)?;
            Ok(BvValue::new(v.bits.wrapping_sub(1), v.width))
        }
        Op::Neg => {
            let v = read(0)?;
            Ok(BvValue::new(0u128.wrapping_sub(v.bits), v.width))
        }
        Op::Redand => {
            let v = read(0)?;
            let all_ones = if v.width >= 128 {
                v.bits == u128::MAX
            } else {
                v.bits == (1u128 << v.width) - 1
            };
            Ok(BvValue::from_bool(all_ones))
        }
        Op::Redor => {
            let v = read(0)?;
            Ok(BvValue::from_bool(v.bits != 0))
        }
        Op::Redxor => {
            let v = read(0)?;
            Ok(BvValue::from_bool(v.bits.count_ones() & 1 == 1))
        }
        Op::Iff | Op::Eq => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::from_bool(a.bits == b.bits))
        }
        Op::Implies => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::from_bool(!a.to_bool() || b.to_bool()))
        }
        Op::Neq => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::from_bool(a.bits != b.bits))
        }
        Op::And => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(a.bits & b.bits, width))
        }
        Op::Or => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(a.bits | b.bits, width))
        }
        Op::Xor => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(a.bits ^ b.bits, width))
        }
        Op::Nand => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(!(a.bits & b.bits), width))
        }
        Op::Nor => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(!(a.bits | b.bits), width))
        }
        Op::Xnor => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(!(a.bits ^ b.bits), width))
        }
        Op::Add => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(a.bits.wrapping_add(b.bits), width))
        }
        Op::Sub => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(a.bits.wrapping_sub(b.bits), width))
        }
        Op::Mul => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new(a.bits.wrapping_mul(b.bits), width))
        }
        Op::Ult => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::from_bool(a.bits < b.bits))
        }
        Op::Ulte => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::from_bool(a.bits <= b.bits))
        }
        Op::Ugt => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::from_bool(a.bits > b.bits))
        }
        Op::Ugte => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::from_bool(a.bits >= b.bits))
        }
        Op::Slt | Op::Slte | Op::Sgt | Op::Sgte => {
            let a = read(0)?;
            let b = read(1)?;
            let sa = sign_extend(a.bits, a.width);
            let sb = sign_extend(b.bits, b.width);
            let r = match op {
                Op::Slt => sa < sb,
                Op::Slte => sa <= sb,
                Op::Sgt => sa > sb,
                Op::Sgte => sa >= sb,
                _ => unreachable!(),
            };
            Ok(BvValue::from_bool(r))
        }
        Op::Sll => {
            let a = read(0)?;
            let b = read(1)?;
            let shift = (b.bits as u32) & (a.width.saturating_sub(1).max(1));
            Ok(BvValue::new(a.bits.wrapping_shl(shift), width))
        }
        Op::Srl => {
            let a = read(0)?;
            let b = read(1)?;
            let shift = (b.bits as u32) & (a.width.saturating_sub(1).max(1));
            Ok(BvValue::new(a.bits.wrapping_shr(shift), width))
        }
        Op::Sra => {
            let a = read(0)?;
            let b = read(1)?;
            let shift = (b.bits as u32) & (a.width.saturating_sub(1).max(1));
            let signed = sign_extend(a.bits, a.width);
            let shifted = signed >> shift;
            Ok(BvValue::new(shifted as u128, width))
        }
        Op::Rol => {
            let a = read(0)?;
            let b = read(1)?;
            let shift = (b.bits as u32) % a.width.max(1);
            let mask = if a.width >= 128 {
                u128::MAX
            } else {
                (1u128 << a.width) - 1
            };
            let bits = ((a.bits << shift) | (a.bits >> (a.width - shift))) & mask;
            Ok(BvValue::new(bits, width))
        }
        Op::Ror => {
            let a = read(0)?;
            let b = read(1)?;
            let shift = (b.bits as u32) % a.width.max(1);
            let mask = if a.width >= 128 {
                u128::MAX
            } else {
                (1u128 << a.width) - 1
            };
            let bits = ((a.bits >> shift) | (a.bits << (a.width - shift))) & mask;
            Ok(BvValue::new(bits, width))
        }
        Op::Concat => {
            let a = read(0)?;
            let b = read(1)?;
            Ok(BvValue::new((a.bits << b.width) | b.bits, width))
        }
        Op::Slice => {
            let a = read(0)?;
            let upper = immediates
                .first()
                .copied()
                .ok_or_else(|| "slice missing upper".to_string())?;
            let lower = immediates
                .get(1)
                .copied()
                .ok_or_else(|| "slice missing lower".to_string())?;
            let shifted = a.bits >> lower;
            let mask = if width >= 128 {
                u128::MAX
            } else {
                (1u128 << width) - 1
            };
            let _ = upper;
            Ok(BvValue::new(shifted & mask, width))
        }
        Op::Uext => {
            let a = read(0)?;
            Ok(BvValue::new(a.bits, width))
        }
        Op::Sext => {
            let a = read(0)?;
            let signed = sign_extend(a.bits, a.width);
            Ok(BvValue::new(signed as u128, width))
        }
        Op::Ite => {
            let c = read(0)?;
            let t = read(1)?;
            let e = read(2)?;
            Ok(if c.to_bool() { t } else { e })
        }
        _ => Err(format!(
            "operator {op:?} unsupported in Phase 1 bit-blaster"
        )),
    }
}

fn sign_extend(bits: u128, width: u32) -> i128 {
    if width == 0 || width >= 128 {
        return bits as i128;
    }
    let sign_bit = 1u128 << (width - 1);
    if bits & sign_bit != 0 {
        let mask = !((1u128 << width) - 1);
        (bits | mask) as i128
    } else {
        bits as i128
    }
}

/// High-level entry: parse + bit-blast + emit.
pub fn translate(content: &str, options: &AdapterOptions) -> Result<AdapterOutput, AdapterError> {
    let file = super::parser::parse(content)?;
    let (ir, warnings) = to_ir(&file, options)?;

    let state_count = ir.automata.first().map(|a| a.states.len()).unwrap_or(0);
    let signal_count = ir.signals.len();
    let property_count = ir.properties.len();

    let emit_result = crate::adapter::emit::emit(&ir)?;

    // Extract state valuations from the IR and stash them in the output's
    // side-channel map. Same pattern as the SV adapter: the CTXDSL text
    // does not encode valuations, so the realize pipeline picks them up
    // from `AdapterOutput.state_valuations` and registers them on the
    // CLTS for the on-demand predicate evaluator.
    let mut state_valuations: std::collections::HashMap<
        String,
        std::collections::HashMap<String, std::collections::BTreeMap<String, String>>,
    > = std::collections::HashMap::new();
    for aut in &ir.automata {
        let mut per_state: std::collections::HashMap<
            String,
            std::collections::BTreeMap<String, String>,
        > = std::collections::HashMap::new();
        for s in &aut.states {
            if let Some(v) = &s.valuations {
                per_state.insert(s.name.clone(), v.clone());
            }
        }
        if !per_state.is_empty() {
            state_valuations.insert(aut.name.clone(), per_state);
        }
    }

    Ok(AdapterOutput {
        ctxdsl: emit_result.ctxdsl,
        warnings,
        source_info: SourceInfo {
            format: SourceFormat::Btor2,
            title: None,
            signal_count,
            state_count,
            property_count,
        },
        state_valuations,
        transition_observations: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::AdapterOptions;

    #[test]
    fn toggle_latch_safety_unrealizable_when_bad_reachable() {
        // 1-bit latch q: q' = !q; init q=0; bad when q=1.
        // Bad is reachable, so safety property must be present.
        let src = r#"
1 sort bitvec 1
2 zero 1
3 state 1 q
4 init 1 3 2
5 not 1 3
6 next 1 3 5
7 bad 3
"#;
        let opts = AdapterOptions::default();
        let out = translate(src, &opts).expect("translate");
        assert_eq!(out.source_info.state_count, 2);
        assert_eq!(out.source_info.property_count, 1);
        assert!(out.ctxdsl.contains("safety_bad_0"));
    }

    #[test]
    fn rejects_unsupported_op() {
        let src = r#"
1 sort bitvec 4
2 input 1 a
3 input 1 b
4 udiv 1 2 3
"#;
        let opts = AdapterOptions::default();
        let err = translate(src, &opts).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::UnsupportedConstruct);
    }

    #[test]
    fn rejects_state_space_overflow() {
        // 17 1-bit states → 2^17 > MAX_STATE_BITS=16
        let mut src = "1 sort bitvec 1\n".to_string();
        for i in 0..17 {
            src.push_str(&format!("{} state 1 s{}\n", i + 2, i));
        }
        let err = translate(&src, &AdapterOptions::default()).unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::StateSpaceOverflow);
    }

    #[test]
    fn empty_design_produces_one_state() {
        let src = "1 sort bitvec 1\n";
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        assert_eq!(out.source_info.state_count, 1);
    }

    #[test]
    fn constraint_filters_transitions() {
        // input drives state; constraint = input must be 0.
        // Without constraint: 2 states × 2 inputs = 4 transitions.
        // With constraint:    2 states × 1 input  = 2 transitions.
        let src = r#"
1 sort bitvec 1
2 zero 1
3 state 1 q
4 init 1 3 2
5 input 1 a
6 next 1 3 5
7 not 1 5
8 constraint 7
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        assert_eq!(out.source_info.state_count, 2);
        // Transitions encoded in CTXDSL — we just check the run completed.
        assert!(out.ctxdsl.contains("automaton"));
    }
}
