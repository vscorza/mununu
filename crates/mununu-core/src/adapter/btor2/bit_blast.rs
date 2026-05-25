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
/// 2^20 = ~1 M explicit states — the explicit-state engine's practical
/// upper bound. Raised from 16 → 20 on 2026-05-18 to admit small
/// industrial RTL modules (e.g. the Caliptra-RTL boot FSM bundles a
/// 3-bit state enum with an 8-bit wait counter + reset-window logic
/// totalling 19 state bits, which the previous cap rejected just past
/// the boundary). Designs above this still error rather than truncate.
pub const MAX_STATE_BITS: u32 = 20;

/// Maximum total bits across all input declarations (per-step combinations).
/// 2^10 = 1024 input combos per state → up to ~1 G transitions at the
/// raised state cap (MAX_STATE_BITS = 20). Lifting the input cap further
/// quickly becomes intractable: 2^20 × 2^16 ≈ 6.8e10 transitions takes
/// hours to enumerate concretely. Designs above this cap must prune
/// unused inputs via a `.mununu.json` sidecar (see
/// [`crate::adapter::sidecar`]) declaring `Ignored` / `Boolean` /
/// `Symbols` abstractions per input.
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
    /// Phase A.3 step 3.6 — auto-partition telemetry populated by
    /// `enumerate_and_blast` after the COI pass runs. `None` when
    /// the partition step was skipped (empty seeds, or a future
    /// `--no-partition` opt-out).
    partition_summary: Option<crate::adapter::partition::PartitionSummary>,
}

/// Top-level entry: convert a parsed BTOR2 file + options to an AdapterIR.
///
/// Returns a tuple of (IR, warnings, partition_summary). The third
/// element is `Some` whenever `enumerate_and_blast` ran the auto-COI
/// pass; the BTOR2 adapter forwards it onto `AdapterOutput`.
pub fn to_ir(
    file: &Btor2File,
    options: &AdapterOptions,
) -> Result<
    (
        AdapterIR,
        Vec<AdapterWarning>,
        Option<crate::adapter::partition::PartitionSummary>,
    ),
    AdapterError,
> {
    let mut warnings = Vec::new();

    // §Phase 10 §10.2 stage 1 — detect array-typed state cells
    // (BTOR2 `state` lines whose sort is `Sort::Array`) and validate
    // them against the sidecar's `memories: [...]` declarations.
    // The detection runs BEFORE the is_blastable check so that
    // memory-bearing fixtures get an actionable error pointing at
    // the sidecar template they need to add, instead of a generic
    // "operator not supported" error from the downstream
    // Read/Write check.
    let memory_cells = detect_btor2_memories(file);
    if !memory_cells.is_empty() {
        validate_sidecar_memories(file, &memory_cells, options)?;
    }

    // Reject unsupported operators up-front so users get a clear error
    // pointing to the BTOR2 line, not a confusing run-time panic.
    // §Phase 10 §10.2 stage 1 nuance: Read/Write operators on
    // memory cells will still trip this check until stage 3 (UF)
    // or stage 4 (bounded bit-blast) ships. The error message
    // includes the §Phase 10 hint so the user knows to wait for
    // (or contribute) the stage 3/4 implementation.
    for line in &file.lines {
        if let Node::Op { op, .. } = &line.node
            && !op.is_blastable()
        {
            let phase10_hint = if matches!(op, Op::Read | Op::Write) {
                " (§Phase 10 §10.2 stage 1 has detected memory cells in this BTOR2 \
                 and validated the sidecar; the actual lift will succeed once \
                 stage 3 (UF mode) or stage 4 (bounded bit-blast) ships.)"
            } else {
                ""
            };
            return Err(AdapterError {
                kind: AdapterErrorKind::UnsupportedConstruct,
                message: format!(
                    "BTOR2 operator '{op:?}' at NID {} is not supported by the Phase 1 bit-blaster.{phase10_hint} \
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

    // M.1 (§Phase 11 priority 2) — abstraction-aware bit accounting.
    // Cells declared `Ignored` in the sidecar contribute zero bits
    // to the state-space cap check (they collapse to a single
    // pinned value at lift time). Discovered while attempting M.1
    // on OpenTitan `uart_tx.sv`: an 11-bit shift register the
    // property doesn't reference would otherwise push the design
    // past `MAX_STATE_BITS = 20` despite the user's intent to
    // ignore it.
    //
    // The pre-existing `sum_widths` runs over raw BTOR2 widths
    // because cell_domains isn't populated yet at this point. We
    // run a lightweight sidecar pre-scan here to subtract Ignored
    // cells' widths before checking the cap. Strict additivity:
    // when no sidecar is provided OR no cells are Ignored, the
    // accounting is identical to the legacy behaviour.
    let raw_state_bits = sum_widths(file, &states)?;
    let total_state_bits =
        raw_state_bits.saturating_sub(sidecar_ignored_state_bits(file, &states, options));
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
    // Input-bit cap is checked AFTER sidecar resolution inside
    // `enumerate_and_blast` (per-input `FieldDomain` abstractions may
    // collapse a wide raw input into a 1-of-N value set — e.g. an
    // `Ignored` input contributes zero effective bits even when its
    // raw BTOR2 width is large). The raw-bit number is kept for
    // diagnostics only.
    let _ = total_input_bits;

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
        blasted.partition_summary,
    ))
}

fn sum_widths(file: &Btor2File, lines: &[&Line]) -> Result<u32, AdapterError> {
    let mut total: u32 = 0;
    for line in lines {
        let sort_nid = match &line.node {
            Node::Input { sort, .. } | Node::State { sort, .. } => *sort,
            _ => continue,
        };
        // §Phase 10 §10.2 stage 1: skip array-sorted state cells in
        // the bit-width accounting. Memory abstraction (UF / Havoc /
        // bounded bit-blast) handles them separately via the
        // sidecar's `memories` declarations. Stage 1 today still
        // errors at the Read/Write op check (the actual lift fails),
        // but the state-bit accounting must not double-error here.
        if matches!(
            sort_of_arc(file, sort_nid),
            Some(crate::adapter::btor2::ast::Sort::Array { .. })
        ) {
            continue;
        }
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

/// M.1 (§Phase 11) — sum the raw bit-widths of state cells whose
/// sidecar entry declares `abstraction: ignored`. Subtracted from
/// the cap-check total so wide registers the user explicitly opted
/// to ignore don't push the design past `MAX_STATE_BITS`.
///
/// Returns 0 when no sidecar is provided, the sidecar fails to
/// parse, or no cells are declared Ignored. Strict additivity:
/// legacy behaviour is preserved when this helper returns 0.
///
/// **M.1 Path B (§Phase 11):** resolves each ignored signal via the
/// `drives` override (falling back to the sidecar `name`) using
/// [`parser::resolve_state_by_symbol`], so sidecar entries match
/// state cells even when Yosys strips the original register symbol
/// from the `state` line and only attaches it via an Op-alias or
/// `output` port symbol.
fn sidecar_ignored_state_bits(file: &Btor2File, states: &[&Line], options: &AdapterOptions) -> u32 {
    let Some(json) = &options.sidecar_json else {
        return 0;
    };
    let Ok(ann) =
        serde_json::from_str::<crate::adapter::systemverilog::annotation::SvAnnotation>(json)
    else {
        return 0;
    };
    // Map each Ignored sidecar signal → its resolved BTOR2 state NID
    // (drives override > sidecar name; BFS-nearest-state).
    let ignored_nids: std::collections::HashSet<Nid> = ann
        .signals
        .iter()
        .filter(|s| {
            matches!(
                s.abstraction,
                crate::adapter::systemverilog::annotation::SignalAbstraction::Ignored
            )
        })
        .filter_map(|s| {
            let target = s.drives.as_deref().unwrap_or(s.name.as_str());
            parser::resolve_state_by_symbol(file, target)
        })
        .collect();
    if ignored_nids.is_empty() {
        return 0;
    }
    let mut sum: u32 = 0;
    for line in states {
        if !ignored_nids.contains(&line.nid) {
            continue;
        }
        let sort_nid = match &line.node {
            Node::State { sort, .. } => *sort,
            _ => continue,
        };
        let Some(width) = parser::bv_width(file, sort_nid) else {
            continue;
        };
        sum = sum.saturating_add(width);
    }
    sum
}

/// M.1 Path B (§Phase 11) — build an augmented NID→sidecar-name map
/// for the four downstream helpers that today look up sidecar names
/// via `symbols.get(nid)`. For each sidecar signal entry, resolve
/// its driver (`drives` override > sidecar `name`) via
/// [`parser::resolve_state_by_symbol`]; when that finds a state NID,
/// override the entry in the map with the sidecar's `name` (so the
/// downstream consumers see the user's name regardless of which
/// symbol Yosys happens to have attached to the state cell).
///
/// **Strict additivity.** Cells with no sidecar entry retain
/// whatever Yosys-attached symbol was already in `base_symbols`.
/// Cells whose sidecar entry resolves to the same NID Yosys already
/// named identically see no change. Only cells where the sidecar's
/// `drives` (or `name`, when `drives` is None) resolves the cell to
/// a sidecar entry whose `name` differs from Yosys's symbol — that
/// is the Path B-shaped overlap — get their entry rewritten.
fn enrich_symbols_with_sidecar_drives(
    file: &Btor2File,
    base_symbols: &std::collections::HashMap<Nid, String>,
    options: &AdapterOptions,
) -> std::collections::HashMap<Nid, String> {
    let mut out = base_symbols.clone();
    let Some(json) = &options.sidecar_json else {
        return out;
    };
    let Ok(ann) =
        serde_json::from_str::<crate::adapter::systemverilog::annotation::SvAnnotation>(json)
    else {
        return out;
    };
    for sig in &ann.signals {
        let target = sig.drives.as_deref().unwrap_or(sig.name.as_str());
        if let Some(state_nid) = parser::resolve_state_by_symbol(file, target) {
            out.insert(state_nid, sig.name.clone());
        }
    }
    out
}

/// §Phase 10 §10.2 stage 1 — metadata for one memory state cell
/// detected in the BTOR2 source. The `name` is the user-visible
/// symbol (from `parser::collect_symbols`), used to cross-reference
/// against the sidecar's `memories[]` declarations.
#[derive(Debug, Clone)]
struct MemoryCellMeta {
    /// BTOR2 NID of the `state` line; reserved for the §Phase 10
    /// §10.2 stage 3+ lifter to look up the cell during predicate-image
    /// queries. Currently unused in stage 1 (schema/validate-only).
    #[allow(dead_code)]
    nid: Nid,
    name: String,
    address_width: u32,
    data_width: u32,
    source_line: usize,
}

/// §Phase 10 §10.2 stage 1 — walk the BTOR2 file for `state` lines
/// whose sort is `Sort::Array`, resolve their address/data widths
/// via the array sort's index/element references, and return one
/// [`MemoryCellMeta`] per detected memory.
///
/// Returns an empty vec when no array state cells exist (the common
/// case for FSM-only fixtures). Memory cells without a resolvable
/// symbol get a synthetic `nid_<n>` name so the sidecar validator
/// can still emit an actionable error pointing at the BTOR2 line.
fn detect_btor2_memories(file: &Btor2File) -> Vec<MemoryCellMeta> {
    let symbols = parser::collect_symbols(file);
    let mut out = Vec::new();
    for line in &file.lines {
        let Node::State { sort, .. } = &line.node else {
            continue;
        };
        let Some(crate::adapter::btor2::ast::Sort::Array { index, element }) =
            sort_of_arc(file, *sort)
        else {
            continue;
        };
        let Some(address_width) = parser::bv_width(file, index) else {
            continue;
        };
        let Some(data_width) = parser::bv_width(file, element) else {
            continue;
        };
        let name = symbols
            .get(&line.nid)
            .cloned()
            .unwrap_or_else(|| format!("nid_{}", line.nid));
        out.push(MemoryCellMeta {
            nid: line.nid,
            name,
            address_width,
            data_width,
            source_line: line.source_line,
        });
    }
    out
}

/// §Phase 10 §10.2 stage 1 — validate detected memory cells against
/// the sidecar's `memories[]` declarations. Returns Ok when every
/// detected memory has a matching sidecar entry with matching
/// `address_width` + `data_width`. Returns an actionable error
/// otherwise — the error message includes a copy-paste-ready sidecar
/// template the user can drop into their `.mununu.json` to declare
/// the missing memories.
///
/// When the sidecar JSON is absent OR the schema fails to parse, the
/// error still fires (memory cells exist in the BTOR2 and we have
/// no declarations to validate against). The user sees the same
/// template; this is the right outcome — silent acceptance of
/// undeclared memories would let the downstream lift produce
/// unsound verdicts.
fn validate_sidecar_memories(
    _file: &Btor2File,
    memory_cells: &[MemoryCellMeta],
    options: &AdapterOptions,
) -> Result<(), AdapterError> {
    use crate::adapter::systemverilog::annotation::SvAnnotation;

    // Try to parse sidecar; missing/unparseable JSON ⇒ no declarations.
    let declared: std::collections::HashMap<String, (u32, u32)> = options
        .sidecar_json
        .as_deref()
        .and_then(|json| serde_json::from_str::<SvAnnotation>(json).ok())
        .map(|ann| {
            ann.memories
                .into_iter()
                .map(|m| (m.name, (m.address_width, m.data_width)))
                .collect()
        })
        .unwrap_or_default();

    let mut missing: Vec<&MemoryCellMeta> = Vec::new();
    let mut mismatched: Vec<(&MemoryCellMeta, u32, u32)> = Vec::new();

    for cell in memory_cells {
        match declared.get(&cell.name) {
            None => missing.push(cell),
            Some(&(declared_aw, declared_dw)) => {
                if declared_aw != cell.address_width || declared_dw != cell.data_width {
                    mismatched.push((cell, declared_aw, declared_dw));
                }
            }
        }
    }

    if missing.is_empty() && mismatched.is_empty() {
        return Ok(());
    }

    let mut message = String::from(
        "§Phase 10 §10.2 stage 1 — the BTOR2 source contains memory cells that need \
         sidecar declarations before lifting can proceed. Add the following entries to your \
         `.mununu.json` sidecar's `memories` field:\n\n",
    );
    message.push_str("  \"memories\": [\n");
    for cell in &missing {
        message.push_str(&format!(
            "    {{\"name\":\"{}\", \"address_width\":{}, \"data_width\":{}, \"abstraction\":\"havoc\"}},\n",
            cell.name, cell.address_width, cell.data_width
        ));
    }
    for (cell, declared_aw, declared_dw) in &mismatched {
        message.push_str(&format!(
            "    // mismatch on `{}`: BTOR2 has address_width={}, data_width={}; \
             sidecar declared address_width={}, data_width={}. Fix:\n",
            cell.name, cell.address_width, cell.data_width, declared_aw, declared_dw
        ));
        message.push_str(&format!(
            "    {{\"name\":\"{}\", \"address_width\":{}, \"data_width\":{}, \"abstraction\":\"havoc\"}},\n",
            cell.name, cell.address_width, cell.data_width
        ));
    }
    message.push_str("  ]\n\n");
    message.push_str(
        "Currently supported abstractions: `havoc` (over-approximation; sound for safety, \
         unsound for liveness). UF mode (stage 3) and bounded bit-blast (stage 4) are queued \
         — track the §Phase 10 plan for shipping status.",
    );

    let first_line = missing
        .first()
        .map(|c| c.source_line)
        .or_else(|| mismatched.first().map(|(c, _, _)| c.source_line))
        .unwrap_or(0);

    Err(AdapterError {
        kind: AdapterErrorKind::UnsupportedConstruct,
        message,
        location: Some(SourceLocation {
            line: first_line,
            column: 0,
        }),
    })
}

/// §Phase 10 §10.2 stage 1 helper — resolve a sort NID to its `Sort`.
fn sort_of_arc(file: &Btor2File, sort_nid: Nid) -> Option<crate::adapter::btor2::ast::Sort> {
    for line in &file.lines {
        if line.nid == sort_nid
            && let Node::Sort { sort } = &line.node
        {
            return Some(sort.clone());
        }
    }
    None
}

fn enumerate_and_blast(
    file: &Btor2File,
    states: &[&Line],
    inputs: &[&Line],
    options: &AdapterOptions,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<BlastOutput, AdapterError> {
    // M.1 Path B (§Phase 11) — start from Yosys's collect_symbols
    // output, then let the sidecar override per-NID names for
    // entries whose `drives` (or fallback `name`) resolves to a
    // state cell whose Yosys symbol is some other alias (`sreg_d`,
    // `bit_cnt_d`, etc.). Strict additivity: no override fires
    // when the sidecar's name already matches Yosys's symbol.
    let symbols = enrich_symbols_with_sidecar_drives(file, &parser::collect_symbols(file), options);

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

    // Two-stage controllability classification (Document B task B1):
    //
    //   1. If the upstream frontend captured port directions before any
    //      flattening / inlining (typically the yosys driver populating
    //      `options.port_directions` from the pre-flatten `write_json`
    //      hierarchy snapshot), apply the §4 rule — input direction →
    //      Uncontrollable, output direction → Controllable. Inputs not
    //      named in the map keep the historical "Uncontrollable" default
    //      (the right call for BTOR2 inputs that originated as cut points
    //      from `cutpoint -blackbox`).
    //   2. `--controllable-inputs` remains the escape hatch and runs
    //      *after* (1), so an explicit list always wins.
    //
    // Clock inputs are never controllable by construction — the controller
    // does not schedule the clock edge, the world does.
    use crate::controllability::BoundaryDirection;
    let mut input_meta = input_meta;
    let derived_from_directions = !options.port_directions.is_empty();
    for im in input_meta.iter_mut() {
        if im.is_clock {
            continue;
        }
        if let Some(dir) = options.port_directions.get(&im.symbol) {
            im.controllable = matches!(dir, BoundaryDirection::Output);
        }
        if options.controllable_inputs.iter().any(|c| c == &im.symbol) {
            im.controllable = true;
        }
    }
    if !input_meta.is_empty() && options.controllable_inputs.is_empty() && !derived_from_directions
    {
        warnings.push(AdapterWarning {
            kind: WarningKind::NeutralControllability,
            message:
                "All BTOR2 inputs treated as uncontrollable (no --controllable-inputs specified)"
                    .into(),
            location: None,
        });
    }

    // Phase A.3 step 3.5 — automatic partition (live).
    //
    // Compute the partition from the BTOR2 dep-graph and the seed set
    // extracted from intrinsic `bad` / `constraint` / `justice` / `fair`
    // lines. The injection into `cell_domains` / `input_domains` happens
    // **after** the sidecar resolver runs (below), so the user-wins-on-
    // collision rule is enforced naturally — anything the user listed
    // appears as a key in those maps; anything they didn't list is
    // absent, and the partition's `Dropped` verdict is what fills the
    // gap.
    //
    // SOUNDNESS: see `crate::adapter::partition` module docs. Auto-COI
    // pins a `Dropped` signal to a single value via
    // `AbstractionType::Ignored`; the abstract model admits more
    // behaviours than the concrete one (sound for safety + over-
    // approximation).
    let partition = {
        use crate::adapter::partition::{self, PartitionOptions};
        let seeds = super::dep_graph::extract_property_seeds(file);
        partition::classify(file, &seeds, &PartitionOptions::default())
    };

    // Phase 1 sub-deliverable 2: when the caller passes a `.mununu.json`
    // sidecar, load it and resolve each named state cell to a
    // [`FieldDomain`]. Cells with a sidecar entry get bounded per the
    // declared abstraction (BoundedCounter, EnumValues, …); cells
    // without an entry fall back to full bit-blast over their width.
    let mut cell_domains = build_cell_domains(&state_meta, &symbols, options, warnings)?;
    // Phase A.3 step 3.5 — apply partition to state cells. Each NID
    // not already keyed in `cell_domains` (i.e. not user-listed in the
    // sidecar) AND classified `Dropped` by the partition gets pinned
    // to `AbstractionType::Ignored` here.
    apply_partition_drops(
        &mut cell_domains,
        state_meta.iter().map(|m| (m.nid, m.symbol.as_str())),
        &partition,
        warnings,
        "state cell",
    );

    // R-Y3 (§Phase 8) — BTOR2 init-line smart defaults for cells
    // WITHOUT sidecar entries. Opt-in via `MUNUNU_BTOR2_SMART_INIT_DEFAULTS=1`.
    // Default OFF — full bit-blast remains the cap-safe legacy
    // behaviour. When enabled, each state cell with NO sidecar entry
    // AND a BTOR2 `Init` line gets a synthetic `EnumValues` entry
    // pinning it to its init value (1-state abstraction instead of
    // `2^width`).
    //
    // SOUNDNESS: collapsing an unsidecared cell to its init value is
    // an **under-approximation** of the design — sound for liveness
    // ("if reset state alone reaches the property, the property is
    // reachable in the real design too"), **unsound for safety**
    // ("property violations that depend on the cell deviating from
    // its init value are silently masked"). The opt-in surfaces this
    // tradeoff explicitly; users enable it when they want to ignore
    // cells they haven't bothered to declare AND accept the
    // soundness implication. Mirrors R-Y1's env-var pattern.
    apply_btor2_init_smart_defaults(
        file,
        &state_meta,
        &symbols,
        &mut cell_domains,
        warnings,
        options,
    );

    // R-S2a (§Phase 9 §9.1) — BTOR2 init-line seeding. For every
    // state cell with an `Init` line in the BTOR2 source AND a
    // sidecar-declared abstraction that takes per-value discriminators
    // (`EnumValues` with a value_map), inject the init value as an
    // additional discriminator if not already present. Bridges the
    // gap when R-S5 / R-S3 / R-S7 don't cover the init value of a
    // sidecar-declared signal (rare on typedef-typed Caliptra-style
    // designs; common on hand-written non-typedef RTL where the
    // reset value is the only "interesting" value before any input
    // arrives). Strictly additive — never replaces existing entries.
    apply_btor2_init_seeding(file, &state_meta, &symbols, &mut cell_domains);

    let cells = CellEnumeration::build(&state_meta, &cell_domains);

    // Phase 1.6: per-input sidecar resolution. The sidecar already
    // carries `inputs: [...]` entries via `SvAnnotation::inputs` (see
    // `crate::adapter::sidecar::btor2_resolver::build_input_field_domains`).
    // Without this, every input enumerates over its full bit-vector
    // width, which blocks real designs at the input-bit cap. With it,
    // an input declared `Ignored` / `Boolean` / `EnumValues` / etc.
    // collapses to a 1-of-N value list and the enumerator sees only
    // the meaningful combinations.
    let mut input_domains = build_input_domains(&input_meta, &symbols, options, warnings)?;
    // Clock inputs are auto-pinned by the bit-blaster regardless; the
    // partition's Dropped classification on a clock would be redundant
    // but harmless. We filter clocks out here only to avoid an
    // unnecessary "auto-COI dropped 'clk'" warning.
    apply_partition_drops(
        &mut input_domains,
        input_meta
            .iter()
            .filter(|m| !m.is_clock)
            .map(|m| (m.nid, m.symbol.as_str())),
        &partition,
        warnings,
        "input port",
    );
    let input_cells = InputCellEnumeration::build(&input_meta, &input_domains);

    // Effective input-bit cap. The previous cap (`MAX_INPUT_BITS`)
    // tested raw bit width and ran before sidecars resolved; that
    // rejected designs where most of the input width is masked off by
    // an abstraction. Now the cap tests the enumerated cardinality.
    let total_input_combos = input_cells.total_combos();
    let cap_combos: usize = 1usize << MAX_INPUT_BITS;
    if total_input_combos > cap_combos {
        return Err(AdapterError {
            kind: AdapterErrorKind::StateSpaceOverflow,
            message: format!(
                "BTOR2 design enumerates {total_input_combos} input combinations per step \
                 (max supported: 2^{MAX_INPUT_BITS} = {cap_combos}). Add `.mununu.json` \
                 `inputs[]` entries declaring `Ignored` / `Boolean` / `EnumValues` per \
                 non-essential input to prune the enumeration."
            ),
            location: None,
        });
    }
    let total_state_combos = cells.total_combos();

    let state_names = enumerate_state_names(total_state_combos);

    // Initial state(s) derived from `init` lines.
    //
    // Path 3 / Option A (§Phase 8 §8.2 residual closure): when a state
    // cell has NO `Node::Init` line (which is how Yosys's
    // `(* anyconst *)` attribute survives lowering to BTOR2), its
    // initial value is **nondeterministic** — under R-Y2 + R-S5, every
    // value in the cell's declared abstraction is an admissible init
    // sample. The bit-blaster used to silently default such cells to
    // zero, producing a single abstract initial state that hid the
    // anyconst nondeterminism from the verdict (the residual the Path 3
    // design note flagged on the Caliptra fixture).
    //
    // Stage 1: detect nondeterministic cells (no `Init` line in the
    // BTOR2 file).
    // Stage 2: enumerate the cartesian product of {deterministic init
    // value for each cell WITH init} × {all admissible values for each
    // cell WITHOUT init}, encode each combination as a state index.
    //
    // To bound state explosion when many anyconst cells coexist, the
    // cartesian product is capped at `MAX_INITIAL_STATES = 256`. On
    // cap exceedance, fall back to the legacy single-init behaviour
    // and emit a soundness warning so the user can narrow the
    // abstraction set or limit the per-signal init policy.
    const MAX_INITIAL_STATES: usize = 256;
    // Path 3 — restrict the nondeterministic-init set to the cells
    // the user EXPLICITLY marked `init_policy: anyconst` via R-Y2
    // sidecar. Yosys emits many cells without `Init` lines (reset
    // synchronizers, FSM intermediates, undef bits under
    // `setundef -anyseq/-anyconst` global policy); enumerating all
    // of them as initial blows up immediately. Surgical anyconst
    // declarations are the user's signal of intent.
    let anyconst_symbols = sidecar_anyconst_symbols(options);
    let nondet_init_nids =
        nondeterministic_init_cells(file, &state_meta, &symbols, &anyconst_symbols);

    // SOUNDNESS (§Phase 8 §8.2 — Path 3 follow-up): warn when state
    // cells silently default to zero. A cell without an `Init` line in
    // the BTOR2 source AND without a sidecar `init_policy: anyconst`
    // declaration is being deterministically pinned to zero by the
    // legacy init path. This is sound iff Yosys ran with
    // `setundef -zero` (the default) which explicitly zeroes undef
    // bits. Under `setundef -anyseq/-anyconst` (R-Y1 global flag or
    // R-Y2 per-signal attribute on a different cell) the cell's
    // initial value is nondeterministic, and defaulting to zero hides
    // a class of reset samples from the verdict.
    //
    // We cannot tell from BTOR2 alone which policy was used (Yosys
    // does not annotate the `state` lines with provenance), so this
    // warning is conservative: it fires whenever the situation
    // exists, and the user decides whether their flow makes it safe.
    let uncovered_uninit_signals: Vec<String> =
        uncovered_uninit_cells(file, &state_meta, &symbols, &anyconst_symbols);
    if !uncovered_uninit_signals.is_empty() {
        // Cap the listed names so the warning stays readable on large designs.
        let shown: Vec<&str> = uncovered_uninit_signals
            .iter()
            .take(5)
            .map(String::as_str)
            .collect();
        let suffix = if uncovered_uninit_signals.len() > shown.len() {
            format!(
                " (and {} more)",
                uncovered_uninit_signals.len() - shown.len()
            )
        } else {
            String::new()
        };
        warnings.push(AdapterWarning {
            kind: WarningKind::ApproximateTranslation,
            message: format!(
                "{n} state cell(s) without explicit `Init` line default to zero: \
                 {names}{suffix}. SOUND only if upstream Yosys ran with \
                 `setundef -zero` (the default). Under `setundef -anyseq/-anyconst` \
                 these cells' initial values are nondeterministic; declare them in \
                 the sidecar with `init_policy: anyconst` so Path 3 (§Phase 8 §8.2) \
                 enumerates their admissible initial values.",
                n = uncovered_uninit_signals.len(),
                names = shown.join(", "),
                suffix = suffix
            ),
            location: None,
        });
    }

    let init_state_indices: Vec<usize> = {
        let mut init_env = make_initial_env(file, &state_meta, &input_meta, true);
        evaluate_pure(file, &mut init_env, /*honor_init=*/ true)?;
        // R-Y6 (§Phase 8) — reset-sequence-aware init. When the
        // sidecar declares `reset_sequence: {...}`, run K cycles of
        // reset-asserted simulation before enumerating initial states.
        // The state after the K-cycle hold becomes the effective
        // initial state. No-op when the sidecar lacks the field.
        apply_reset_sequence(
            file,
            &mut init_env,
            &state_meta,
            &input_meta,
            &symbols,
            options,
        )?;
        // R-Y4 — bounded-init overrides from the sidecar.
        let bounded_init_overrides = sidecar_bounded_init_overrides(options, &state_meta, &symbols);
        let combos = enumerate_initial_combos(
            &init_env,
            &state_meta,
            &cells,
            &nondet_init_nids,
            &bounded_init_overrides,
        );
        if combos.len() > MAX_INITIAL_STATES {
            warnings.push(AdapterWarning {
                kind: WarningKind::ApproximateTranslation,
                message: format!(
                    "anyconst-driven initial state set has {n} entries (cap {cap}); \
                     falling back to single-init state — verdict at init is unsound \
                     for the nondeterministic init samples beyond the first.",
                    n = combos.len(),
                    cap = MAX_INITIAL_STATES
                ),
                location: None,
            });
            let single = encode_state(&init_env, &state_meta, &cells).unwrap_or(0);
            vec![single]
        } else if combos.is_empty() {
            // Fallback to legacy: no nondet cells, single init state.
            let single = encode_state(&init_env, &state_meta, &cells).unwrap_or(0);
            vec![single]
        } else {
            combos
        }
    };
    let init_state_set: std::collections::HashSet<usize> =
        init_state_indices.iter().copied().collect();

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
            let mut env = make_step_env(
                &state_meta,
                &input_meta,
                &cells,
                &input_cells,
                state_idx,
                input_idx,
            );
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
                labels: signal_labels_for_input(input_idx, &input_meta, &input_cells),
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
                is_initial: init_state_set.contains(&i),
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

    let mut properties = build_properties(
        file,
        &state_meta,
        &input_meta,
        &cells,
        &input_cells,
        &state_names,
    )?;
    // Append sidecar-declared mu-calculus properties. The custom SV
    // adapter (which constructs its own SvAnnotation flow) already
    // honours `SvAnnotation::properties`; this mirrors the same shape
    // on the BTOR2 / sv-yosys path so a user's `.mununu.json` can
    // carry hand-authored property formulas alongside abstractions.
    properties.extend(sidecar_properties(options));

    // Phase A.3 step 3.6 — partition summary. Pair the partition's
    // per-signal classification with the bit-blaster's known widths
    // so the user can read `state_bits_before` / `state_bits_after`
    // as observable evidence of COI's reduction effect.
    let widths: std::collections::HashMap<String, usize> = state_meta
        .iter()
        .map(|m| (m.symbol.clone(), m.width as usize))
        .chain(
            input_meta
                .iter()
                .map(|m| (m.symbol.clone(), m.width as usize)),
        )
        .collect();
    let partition_summary = Some(crate::adapter::partition::PartitionSummary::from_partition(
        &partition,
        Some(&widths),
    ));

    Ok(BlastOutput {
        signals,
        properties,
        automaton,
        partition_summary,
    })
}

/// Parse the sidecar's `properties[]` array (if any) into the
/// adapter-IR `PropertySpec` shape. Malformed entries are logged as
/// adapter warnings — same permissive policy as `build_cell_domains`.
fn sidecar_properties(options: &AdapterOptions) -> Vec<PropertySpec> {
    let Some(json) = &options.sidecar_json else {
        return vec![];
    };
    let annotation =
        match serde_json::from_str::<crate::adapter::systemverilog::annotation::SvAnnotation>(json)
        {
            Ok(a) => a,
            Err(_) => return vec![],
        };
    annotation
        .properties
        .into_iter()
        .filter_map(|p| {
            // Prefer an inline formula; template_ref support is the
            // custom SV adapter's responsibility (not the BTOR2 path).
            let body = p.formula?;
            Some(PropertySpec {
                name: p.id,
                kind: PropertyKind::Safety,
                formula: PropertyFormula::MuCalculus(body),
                role: match p.role.as_str() {
                    "assumption" => PropertyRole::Assumption,
                    "standalone" => PropertyRole::Standalone,
                    _ => PropertyRole::Guarantee,
                },
                over: None,
                description: p.description,
            })
        })
        .collect()
}

fn build_properties(
    file: &Btor2File,
    state_meta: &[StateMeta],
    input_meta: &[InputMeta],
    cells: &CellEnumeration,
    input_cells: &InputCellEnumeration,
    state_names: &[String],
) -> Result<Vec<PropertySpec>, AdapterError> {
    let mut out = Vec::new();
    let total_input_combos = input_cells.total_combos();

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
                let mut env = make_step_env(
                    state_meta,
                    input_meta,
                    cells,
                    input_cells,
                    state_idx,
                    input_idx,
                );
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

    // Justice properties → liveness: nu Y. ((mu X. (pred || <> X)) && ([] Y))
    for (i, line) in file.justices().enumerate() {
        let signals = match &line.node {
            Node::Justice { signals } => signals.clone(),
            _ => unreachable!(),
        };
        let mut hit = Vec::new();
        for (state_idx, state_name) in state_names.iter().enumerate() {
            let mut found = false;
            for input_idx in 0..total_input_combos {
                let mut env = make_step_env(
                    state_meta,
                    input_meta,
                    cells,
                    input_cells,
                    state_idx,
                    input_idx,
                );
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
                let mut env = make_step_env(
                    state_meta,
                    input_meta,
                    cells,
                    input_cells,
                    state_idx,
                    input_idx,
                );
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

/// Phase A.3 step 3.5 — inject `AbstractionType::Ignored` for signals
/// the partition classified `Dropped` *and* the user did not list in
/// the sidecar.
///
/// User-listed signals are identified by membership of their NID in
/// `domains` (the sidecar resolver only inserts entries for names that
/// appear in the sidecar's `signals[]` / `inputs[]` arrays). The
/// partition's verdict is keyed by symbol; we look up the symbol for
/// each NID via `iter` and skip anonymous signals (no symbol available).
///
/// SOUNDNESS: see `crate::adapter::partition` module docs. The injected
/// `Ignored` domain pins the signal to its initial value; the abstract
/// model thereby admits more behaviours than the concrete model — sound
/// for safety + over-approximation.
fn apply_partition_drops<'a>(
    domains: &mut CellDomainMap,
    iter: impl Iterator<Item = (i64, &'a str)>,
    partition: &crate::adapter::partition::Partition,
    warnings: &mut Vec<AdapterWarning>,
    kind: &str,
) {
    use crate::adapter::domain::{AbstractValue, AbstractionType, FieldDomain};
    use crate::adapter::partition::PartitionClass;

    for (nid, symbol) in iter {
        // User wins: an entry already keyed by NID in `domains` means
        // the sidecar listed this signal — trust the user.
        if domains.contains_key(&nid) {
            continue;
        }
        match partition.classes.get(symbol) {
            Some(PartitionClass::Dropped { reason }) => {
                warnings.push(AdapterWarning {
                    kind: WarningKind::ApproximateTranslation,
                    message: format!(
                        "auto-partition: {kind} '{symbol}' dropped ({reason}); add an explicit sidecar entry to override"
                    ),
                    location: None,
                });
                domains.insert(
                    nid,
                    (
                        FieldDomain {
                            name: symbol.to_string(),
                            abstraction: AbstractionType::Ignored,
                            bound: None,
                            lower_bound: None,
                            variants: None,
                            initial: AbstractValue::Counter(0),
                        },
                        Vec::new(),
                    ),
                );
            }
            _ => {
                // Kept (or absent for symbols outside the partition's
                // signal universe — clocks, anonymous synthesisers).
                // Leave `domains` untouched; the existing fall-through
                // (full bit-blast) applies.
            }
        }
    }
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
    /// Per state cell, whether `encode` should saturate out-of-range
    /// values to the nearest in-range value (true for `BoundedCounter`
    /// abstraction; false otherwise). Saturating maps the design's
    /// arbitrary concrete writes (e.g. `wait_count = 5`) onto the
    /// declared abstract counter range (e.g. `bound = 1` → values
    /// `{0, 1}`, with `5` saturating to `1` = the "non-zero" class).
    /// This is an over-approximation: a smaller abstract domain
    /// represents a larger concrete one. Sound for safety; under-
    /// approximates liveness. See `docs/abstraction.md` for the
    /// memory-soundness matrix.
    saturate: Vec<bool>,
}

impl CellEnumeration {
    fn build(state_meta: &[StateMeta], cell_domains: &CellDomainMap) -> Self {
        use crate::adapter::domain::AbstractionType;
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
        let saturate: Vec<bool> = state_meta
            .iter()
            .map(|sm| {
                cell_domains
                    .get(&sm.nid)
                    .map(|(fd, _)| fd.abstraction == AbstractionType::BoundedCounter)
                    .unwrap_or(false)
            })
            .collect();
        CellEnumeration {
            per_cell,
            radices,
            saturate,
        }
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
    /// Returns `None` if any value is not in its cell's allowed set
    /// AND the cell does not declare saturating semantics — this
    /// happens when the design's transition function lands a state
    /// outside the sidecar-declared abstraction (e.g., an `EnumValues`
    /// signal landing outside its variant set). The caller treats
    /// this as out-of-bounds.
    ///
    /// For cells whose `saturate` flag is set (`BoundedCounter`), an
    /// out-of-range value is clamped to the nearest in-range value:
    /// `v > max(per_cell)` → `max`; `v < min(per_cell)` → `min`. This
    /// is the standard saturating-counter semantics — a soundness-
    /// preserving over-approximation that lets the abstract domain
    /// stand in for an unbounded concrete domain without dropping
    /// transitions. The classic use case is `bound = 1` standing in
    /// for `{0, non-zero}`: the design's `wait_count = 5` write
    /// saturates to `1` (= the "non-zero" abstract class).
    fn encode(&self, values: &[u128]) -> Option<usize> {
        let mut combo = 0usize;
        let mut multiplier = 1usize;
        for (i, &v) in values.iter().enumerate() {
            let radix = self.radices[i];
            let cell_values = &self.per_cell[i];
            let idx = cell_values.iter().position(|x| *x == v).or_else(|| {
                if !self.saturate[i] || cell_values.is_empty() {
                    return None;
                }
                // `per_cell` is sorted ascending in `build`. Clamp.
                let last = *cell_values.last()?;
                let first = *cell_values.first()?;
                if v > last {
                    Some(cell_values.len() - 1)
                } else if v < first {
                    Some(0)
                } else {
                    // Value sits in a gap within the declared range.
                    // For `BoundedCounter` the range is contiguous,
                    // so this branch is unreachable in practice; if
                    // the abstraction ever gets gappy variants the
                    // saturation is undefined and we drop the
                    // transition (under-approx) rather than guess.
                    None
                }
            })?;
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

/// Per-input enumeration plan — sibling to [`CellEnumeration`].
///
/// Each non-clock input carries a `Vec<u128>` of allowed concrete
/// values, sourced from the sidecar's `inputs[]` `FieldDomain`. Inputs
/// without a sidecar entry fall back to full bit-blast over their
/// declared BTOR2 width. **Clock inputs are pinned to a single value
/// (1, posedge active)** — they are not part of the enumerated input
/// space, matching the prior `combinations_of_inputs` semantics.
///
/// The total input-combination count is the product of per-input
/// value-set sizes. The bit-blaster's transition loops iterate
/// `0..total_combos()` and call [`Self::value_at`] per input nid.
struct InputCellEnumeration {
    per_input: Vec<Vec<u128>>,
    radices: Vec<usize>,
}

impl InputCellEnumeration {
    fn build(input_meta: &[InputMeta], input_domains: &CellDomainMap) -> Self {
        let per_input: Vec<Vec<u128>> = input_meta
            .iter()
            .map(|im| {
                if im.is_clock {
                    // Implicit clock: pinned to 1 (posedge active).
                    return vec![1u128];
                }
                if let Some((fd, vm)) = input_domains.get(&im.nid) {
                    CellEnumeration::values_for_field_domain(fd, vm, im.width)
                } else {
                    let cap = im.width.min(31);
                    (0..(1u128 << cap)).collect()
                }
            })
            .collect();
        let radices: Vec<usize> = per_input.iter().map(|v| v.len().max(1)).collect();
        InputCellEnumeration { per_input, radices }
    }

    fn total_combos(&self) -> usize {
        self.radices
            .iter()
            .fold(1usize, |a, r| a.saturating_mul(*r))
    }

    /// Per-input value at this combo for the input at `input_idx`.
    fn value_at(&self, combo: usize, input_idx: usize) -> u128 {
        let mut rem = combo;
        for (i, radix) in self.radices.iter().enumerate() {
            let pick = rem % radix;
            if i == input_idx {
                return *self.per_input[i].get(pick).unwrap_or(&0);
            }
            rem /= radix;
        }
        0
    }
}

/// Sidecar resolution for inputs — mirrors [`build_cell_domains`] but
/// scopes the symbol filter to input NIDs and delegates to
/// [`build_input_field_domains`](crate::adapter::sidecar::btor2_resolver::build_input_field_domains).
fn build_input_domains(
    input_meta: &[InputMeta],
    symbols: &std::collections::HashMap<i64, String>,
    options: &AdapterOptions,
    warnings: &mut Vec<AdapterWarning>,
) -> Result<CellDomainMap, AdapterError> {
    let Some(json) = &options.sidecar_json else {
        return Ok(std::collections::HashMap::new());
    };
    let annotation = match serde_json::from_str::<
        crate::adapter::systemverilog::annotation::SvAnnotation,
    >(json)
    {
        Ok(a) => a,
        Err(e) => {
            warnings.push(AdapterWarning {
                kind: WarningKind::UnsupportedConstruct,
                message: format!(
                    "adapter/btor2: failed to parse .mununu.json sidecar for inputs ({e}); falling back to full bit-blast"
                ),
                location: None,
            });
            return Ok(std::collections::HashMap::new());
        }
    };
    let input_symbols: std::collections::HashMap<i64, String> = input_meta
        .iter()
        .filter_map(|im| symbols.get(&im.nid).map(|s| (im.nid, s.clone())))
        .collect();
    Ok(
        crate::adapter::sidecar::btor2_resolver::build_input_field_domains(
            &annotation,
            &input_symbols,
        ),
    )
}

#[allow(dead_code)]
fn combinations_of(meta: &[StateMeta]) -> usize {
    meta.iter()
        .fold(1usize, |acc, m| acc.saturating_mul(1usize << m.width))
}

// `combinations_of_inputs` was removed in Phase 1.6 — input enumeration
// now goes through `InputCellEnumeration::total_combos`, which respects
// per-input `FieldDomain` abstractions from the sidecar.

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
fn signal_labels_for_input(
    combo: usize,
    meta: &[InputMeta],
    input_cells: &InputCellEnumeration,
) -> Vec<String> {
    let labels: Vec<String> = meta
        .iter()
        .enumerate()
        .filter(|(_, im)| !im.is_clock)
        .map(|(i, im)| {
            let v = input_cells.value_at(combo, i);
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
///
/// Superseded in Phase 1.6 by [`InputCellEnumeration::value_at`], which
/// respects per-input `FieldDomain` mixed-radix encoding. Kept for
/// reference only.
#[allow(dead_code)]
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
    input_cells: &InputCellEnumeration,
    state_idx: usize,
    input_idx: usize,
) -> Env {
    let mut env = Env::default();
    for (i, sm) in state_meta.iter().enumerate() {
        let bits = cells.value_at(state_idx, i);
        env.values.insert(sm.nid, BvValue::new(bits, sm.width));
    }
    for (i, im) in input_meta.iter().enumerate() {
        let bits = input_cells.value_at(input_idx, i);
        env.values.insert(im.nid, BvValue::new(bits, im.width));
    }
    env
}

/// Path 3 / Option A (§Phase 8 §8.2 residual closure) — return the
/// NIDs of state cells the user EXPLICITLY marked `init_policy: anyconst`
/// in the sidecar.
///
/// Restricted to user-declared anyconst (not every BTOR2 cell without
/// an `Init` line) because Yosys emits many cells without init lines
/// for non-anyconst reasons (reset synchronizers, undef bits under
/// the global `setundef` policy, FSM intermediates). Enumerating all
/// of them as initial blows up immediately; the explicit per-signal
/// anyconst declaration is the user's signal of intent.
///
/// `anyconst_symbols` is the set of signal names from the sidecar's
/// `signals[].init_policy == anyconst` entries (see
/// [`sidecar_anyconst_symbols`]). Empty set ⇒ no Path 3 enumeration;
/// falls back to the legacy single-init behaviour.
/// R-S2a (§Phase 9 §9.1) — BTOR2 init-line seeding. Walks every
/// `Node::Init { state, value }` line in the BTOR2 file; for each
/// state cell with a sidecar-declared `EnumValues` abstraction whose
/// value_map does NOT already contain the resolved init value, adds
/// a synthetic discriminator `(<signal>_<value>, value)` to the
/// cell's value_map AND appends `<signal>_<value>` to its variants
/// list.
///
/// Resolves the init value by reading the `value` operand's NID and
/// looking up its constant value (`Node::Const` / `Node::Op::Zero` /
/// `Node::Op::One` / `Node::Op::Ones`). Init lines whose value is a
/// non-constant expression are silently skipped — those require the
/// full `evaluate_pure` pass which runs later for the init-state
/// computation; rare in practice (Yosys typically emits constant
/// init values).
///
/// Bridges the gap when R-S5 / R-S3 / R-S7 don't cover the signal's
/// init value. Strictly additive — never replaces existing entries.
/// R-Y3 (§Phase 8) — BTOR2 init-line smart defaults for cells
/// WITHOUT sidecar entries. Opt-in via `MUNUNU_BTOR2_SMART_INIT_DEFAULTS=1`.
///
/// For each state cell with NO entry in `cell_domains` AND a BTOR2
/// `Init` line resolvable to a constant, inserts a synthetic
/// `EnumValues` `FieldDomain` pinning the cell to its init value.
/// The synthetic variant name is `<signal>_<init_value>`.
///
/// Effect: `CellEnumeration::build` sees the cell as if the user had
/// declared `{abstraction: enum, variants: [<signal>_<init>],
/// value_map: [{<signal>_<init>: <init>}]}` in the sidecar. State
/// space for that cell collapses from `2^width` (full bit-blast) to 1.
///
/// SOUNDNESS: see the call-site comment in `enumerate_and_blast`.
/// Under-approximation; sound for liveness, unsound for safety. Emits
/// an adapter warning naming the affected cells so users see what
/// was pinned.
///
/// No-op when the env var is unset OR when the design has no
/// unsidecared init-line cells.
fn apply_btor2_init_smart_defaults(
    file: &Btor2File,
    state_meta: &[StateMeta],
    symbols: &std::collections::HashMap<i64, String>,
    cell_domains: &mut CellDomainMap,
    warnings: &mut Vec<AdapterWarning>,
    options: &AdapterOptions,
) {
    let enabled = options.smart_init_defaults
        || std::env::var("MUNUNU_BTOR2_SMART_INIT_DEFAULTS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    if !enabled {
        return;
    }
    use crate::adapter::domain::{AbstractValue, AbstractionType, FieldDomain};

    // Build a NID → width lookup.
    let nid_to_width: std::collections::HashMap<Nid, u32> =
        state_meta.iter().map(|sm| (sm.nid, sm.width)).collect();

    // Walk Init lines; build (state_nid, init_value) pairs.
    let mut pinned: Vec<(Nid, u64)> = Vec::new();
    for line in &file.lines {
        let Node::Init { state, value, .. } = &line.node else {
            continue;
        };
        // Only consider cells without sidecar entries.
        if cell_domains.contains_key(state) {
            continue;
        }
        let Some(init_value) = resolve_btor2_constant(file, value.nid()) else {
            continue;
        };
        let Some(width) = nid_to_width.get(state) else {
            continue;
        };
        let masked = if *width >= 64 {
            init_value
        } else {
            init_value & ((1u64 << *width) - 1)
        };
        pinned.push((*state, masked));
    }

    if pinned.is_empty() {
        return;
    }

    // Inject synthetic sidecar entries.
    let mut pinned_names: Vec<String> = Vec::new();
    for (nid, val) in &pinned {
        let signal_name = symbols
            .get(nid)
            .cloned()
            .unwrap_or_else(|| format!("nid_{nid}"));
        let variant_name = format!("{}_{}", signal_name, *val);
        let fd = FieldDomain {
            name: signal_name.clone(),
            abstraction: AbstractionType::EnumValues,
            bound: None,
            lower_bound: None,
            variants: Some(vec![variant_name.clone()]),
            initial: AbstractValue::Variant(variant_name.clone()),
        };
        let value_map = vec![(variant_name, *val as i64)];
        cell_domains.insert(*nid, (fd, value_map));
        pinned_names.push(signal_name);
    }
    pinned_names.sort();
    pinned_names.dedup();

    let shown: Vec<&str> = pinned_names.iter().take(5).map(String::as_str).collect();
    let suffix = if pinned_names.len() > shown.len() {
        format!(" (and {} more)", pinned_names.len() - shown.len())
    } else {
        String::new()
    };
    warnings.push(AdapterWarning {
        kind: WarningKind::ApproximateTranslation,
        message: format!(
            "R-Y3: pinned {n} unsidecared state cell(s) to their BTOR2 init values \
             (smart-init-defaults policy enabled): {names}{suffix}. SOUNDNESS: \
             under-approximation — sound for liveness, unsound for safety. \
             Property violations that require these cells to deviate from their \
             init values are silently masked. Disable via unsetting \
             MUNUNU_BTOR2_SMART_INIT_DEFAULTS or declare the affected signals in \
             the sidecar to override the pinning.",
            n = pinned_names.len(),
            names = shown.join(", "),
            suffix = suffix
        ),
        location: None,
    });
}

fn apply_btor2_init_seeding(
    file: &Btor2File,
    state_meta: &[StateMeta],
    symbols: &std::collections::HashMap<i64, String>,
    cell_domains: &mut CellDomainMap,
) {
    // Build a NID → state_meta index for fast lookup of widths.
    let nid_to_width: std::collections::HashMap<Nid, u32> =
        state_meta.iter().map(|sm| (sm.nid, sm.width)).collect();

    for line in &file.lines {
        let Node::Init { state, value, .. } = &line.node else {
            continue;
        };
        // Only seed cells that have a sidecar entry. Cells without
        // one fall back to full bit-blast over their width — init
        // value is already admissible.
        let Some((fd, value_map)) = cell_domains.get_mut(state) else {
            continue;
        };
        // Only widen `EnumValues` abstractions; `BoundedCounter` and
        // friends use a different value-set construction that does
        // not benefit from per-value variant names.
        if !matches!(
            fd.abstraction,
            crate::adapter::domain::AbstractionType::EnumValues
        ) {
            continue;
        }
        // Resolve the init value's constant via the value operand's NID.
        let Some(init_value) = resolve_btor2_constant(file, value.nid()) else {
            continue;
        };
        // Mask to the cell's bit-width.
        let Some(width) = nid_to_width.get(state) else {
            continue;
        };
        let masked = if *width >= 64 {
            init_value
        } else {
            init_value & ((1u64 << *width) - 1)
        };
        let masked_signed = masked as i64;
        // Skip if init value already in value_map.
        if value_map.iter().any(|(_, v)| *v == masked_signed) {
            continue;
        }
        // Augment: append new (name, value) to value_map AND name to variants.
        let signal_name = symbols
            .get(state)
            .cloned()
            .unwrap_or_else(|| format!("nid_{state}"));
        let variant_name = format!("{}_{}", signal_name, masked_signed);
        value_map.push((variant_name.clone(), masked_signed));
        match &mut fd.variants {
            Some(v) => v.push(variant_name),
            None => fd.variants = Some(vec![variant_name]),
        }
    }
}

/// R-S2a helper — resolve a BTOR2 NID to its constant `u64` value if
/// it's one of the simple constant nodes mununu's bit-blaster
/// recognises: `Node::Const { value: Binary/Decimal/Hex }`, or one
/// of the `Op::Zero` / `Op::One` / `Op::Ones` shortcuts that Yosys
/// emits frequently for init lines.
///
/// Returns `None` for any non-constant NID (operations on other
/// signals, references to inputs, etc.).
fn resolve_btor2_constant(file: &Btor2File, nid: Nid) -> Option<u64> {
    for line in &file.lines {
        if line.nid != nid {
            continue;
        }
        let Node::Const { value, sort } = &line.node else {
            return None;
        };
        let width = parser::bv_width(file, *sort)?;
        let mask = if width >= 64 {
            u64::MAX
        } else {
            (1u64 << width) - 1
        };
        return match value {
            ConstValue::Zero => Some(0),
            ConstValue::One => Some(1 & mask),
            ConstValue::Ones => Some(mask),
            ConstValue::Bin(s) => u64::from_str_radix(s, 2).ok().map(|v| v & mask),
            ConstValue::Dec(n) => {
                let raw = if *n < 0 {
                    (mask.wrapping_sub((-*n) as u64).wrapping_add(1)) & mask
                } else {
                    (*n as u64) & mask
                };
                Some(raw)
            }
            ConstValue::Hex(s) => u64::from_str_radix(s, 16).ok().map(|v| v & mask),
        };
    }
    None
}

fn nondeterministic_init_cells(
    file: &Btor2File,
    state_meta: &[StateMeta],
    symbols: &std::collections::HashMap<i64, String>,
    anyconst_symbols: &std::collections::HashSet<String>,
) -> std::collections::HashSet<Nid> {
    if anyconst_symbols.is_empty() {
        return std::collections::HashSet::new();
    }
    let mut has_init: std::collections::HashSet<Nid> = std::collections::HashSet::new();
    for line in &file.lines {
        if let Node::Init { state, .. } = &line.node {
            has_init.insert(*state);
        }
    }
    state_meta
        .iter()
        .filter(|sm| !has_init.contains(&sm.nid))
        .filter(|sm| {
            symbols
                .get(&sm.nid)
                .map(|name| anyconst_symbols.contains(name))
                .unwrap_or(false)
        })
        .map(|sm| sm.nid)
        .collect()
}

/// Soundness companion to [`nondeterministic_init_cells`] — return
/// the signal names of state cells that have NO `Init` line AND are
/// NOT in `anyconst_symbols`. These cells silently default to zero
/// in the legacy init path; the caller emits a SOUNDNESS warning so
/// the user knows whether their upstream `setundef` policy makes
/// that safe (sound under `-zero`; unsound under `-anyseq/-anyconst`).
///
/// Cells without a recoverable symbol name (Yosys synthetic state
/// cells without user-visible names) are silently skipped — they
/// don't appear in the user-facing warning either way, and Yosys
/// almost always emits Init lines for synthetic intermediate cells
/// it created itself.
fn uncovered_uninit_cells(
    file: &Btor2File,
    state_meta: &[StateMeta],
    symbols: &std::collections::HashMap<i64, String>,
    anyconst_symbols: &std::collections::HashSet<String>,
) -> Vec<String> {
    let mut has_init: std::collections::HashSet<Nid> = std::collections::HashSet::new();
    for line in &file.lines {
        if let Node::Init { state, .. } = &line.node {
            has_init.insert(*state);
        }
    }
    let mut out: Vec<String> = Vec::new();
    for sm in state_meta {
        if has_init.contains(&sm.nid) {
            continue;
        }
        let Some(name) = symbols.get(&sm.nid) else {
            continue;
        };
        if anyconst_symbols.contains(name) {
            continue;
        }
        out.push(name.clone());
    }
    out.sort();
    out.dedup();
    out
}

/// Path 3 — collect the set of signal names declared with
/// `init_policy: anyconst` in the sidecar. Used by
/// [`nondeterministic_init_cells`] to restrict init enumeration to
/// user-declared anyconst cells (avoiding the state explosion on
/// every Yosys-emitted cell-without-init).
fn sidecar_anyconst_symbols(options: &AdapterOptions) -> std::collections::HashSet<String> {
    let Some(json) = &options.sidecar_json else {
        return std::collections::HashSet::new();
    };
    let Ok(ann) =
        serde_json::from_str::<crate::adapter::systemverilog::annotation::SvAnnotation>(json)
    else {
        return std::collections::HashSet::new();
    };
    ann.init_policy_overrides()
        .into_iter()
        .filter(|(_, p)| {
            matches!(
                p,
                crate::adapter::systemverilog::annotation::InitPolicy::Anyconst
            )
        })
        .map(|(name, _)| name)
        .collect()
}

/// Path 3 / Option A — enumerate the cartesian product of admissible
/// initial-state combinations. Cells WITH an init line contribute
/// their single pinned value (from the already-evaluated `init_env`);
/// cells WITHOUT an init line contribute every value in their
/// `CellEnumeration` per-cell admissible set.
///
/// Returns an empty vec when there are no nondeterministic cells —
/// signals the caller to use the legacy single-init path. Cells
/// whose pinned init value isn't in their declared abstraction are
/// silently dropped (cannot encode); a deterministic init outside
/// the abstraction was already a soundness issue pre-Path 3.
fn enumerate_initial_combos(
    init_env: &Env,
    state_meta: &[StateMeta],
    cells: &CellEnumeration,
    nondet_nids: &std::collections::HashSet<Nid>,
    bounded_init_overrides: &std::collections::HashMap<Nid, Vec<u128>>,
) -> Vec<usize> {
    if nondet_nids.is_empty() {
        return Vec::new();
    }
    // Build per-cell admissible-init-value lists.
    let per_cell_init: Vec<Vec<u128>> = state_meta
        .iter()
        .enumerate()
        .map(|(i, sm)| {
            if nondet_nids.contains(&sm.nid) {
                // R-Y4 (§Phase 8) — if the sidecar declared
                // `bounded_init: [...]` for this signal, restrict
                // the per-cell init set to those values. The
                // bounded-init values must already be in the cell's
                // abstract admissible set (`cells.per_cell[i]`);
                // out-of-set values are silently dropped by the
                // cartesian-product encoder (cells.encode returns
                // None for unrepresentable combinations).
                if let Some(bounded) = bounded_init_overrides.get(&sm.nid) {
                    bounded.clone()
                } else {
                    // Anyconst-style: every value in the declared abstraction.
                    cells.per_cell[i].clone()
                }
            } else {
                // Deterministic: use the pinned init value.
                let bits = init_env
                    .values
                    .get(&sm.nid)
                    .copied()
                    .unwrap_or(BvValue::zero(sm.width))
                    .bits;
                vec![bits]
            }
        })
        .collect();
    // Cartesian product → encode each combination.
    let mut combos: Vec<usize> = Vec::new();
    let mut current: Vec<u128> = vec![0; per_cell_init.len()];
    cartesian_collect_combos(&per_cell_init, 0, &mut current, &mut combos, cells);
    combos.sort_unstable();
    combos.dedup();
    combos
}

/// R-Y6 (§Phase 8) — apply the sidecar-declared reset-hold sequence
/// to the init env. Pins the named input signal to its asserted value
/// and runs `hold_cycles` cycles of `evaluate_pure` + `apply_next`,
/// mutating `init_env` to reflect the state AFTER the reset hold.
///
/// No-op when:
/// - The sidecar JSON is absent / unparseable.
/// - No `reset_sequence` field is declared in the SvAnnotation.
/// - `hold_cycles == 0`.
/// - The named reset input does not resolve to a BTOR2 input symbol
///   (silently skipped to keep the precondition behaviour additive).
///
/// SOUNDNESS: this is an exact transformation of the init env per the
/// design's own reset semantics — the K-cycle hold is what the
/// downstream verification engine would do anyway if it could
/// observe time before cycle 0. No abstraction tradeoff; the init
/// state set after R-Y6 represents "what the design's state is after
/// the reset hold settles", which is the intended initial state for
/// most real designs with multi-stage reset synchronisers (e.g.
/// OpenTitan's `prim_reset_sync`).
fn apply_reset_sequence(
    file: &Btor2File,
    init_env: &mut Env,
    state_meta: &[StateMeta],
    input_meta: &[InputMeta],
    symbols: &std::collections::HashMap<i64, String>,
    options: &AdapterOptions,
) -> Result<(), AdapterError> {
    let Some(json) = &options.sidecar_json else {
        return Ok(());
    };
    let Ok(ann) =
        serde_json::from_str::<crate::adapter::systemverilog::annotation::SvAnnotation>(json)
    else {
        return Ok(());
    };
    let Some(seq) = ann.reset_sequence.as_ref() else {
        return Ok(());
    };
    if seq.hold_cycles == 0 {
        return Ok(());
    }
    // Resolve reset input name to BTOR2 input NID + width.
    let reset_nid_width: Option<(Nid, u32)> = input_meta
        .iter()
        .filter_map(|im| {
            symbols.get(&im.nid).and_then(|name| {
                if name == &seq.reset_input {
                    Some((im.nid, im.width))
                } else {
                    None
                }
            })
        })
        .next();
    let Some((reset_nid, width)) = reset_nid_width else {
        // Silently skip — the reset_input name didn't match any BTOR2
        // input. Additive: leaves init_env unchanged.
        return Ok(());
    };
    let mask = if width >= 128 {
        u128::MAX
    } else {
        (1u128 << width) - 1
    };
    let asserted_bv = BvValue::new(seq.asserted_value as u128 & mask, width);
    for _ in 0..seq.hold_cycles {
        // Pin the reset input to its asserted value.
        init_env.values.insert(reset_nid, asserted_bv);
        // Re-evaluate pure expressions (constants, propagation through
        // combinational logic) — honor_init=false because we're past
        // cycle 0 now.
        evaluate_pure(file, init_env, /*honor_init=*/ false)?;
        // Advance state cells via their next-state functions.
        apply_next(file, init_env, state_meta)?;
    }
    Ok(())
}

/// R-Y4 (§Phase 8) — collect per-signal bounded-init overrides from
/// the sidecar. For each `signals[]` entry with both
/// `init_policy: anyconst` AND a non-empty `bounded_init: [...]`,
/// returns `(NID, [values])` keyed by the BTOR2 state-cell NID.
///
/// Values are masked to the cell's bit-width to avoid surprising
/// encode failures on values that span more bits than the cell admits.
/// Cells whose symbol does not resolve to a state-meta entry are
/// silently skipped (defensive — typically can't happen if the
/// sidecar names match the SV source).
fn sidecar_bounded_init_overrides(
    options: &AdapterOptions,
    state_meta: &[StateMeta],
    symbols: &std::collections::HashMap<i64, String>,
) -> std::collections::HashMap<Nid, Vec<u128>> {
    let Some(json) = &options.sidecar_json else {
        return std::collections::HashMap::new();
    };
    let Ok(ann) =
        serde_json::from_str::<crate::adapter::systemverilog::annotation::SvAnnotation>(json)
    else {
        return std::collections::HashMap::new();
    };
    let name_to_nid_width: std::collections::HashMap<&str, (Nid, u32)> = state_meta
        .iter()
        .filter_map(|sm| {
            symbols
                .get(&sm.nid)
                .map(|n| (n.as_str(), (sm.nid, sm.width)))
        })
        .collect();
    let mut out = std::collections::HashMap::new();
    for sig in &ann.signals {
        if !matches!(
            sig.init_policy,
            crate::adapter::systemverilog::annotation::InitPolicy::Anyconst
        ) {
            continue;
        }
        let Some(bounded) = sig.bounded_init.as_ref() else {
            continue;
        };
        if bounded.is_empty() {
            continue;
        }
        let Some((nid, width)) = name_to_nid_width.get(sig.name.as_str()) else {
            continue;
        };
        let mask = if *width >= 128 {
            u128::MAX
        } else {
            (1u128 << *width) - 1
        };
        let values: Vec<u128> = bounded.iter().map(|v| (*v as u128) & mask).collect();
        out.insert(*nid, values);
    }
    out
}

/// Recursive helper for `enumerate_initial_combos` — fills the
/// `combos` vec with every encoding of every value-vector in the
/// cartesian product of `per_cell_init`. Encodings that fall outside
/// the cell domain are silently dropped (cannot happen under R-S5's
/// invariant that the typedef-derived abstraction set is complete).
fn cartesian_collect_combos(
    per_cell: &[Vec<u128>],
    depth: usize,
    current: &mut Vec<u128>,
    out: &mut Vec<usize>,
    cells: &CellEnumeration,
) {
    if depth == per_cell.len() {
        if let Some(idx) = cells.encode(current) {
            out.push(idx);
        }
        return;
    }
    for &v in &per_cell[depth] {
        current[depth] = v;
        cartesian_collect_combos(per_cell, depth + 1, current, out, cells);
    }
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
    let (ir, warnings, partition_summary) = to_ir(&file, options)?;

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
        sidecars: Vec::new(),
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
        partition_summary,
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
        // `MAX_STATE_BITS + 1` 1-bit states → 2^(N+1) > MAX_STATE_BITS.
        let mut src = "1 sort bitvec 1\n".to_string();
        let overflow_count = MAX_STATE_BITS as usize + 1;
        for i in 0..overflow_count {
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
    fn input_pruning_via_sidecar_unlocks_designs_past_raw_input_cap() {
        // Twelve 1-bit inputs → raw input width 12, exceeds
        // MAX_INPUT_BITS = 10. Without sidecar pruning, translation
        // must reject. With a sidecar declaring three of the inputs
        // as `ignored`, the effective input space is 2^9 = 512, well
        // under the cap, and translation succeeds.
        let mut src = "1 sort bitvec 1\n".to_string();
        src.push_str("2 zero 1\n");
        src.push_str("3 state 1 q\n");
        src.push_str("4 init 1 3 2\n");
        for i in 0..12 {
            src.push_str(&format!("{} input 1 in{}\n", i + 5, i));
        }

        // 1) Without sidecar — should reject on the input-bit cap.
        let bare = translate(&src, &AdapterOptions::default());
        assert!(bare.is_err(), "expected raw-input-cap rejection");
        let err = bare.unwrap_err();
        assert_eq!(err.kind, AdapterErrorKind::StateSpaceOverflow);

        // 2) With a sidecar pruning three inputs to `ignored`, the
        // effective input space drops below the cap and translation
        // succeeds.
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "demo",
            "signals": [],
            "inputs": [
                { "name": "in0", "abstraction": "ignored" },
                { "name": "in1", "abstraction": "ignored" },
                { "name": "in2", "abstraction": "ignored" },
            ],
        })
        .to_string();
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar),
            ..Default::default()
        };
        let out = translate(&src, &opts).expect("sidecar-pruned design should translate");
        assert!(out.ctxdsl.contains("automaton"));
    }

    #[test]
    fn input_pruning_ignored_inputs_collapse_to_one_value() {
        // One state, two inputs. Without sidecar: 4 input combos
        // → 2 transitions per input combo across 2 states. With
        // sidecar declaring one input as `ignored`: 2 input combos
        // total — the enumeration count is observably smaller.
        let mut src = "1 sort bitvec 1\n".to_string();
        src.push_str("2 zero 1\n");
        src.push_str("3 state 1 q\n");
        src.push_str("4 init 1 3 2\n");
        src.push_str("5 input 1 keep\n");
        src.push_str("6 input 1 drop\n");
        src.push_str("7 next 1 3 5\n"); // q' = keep (drop is unused)

        // Sidecar pins `drop` to 0.
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "demo",
            "signals": [],
            "inputs": [
                { "name": "drop", "abstraction": "ignored" },
            ],
        })
        .to_string();
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar),
            ..Default::default()
        };
        let out = translate(&src, &opts).expect("translate");
        // Labels should mention `keep_0` and `keep_1` but never
        // `drop_1` (drop is pinned to 0).
        let ctxdsl = &out.ctxdsl;
        assert!(
            ctxdsl.contains("keep_0") || ctxdsl.contains("keep_1"),
            "expected keep-labeled transitions; ctxdsl was:\n{ctxdsl}"
        );
        assert!(
            !ctxdsl.contains("drop_1"),
            "drop should be pinned to 0; ctxdsl was:\n{ctxdsl}"
        );
    }

    #[test]
    fn partition_summary_populated_on_adapter_output() {
        // Phase A.3 step 3.6 — BTOR2 adapter populates
        // `AdapterOutput.partition_summary` with the right counts and
        // bit-width totals.
        //
        // Fixture: one named state `s_keep` reachable from a `bad` line
        // (Kept), one named state `s_drop` not reachable (Dropped),
        // and one named input `trigger` that drives `s_keep` (Kept by
        // transitive walk). state_bits_before = 1+1 = 2, state_bits_after
        // = 1 (after dropping s_drop's 1-bit width).
        let src = r#"
1 sort bitvec 1
2 zero 1
3 input 1 trigger
4 state 1 s_keep
5 init 1 4 2
6 next 1 4 3
7 state 1 s_drop
8 init 1 7 2
9 next 1 7 2
10 bad 4
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        let summary = out
            .partition_summary
            .expect("partition_summary must be populated");
        assert!(
            summary.dropped_coi >= 1,
            "expected ≥ 1 dropped signal (s_drop); got {summary:?}"
        );
        assert!(
            summary.kept >= 2,
            "expected ≥ 2 kept signals; got {summary:?}"
        );
        assert_eq!(summary.datapath_uf, 0, "datapath UF disabled in A.3");
        // Width tracking should be present for BTOR2.
        let before = summary
            .state_bits_before
            .expect("BTOR2 always tracks widths");
        let after = summary
            .state_bits_after
            .expect("BTOR2 always tracks widths");
        assert!(
            after <= before,
            "state_bits_after ({after}) must not exceed state_bits_before ({before})"
        );
        assert!(
            after < before,
            "expected reduction from dropping s_drop; got before={before} after={after}"
        );
    }

    #[test]
    fn constraint_filters_transitions() {
        // input drives state; constraint = input must be 0; bad line
        // pinned to state `q` so auto-COI (Phase A.3) keeps `q` in the
        // partition. Without that pin, COI would correctly drop `q`
        // because no property atom referenced it — collapsing the
        // test's state space below the count required to assert that
        // *constraint* filtering takes effect.
        //
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
9 bad 3
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        assert_eq!(out.source_info.state_count, 2);
        // Transitions encoded in CTXDSL — we just check the run completed.
        assert!(out.ctxdsl.contains("automaton"));
    }

    // ---- Path 3 / Option A (§Phase 8 §8.2 residual) — anyconst-init enumeration ----

    #[test]
    fn path3_baseline_single_init_when_all_cells_have_init_lines() {
        // 1-bit state q with explicit init 0; q' = q (latch). Single
        // init state — Path 3 fallback path returns the deterministic
        // index. Validates the legacy behaviour stays untouched when
        // there are no anyconst cells.
        let src = r#"
1 sort bitvec 1
2 zero 1
3 state 1 q
4 init 1 3 2
5 next 1 3 3
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        // Two abstract states (q=0, q=1); only q=0 is initial.
        assert_eq!(out.source_info.state_count, 2);
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(initial_count, 1, "exactly one initial-state declaration");
    }

    #[test]
    fn path3_uninit_cell_without_anyconst_emits_soundness_warning() {
        // 2-bit state q with NO init line and NO sidecar anyconst
        // declaration — the cell defaults to zero. Sound only under
        // `setundef -zero`; unsound under any nondeterministic policy.
        // The translator must emit a soundness warning naming the
        // uncovered cell.
        let src = r#"
1 sort bitvec 2
2 zero 1
3 ones 1
4 state 1 q
5 next 1 4 4
6 eq 1 4 3
7 bad 6
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        let soundness_warning = out
            .warnings
            .iter()
            .find(|w| w.message.contains("default to zero"))
            .expect("expected SOUNDNESS warning for uninit cell");
        assert!(
            soundness_warning.message.contains("q"),
            "warning should name the uncovered cell: {}",
            soundness_warning.message
        );
        assert!(
            soundness_warning.message.contains("init_policy: anyconst"),
            "warning should point at the remediation"
        );
    }

    #[test]
    fn path3_uninit_cell_with_anyconst_emits_no_soundness_warning() {
        // Same fixture as above, but `q` is declared anyconst in the
        // sidecar — the soundness concern is resolved by Path 3
        // enumeration, no warning emitted.
        let src = r#"
1 sort bitvec 2
2 zero 1
3 ones 1
4 state 1 q
5 next 1 4 4
6 eq 1 4 3
7 bad 6
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {"name": "q", "abstraction": "bit_blast", "init_policy": "anyconst"}
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        assert!(
            !out.warnings
                .iter()
                .any(|w| w.message.contains("default to zero")),
            "no soundness warning expected when cell is covered by anyconst sidecar"
        );
    }

    #[test]
    fn path3_init_line_present_emits_no_soundness_warning() {
        // Baseline: cell has an explicit Init line — no warning needed
        // because the init value is determined, not defaulted.
        let src = r#"
1 sort bitvec 1
2 zero 1
3 state 1 q
4 init 1 3 2
5 next 1 3 3
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        assert!(
            !out.warnings
                .iter()
                .any(|w| w.message.contains("default to zero")),
            "no soundness warning when cell has explicit init"
        );
    }

    // ---- R-S2a (§Phase 9 §9.1) — BTOR2 init-line seeding ----

    #[test]
    fn r_s2a_seeds_init_value_when_sidecar_enum_lacks_it() {
        // 2-bit state q with init=3. Sidecar declares q as enum
        // with only variant {Q_0: 0}. R-S2a should auto-add
        // {q_3: 3} so the formula can reference q_3.
        let src = r#"
1 sort bitvec 2
2 constd 1 3
3 state 1 q
4 init 1 3 2
5 next 1 3 3
6 eq 1 3 2
7 bad 6
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {
                    "name": "q",
                    "abstraction": "enum",
                    "variants": ["Q_0"],
                    "value_map": [{"name": "Q_0", "value": 0}]
                }
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // After R-S2a seeding, the abstraction set has {0, 3} → 2 states.
        assert_eq!(out.source_info.state_count, 2);
    }

    #[test]
    fn r_s2a_no_op_when_init_value_already_in_value_map() {
        // Same fixture as above but value_map already includes 3.
        // R-S2a should skip (additive, dedupes).
        let src = r#"
1 sort bitvec 2
2 constd 1 3
3 state 1 q
4 init 1 3 2
5 next 1 3 3
6 eq 1 3 2
7 bad 6
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {
                    "name": "q",
                    "abstraction": "enum",
                    "variants": ["Q_0", "Q_3"],
                    "value_map": [
                        {"name": "Q_0", "value": 0},
                        {"name": "Q_3", "value": 3}
                    ]
                }
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // value_map has 2 entries (0, 3) → 2 states; R-S2a no-op.
        assert_eq!(out.source_info.state_count, 2);
    }

    #[test]
    fn r_s2a_skips_signals_without_sidecar_entry() {
        // q has init=1 but no sidecar entry. R-S2a should skip
        // (cells without sidecar use full bit-blast — init already
        // in the admissible set).
        let src = r#"
1 sort bitvec 2
2 one 1
3 state 1 q
4 init 1 3 2
5 next 1 3 3
6 eq 1 3 2
7 bad 6
"#;
        // No sidecar → full bit-blast over 2 bits = 4 states.
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        assert_eq!(out.source_info.state_count, 4);
    }

    #[test]
    fn r_s2a_skips_non_enum_abstractions() {
        // q has init=1 and sidecar bounded_counter abstraction.
        // R-S2a only widens EnumValues; bounded_counter handles its
        // value set via the (lo, hi) bound and doesn't take per-value
        // variant names.
        let src = r#"
1 sort bitvec 4
2 one 1
3 state 1 q
4 init 1 3 2
5 next 1 3 3
6 eq 1 3 2
7 bad 6
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {"name": "q", "abstraction": "bounded_counter", "bound": 2}
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // bounded_counter [0, 2] = 3 states; R-S2a no-op.
        assert_eq!(out.source_info.state_count, 3);
    }

    // ---- R-Y3 (§Phase 8) — BTOR2 init smart defaults for unsidecared cells ----

    // ---- §Phase 10 §10.2 stage 1 — BTOR2 $mem lifter extension ----

    /// BTOR2 fixture with one array state cell (5-bit address × 8-bit data).
    /// Has Write + Next + Read ops on the array; mirrors what Yosys
    /// emits for a small `logic [7:0] m [0:31]` SV declaration.
    const PHASE10_FIXTURE_WITH_MEMORY: &str = r#"
1 sort bitvec 1
2 sort bitvec 5
3 sort bitvec 8
4 sort array 2 3
5 state 4 rf_reg
6 input 1 we
7 input 2 addr
8 input 3 wdata
9 ite 4 6 5 5
10 next 4 5 9
11 read 3 5 7
12 zero 3
13 eq 1 11 12
14 bad 13
"#;

    #[test]
    fn phase10_stage1_no_sidecar_emits_actionable_template_error() {
        let err = translate(PHASE10_FIXTURE_WITH_MEMORY, &AdapterOptions::default())
            .expect_err("should error on undeclared memory");
        assert!(
            err.message.contains("§Phase 10"),
            "error should cite §Phase 10: {}",
            err.message
        );
        assert!(
            err.message.contains("rf_reg"),
            "error should name the undeclared memory cell: {}",
            err.message
        );
        assert!(
            err.message.contains("\"address_width\":5"),
            "error template should include the BTOR2-derived address_width: {}",
            err.message
        );
        assert!(
            err.message.contains("\"data_width\":8"),
            "error template should include the BTOR2-derived data_width: {}",
            err.message
        );
        assert!(
            err.message.contains("\"abstraction\":\"havoc\""),
            "error template should suggest havoc as the stage-1 default: {}",
            err.message
        );
    }

    #[test]
    fn phase10_stage1_sidecar_consistent_passes_to_op_check() {
        // With a consistent sidecar declaration, validation passes —
        // but the lift then errors at the Read/Write op check (stage 3+
        // territory). The error message includes the §Phase 10 hint.
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "memories": [
                {
                    "name": "rf_reg",
                    "address_width": 5,
                    "data_width": 8,
                    "abstraction": "havoc"
                }
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let err = translate(PHASE10_FIXTURE_WITH_MEMORY, &opts)
            .expect_err("stage 1 still errors at Read/Write op check");
        assert!(
            err.message.contains("§Phase 10"),
            "op-check error should include the §Phase 10 hint: {}",
            err.message
        );
        assert!(
            err.message.contains("stage 3") || err.message.contains("stage 4"),
            "op-check hint should point at stages 3/4: {}",
            err.message
        );
    }

    #[test]
    fn phase10_stage1_sidecar_mismatched_dimensions_errors() {
        // Sidecar declares wrong dimensions; validation should flag
        // the mismatch and emit a corrective template.
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "memories": [
                {
                    "name": "rf_reg",
                    "address_width": 6,  // wrong (BTOR2 says 5)
                    "data_width": 16,    // wrong (BTOR2 says 8)
                    "abstraction": "havoc"
                }
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let err = translate(PHASE10_FIXTURE_WITH_MEMORY, &opts)
            .expect_err("should error on dimension mismatch");
        assert!(
            err.message.contains("mismatch"),
            "error should flag mismatch: {}",
            err.message
        );
        assert!(
            err.message
                .contains("BTOR2 has address_width=5, data_width=8"),
            "error should show the BTOR2-detected dimensions: {}",
            err.message
        );
        assert!(
            err.message
                .contains("sidecar declared address_width=6, data_width=16"),
            "error should show the sidecar-declared dimensions: {}",
            err.message
        );
    }

    #[test]
    fn phase10_stage1_no_memory_cells_no_op() {
        // Fixture without any array state cells: validation runs but
        // emits no error; existing behaviour preserved.
        let src = r#"
1 sort bitvec 1
2 zero 1
3 state 1 q
4 init 1 3 2
5 next 1 3 3
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        assert!(
            out.warnings
                .iter()
                .all(|w| !w.message.contains("§Phase 10"))
        );
    }

    // ---- R-Y6 (§Phase 8) — reset-sequence-aware init ----

    #[test]
    fn r_y6_no_sidecar_no_op() {
        // Counter that increments every cycle; init=0. Without R-Y6
        // sequence, init state is 0.
        let src = r#"
1 sort bitvec 2
2 zero 1
3 state 1 q
4 init 1 3 2
5 one 1
6 add 1 3 5
7 next 1 3 6
8 input 1 rst_n
9 ones 1
10 eq 1 3 9
11 bad 10
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        // Single init state (q=0), exactly 1 "initial" marker.
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(initial_count, 1);
    }

    #[test]
    fn r_y6_with_sidecar_advances_init_state_by_k_cycles() {
        // Counter q increments every cycle UNLESS rst_n=0 (active-low
        // reset) which holds q at 0. R-Y6 with hold_cycles=2,
        // asserted_value=0 → q held at 0 for 2 cycles, then init is
        // computed from the post-reset state (still 0; counter never
        // increments while rst_n=0).
        let src = r#"
1 sort bitvec 2
2 zero 1
3 state 1 q
4 init 1 3 2
5 one 1
6 add 1 3 5
7 input 1 rst_n
8 ite 1 7 6 2
9 next 1 3 8
10 ones 1
11 eq 1 3 10
12 bad 11
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "reset_sequence": {
                "reset_input": "rst_n",
                "asserted_value": 0,
                "hold_cycles": 2
            }
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // Single init state — q still 0 after 2 reset cycles.
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(initial_count, 1);
    }

    #[test]
    fn r_y6_runs_design_logic_for_k_cycles_without_reset() {
        // Counter q increments every cycle unconditionally (no reset
        // logic in the next-state function). R-Y6 still pins the
        // declared "reset" input but the design ignores it. After
        // hold_cycles=3 cycles, q should be at value 3.
        let src = r#"
1 sort bitvec 3
2 zero 1
3 state 1 q
4 init 1 3 2
5 one 1
6 add 1 3 5
7 next 1 3 6
8 input 1 rst_n
9 ones 1
10 eq 1 3 9
11 bad 10
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "reset_sequence": {
                "reset_input": "rst_n",
                "asserted_value": 0,
                "hold_cycles": 3
            }
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // q advances from 0 → 1 → 2 → 3 over the 3 reset cycles.
        // The bad signal `eq 1 3 9` (=q==7) is not satisfied at q=3,
        // so init state q=3 satisfies. The CTXDSL has a `q = 3`
        // valuation block on the initial state.
        assert!(
            out.ctxdsl.contains("q = 3"),
            "expected init valuation q = 3 after 3-cycle reset hold; got:\n{}",
            out.ctxdsl
        );
    }

    #[test]
    fn r_y6_unknown_reset_input_is_silent_no_op() {
        // Sidecar names a reset signal that doesn't exist in the
        // BTOR2. R-Y6 silently skips (additive); init state computed
        // normally.
        let src = r#"
1 sort bitvec 2
2 zero 1
3 state 1 q
4 init 1 3 2
5 one 1
6 add 1 3 5
7 next 1 3 6
8 ones 1
9 eq 1 3 8
10 bad 9
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "reset_sequence": {
                "reset_input": "nonexistent_reset",
                "asserted_value": 0,
                "hold_cycles": 3
            }
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // Without applicable reset signal, init state is cycle 0 (q=0).
        assert!(out.ctxdsl.contains("q = 0"));
    }

    // ---- R-Y4 (§Phase 8) — bounded-havoc init value sets ----

    #[test]
    fn r_y4_bounded_init_restricts_path3_enumeration() {
        // 2-bit state q with anyconst init policy + bounded_init: [0, 2].
        // Path 3 should enumerate exactly 2 init states (q=0, q=2)
        // instead of all 4 admissible values.
        let src = r#"
1 sort bitvec 2
2 zero 1
3 ones 1
4 state 1 q
5 next 1 4 4
6 eq 1 4 3
7 bad 6
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {
                    "name": "q",
                    "abstraction": "bit_blast",
                    "init_policy": "anyconst",
                    "bounded_init": [0, 2]
                }
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // 4 abstract states; 2 are initial (the bounded set).
        assert_eq!(out.source_info.state_count, 4);
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(
            initial_count, 2,
            "bounded_init restricts enumeration to {{0, 2}} = 2 init states"
        );
    }

    #[test]
    fn r_y4_empty_bounded_init_falls_back_to_full_anyconst() {
        // bounded_init: [] should be treated as "no bounding" — Path 3
        // enumerates the full admissible set per the anyconst policy.
        let src = r#"
1 sort bitvec 2
2 zero 1
3 ones 1
4 state 1 q
5 next 1 4 4
6 eq 1 4 3
7 bad 6
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {
                    "name": "q",
                    "abstraction": "bit_blast",
                    "init_policy": "anyconst",
                    "bounded_init": []
                }
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(
            initial_count, 4,
            "empty bounded_init → full anyconst enumeration"
        );
    }

    #[test]
    fn r_y4_no_op_when_init_policy_not_anyconst() {
        // bounded_init declared but init_policy not anyconst — Path 3
        // does not fire (no nondet_init_nids); bounded_init is
        // collected but ignored. Legacy single-init runs.
        let src = r#"
1 sort bitvec 2
2 zero 1
3 ones 1
4 state 1 q
5 init 1 4 2
6 next 1 4 4
7 eq 1 4 3
8 bad 7
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {
                    "name": "q",
                    "abstraction": "bit_blast",
                    "bounded_init": [1, 2]
                }
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(
            initial_count, 1,
            "no anyconst → no Path 3 enumeration → legacy single-init"
        );
    }

    #[test]
    fn r_y3_default_off_preserves_full_bit_blast() {
        // 2-bit state q with init=1; NO sidecar entry. Default
        // R-Y3 off → full bit-blast = 4 states.
        let src = r#"
1 sort bitvec 2
2 one 1
3 state 1 q
4 init 1 3 2
5 next 1 3 3
6 eq 1 3 2
7 bad 6
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        assert_eq!(out.source_info.state_count, 4);
    }

    #[test]
    fn r_y3_enabled_pins_unsidecared_cell_to_init_value() {
        // Same fixture; R-Y3 ON → q pinned to init=1, single state.
        let src = r#"
1 sort bitvec 2
2 one 1
3 state 1 q
4 init 1 3 2
5 next 1 3 3
6 eq 1 3 2
7 bad 6
"#;
        let opts = AdapterOptions {
            smart_init_defaults: true,
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // q pinned to init=1 → 1 state.
        assert_eq!(out.source_info.state_count, 1);
        // Warning fires naming the affected cell.
        let warning = out
            .warnings
            .iter()
            .find(|w| w.message.contains("R-Y3"))
            .expect("R-Y3 warning expected");
        assert!(
            warning.message.contains("q"),
            "warning should name the pinned cell: {}",
            warning.message
        );
        assert!(
            warning.message.contains("unsound for safety"),
            "warning should surface the soundness tradeoff"
        );
    }

    #[test]
    fn r_y3_skips_sidecared_cells() {
        // q has init=1 AND sidecar entry. R-Y3 should NOT override
        // the sidecar declaration (only fills in for cells WITHOUT one).
        let src = r#"
1 sort bitvec 4
2 one 1
3 state 1 q
4 init 1 3 2
5 next 1 3 3
6 eq 1 3 2
7 bad 6
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {"name": "q", "abstraction": "bounded_counter", "bound": 2}
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            smart_init_defaults: true,
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // Sidecar wins: bounded_counter [0, 2] = 3 states; R-Y3 no-op.
        assert_eq!(out.source_info.state_count, 3);
    }

    #[test]
    fn r_y3_skips_cells_without_init_line() {
        // q has NO init line AND no sidecar. R-Y3 skips (nothing to
        // pin to); falls back to full bit-blast.
        let src = r#"
1 sort bitvec 2
2 state 1 q
3 next 1 2 2
4 zero 1
5 eq 1 2 4
6 bad 5
"#;
        let opts = AdapterOptions {
            smart_init_defaults: true,
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        // Full bit-blast (no init to pin to) = 4 states.
        assert_eq!(out.source_info.state_count, 4);
    }

    #[test]
    fn path3_no_anyconst_sidecar_means_legacy_single_init_even_for_uninit_cells() {
        // 2-bit state q with NO init line, NO sidecar. The `bad` line
        // pins q into the property cone so auto-COI doesn't drop it.
        // Path 3 is surgical — without sidecar anyconst, q defaults
        // to zero (legacy single-init).
        let src = r#"
1 sort bitvec 2
2 zero 1
3 ones 1
4 state 1 q
5 next 1 4 4
6 eq 1 4 3
7 bad 6
"#;
        let out = translate(src, &AdapterOptions::default()).expect("translate");
        assert_eq!(out.source_info.state_count, 4);
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(
            initial_count, 1,
            "no sidecar anyconst declaration ⇒ legacy single-init"
        );
    }

    #[test]
    fn path3_anyconst_sidecar_enumerates_all_admissible_init_values() {
        // 2-bit state q with NO init line + bad q==3 (so partition
        // keeps q). Sidecar declares anyconst on q → all 4 admissible
        // values become initial states.
        let src = r#"
1 sort bitvec 2
2 zero 1
3 ones 1
4 state 1 q
5 next 1 4 4
6 eq 1 4 3
7 bad 6
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {"name": "q", "abstraction": "bit_blast", "init_policy": "anyconst"}
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        assert_eq!(out.source_info.state_count, 4);
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(
            initial_count, 4,
            "sidecar-declared anyconst ⇒ every admissible init value is initial"
        );
    }

    #[test]
    fn path3_mixed_some_deterministic_some_anyconst_init() {
        // Two state cells: q1 (init 0), q2 (no init). Sidecar declares
        // anyconst on q2 only. The `bad` line keeps both in the cone.
        // Expected initial states: {(q1=0, q2=0), (q1=0, q2=1)} = 2.
        let src = r#"
1 sort bitvec 1
2 zero 1
3 ones 1
4 state 1 q1
5 state 1 q2
6 init 1 4 2
7 next 1 4 4
8 next 1 5 5
9 eq 1 4 3
10 bad 9
"#;
        let sidecar = serde_json::json!({
            "$schema": "mununu_sv_annotation_v1",
            "module": "test",
            "signals": [
                {"name": "q1", "abstraction": "bit_blast"},
                {"name": "q2", "abstraction": "bit_blast", "init_policy": "anyconst"}
            ]
        });
        let opts = AdapterOptions {
            sidecar_json: Some(sidecar.to_string()),
            ..Default::default()
        };
        let out = translate(src, &opts).expect("translate");
        assert_eq!(out.source_info.state_count, 4);
        let initial_count = out.ctxdsl.lines().filter(|l| l.contains("initial")).count();
        assert_eq!(
            initial_count, 2,
            "cartesian product collapses on the pinned cell"
        );
    }
}
