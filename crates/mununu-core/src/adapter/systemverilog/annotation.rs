//! SystemVerilog annotation sidecar (`.mununu.json`).
//!
//! Declares which signals to preserve, how to abstract them, which
//! properties to verify, and (optionally) SMT-discovered significant
//! values. The sidecar file lives next to the `.sv` source and is the
//! single source of truth for the Kripke verification pipeline.
//!
//! # Formats
//!
//! Two sidecar formats are supported:
//!
//! - **Single-module** (`mununu_sv_annotation_v1`): one `"module"` field,
//!   one `.sv` source, one automaton output.
//! - **Multi-module** (`mununu_sv_multi_v1`): a `"modules"` array with
//!   connections, composition directives, and cross-module properties.
//!   Builds one Kripke automaton per module and composes them.
//!
//! # Pipeline
//!
//! ```text
//! Parse SV → Load .mununu.json → SMT discovery → Build abstraction → Kripke → Verify
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Top-level annotation loaded from `<module>.mununu.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SvAnnotation {
    /// Schema version identifier.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Module name (must match the SV module declaration).
    pub module: String,

    /// Path to the `.sv` source file (relative to the sidecar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,

    /// Register/internal signal annotations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<SignalAnnotation>,

    /// Input port annotations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputAnnotation>,

    /// Signals explicitly marked as controllable (output ports or overrides).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllable: Vec<String>,

    /// Properties to verify.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<PropertyAnnotation>,

    /// SMT-discovered significant values per signal.
    /// Populated by `mununu sv discover`; user may edit names.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub discovered_values: HashMap<String, DiscoveredValues>,

    /// Module parameter overrides (e.g., `{"DEPTH": 4}`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: HashMap<String, i64>,
}

/// Annotation for a register or internal signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalAnnotation {
    /// Signal name (must match an SV declaration).
    pub name: String,

    /// Whether to include this signal in the state space.
    /// Default: `true` if listed, but can be explicitly set to `false`.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub preserve: bool,

    /// Abstraction strategy.
    #[serde(default)]
    pub abstraction: SignalAbstraction,

    /// Upper bound for `bounded_counter`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<i64>,

    /// Explicit enum variants (for `abstraction: "enum"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<String>>,

    /// Value map entries (for `abstraction: "enum"` with numeric mapping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_map: Option<Vec<ValueMapEntry>>,

    /// Whether this signal is combinational (computed from `assign` / `always_comb`).
    /// Combinational signals are included in the state space but their value is
    /// computed from the combinational logic each cycle, not from sequential assignments.
    #[serde(default, skip_serializing_if = "is_false")]
    pub combinational: bool,

    /// R-Y2 (§Phase 8 §8.1) — per-signal init policy override. When
    /// not `Inherit`, this signal's undef bits are treated per the
    /// chosen policy (zero / anyconst / anyseq) regardless of the
    /// global `YosysOptions::setundef_*` flags. The Yosys script-
    /// builder emits `setattr -mod -set <attr> <val> w:<name>`
    /// between `read_verilog` and `proc` to apply the override.
    /// **Default `Inherit`** — strict additivity; legacy sidecars
    /// unchanged.
    #[serde(default, skip_serializing_if = "is_inherit_init_policy")]
    pub init_policy: InitPolicy,

    /// R-S5 (§Phase 9 §9.1) — SV typedef name (e.g. `boot_fsm_state_e`)
    /// this signal is declared with. When set AND the signal's
    /// `abstraction` is `Discover` or `Enum` with empty `variants` /
    /// `value_map`, the loader walks SV source for the typedef
    /// declaration via [`super::typedef_extract::extract_typedef_enums`]
    /// and auto-fills the variant list + value map. Includes the
    /// `UNMATCHED_<n>` synthetic variants for encodings the typedef's
    /// bit-width admits but doesn't enumerate, which is the load-bearing
    /// CWE-1245 detection mechanism on the Caliptra fixture (the bug
    /// fires precisely on the unmatched encodings).
    ///
    /// Default `None` — opt-in per-signal; existing sidecars unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,

    /// Human-readable note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// R-Y2 — serde skip-helper: skip serialising `init_policy` when it
/// is the default `Inherit`. Keeps existing sidecars round-trip stable.
fn is_inherit_init_policy(p: &InitPolicy) -> bool {
    matches!(p, InitPolicy::Inherit)
}

impl SvAnnotation {
    /// R-Y2 (§Phase 8 §8.1) — Collect per-signal init-policy overrides
    /// from the sidecar's `signals` + `inputs` declarations. Returns
    /// `(signal_name, InitPolicy)` pairs for every signal whose policy
    /// is not `Inherit`. The Yosys script-builder consumes this to
    /// emit `setattr -mod -set <attr> <val> w:<signal>` commands.
    ///
    /// Order is deterministic (signals first, in declaration order,
    /// then inputs in declaration order) so the emitted script is
    /// stable across runs.
    pub fn init_policy_overrides(&self) -> Vec<(String, InitPolicy)> {
        let mut out: Vec<(String, InitPolicy)> = Vec::new();
        for sig in &self.signals {
            if !matches!(sig.init_policy, InitPolicy::Inherit) {
                out.push((sig.name.clone(), sig.init_policy));
            }
        }
        for inp in &self.inputs {
            if !matches!(inp.init_policy, InitPolicy::Inherit) {
                out.push((inp.name.clone(), inp.init_policy));
            }
        }
        out
    }

    /// R-S5 (§Phase 9 §9.1) — auto-fill `variants` + `value_map` on
    /// signals that declare a `type_name` and whose abstraction is
    /// `Discover` or `Enum` with empty variant info. The `typedefs`
    /// map is the output of
    /// [`super::typedef_extract::extract_typedef_enums`] over every
    /// SV source file the loader has access to (primary + sidecars).
    ///
    /// **Closes the §Phase 8 §8.2 abstraction-clipping bottleneck
    /// without manual sidecar editing.** When the typedef admits more
    /// encodings than its named variants (e.g. `boot_fsm_state_e`:
    /// width 3, 5 named variants, 3 unmatched encodings `{5,6,7}`),
    /// the auto-widening emits the named variants AND the synthetic
    /// `UNMATCHED_<n>` variants so the abstraction layer keeps
    /// transitions to the unmatched encodings in the abstract relation.
    /// On the Caliptra fixture this is exactly the manual Path 1
    /// widening reproduced automatically.
    ///
    /// Returns the list of `(signal_name, type_name, variant_count)`
    /// triples that were widened, for caller logging. Signals without
    /// a `type_name`, with `Ignored` / `Boolean` abstraction, or whose
    /// `type_name` doesn't appear in `typedefs` are silently skipped
    /// (additive — no behavior change for legacy sidecars).
    pub fn apply_type_driven_widening(
        &mut self,
        typedefs: &std::collections::HashMap<String, super::typedef_extract::TypedefEnum>,
    ) -> Vec<(String, String, usize)> {
        let mut applied = Vec::new();
        for sig in &mut self.signals {
            let Some(type_name) = sig.type_name.as_deref() else {
                continue;
            };
            let Some(td) = typedefs.get(type_name) else {
                continue;
            };
            // Only widen when the abstraction strategy can take a
            // variant list AND it isn't already supplied. Discover and
            // Enum-with-empty-variants are the targets; explicit
            // user-supplied variants are NEVER overwritten.
            let target_strategy = matches!(
                sig.abstraction,
                SignalAbstraction::Discover | SignalAbstraction::Enum
            );
            let variants_empty = sig.variants.as_ref().map(|v| v.is_empty()).unwrap_or(true);
            if !target_strategy || !variants_empty {
                continue;
            }
            // Apply the type-driven widening. Variants include the
            // synthetic UNMATCHED_<n> entries; value_map binds each
            // variant name to its numeric encoding.
            let all = td.all_encodings();
            sig.abstraction = SignalAbstraction::Enum;
            sig.variants = Some(all.iter().map(|(n, _)| n.clone()).collect());
            sig.value_map = Some(
                all.iter()
                    .map(|(n, v)| ValueMapEntry {
                        name: n.clone(),
                        value: *v as i64,
                    })
                    .collect(),
            );
            applied.push((sig.name.clone(), type_name.to_string(), all.len()));
        }
        applied
    }

    /// R-S7 (§Phase 9 §9.1) — property-syntactic seeding. Walks every
    /// property formula in the sidecar, collects predicate names of
    /// the shape `<signal>_<integer-suffix>` where `<signal>` is a
    /// declared signal in the sidecar, and adds the integer values as
    /// abstraction discriminators for that signal (synthetic variant
    /// name `<signal>_<n>` mirroring R-S5's UNMATCHED_<n> convention).
    ///
    /// **Bridges the gap between handwritten formulas and the
    /// abstraction set.** When the user writes a property referencing
    /// `count_3` or `mode_7` but the signal has no typedef for R-S5
    /// to widen against, R-S7 picks up the values from the formula
    /// text and ensures the abstraction layer keeps transitions to
    /// those specific values.
    ///
    /// Strictly additive — only inserts values not already in the
    /// signal's value_map. Signals with explicit `bit_blast` /
    /// `boolean` / `ignored` abstractions are skipped (those don't
    /// take per-value discriminators). Signals without any seeded
    /// values from the property text are untouched.
    ///
    /// Returns `(signal_name, [seeded_values])` for each signal
    /// widened, for caller logging.
    pub fn apply_property_syntactic_seeding(&mut self) -> Vec<(String, Vec<i64>)> {
        let mut harvest: std::collections::BTreeMap<String, std::collections::BTreeSet<i64>> =
            std::collections::BTreeMap::new();
        let signal_names: std::collections::HashSet<String> =
            self.signals.iter().map(|s| s.name.clone()).collect();

        for prop in &self.properties {
            let Some(formula_text) = prop.formula.as_deref() else {
                continue;
            };
            let Ok(formula) = crate::mu_calculus::parser::parse(formula_text) else {
                continue;
            };
            for node in formula.nodes() {
                let preds_in_node: Vec<String> = match node {
                    crate::mu_calculus::Node::Predicate(name) => vec![name.clone()],
                    crate::mu_calculus::Node::Modal { guard, .. } => guard
                        .current
                        .required
                        .iter()
                        .chain(guard.current.forbidden.iter())
                        .chain(guard.next.required.iter())
                        .chain(guard.next.forbidden.iter())
                        .cloned()
                        .collect(),
                    _ => Vec::new(),
                };
                for pred in preds_in_node {
                    let Some((sig, value)) = split_signal_integer_suffix(&pred, &signal_names)
                    else {
                        continue;
                    };
                    harvest.entry(sig).or_default().insert(value);
                }
            }
        }

        let mut applied = Vec::new();
        for sig in &mut self.signals {
            let Some(seeds) = harvest.get(&sig.name) else {
                continue;
            };
            let widenable = matches!(
                sig.abstraction,
                SignalAbstraction::Discover | SignalAbstraction::Enum
            );
            if !widenable {
                continue;
            }
            let mut value_map = sig.value_map.clone().unwrap_or_default();
            let existing_values: std::collections::HashSet<i64> =
                value_map.iter().map(|e| e.value).collect();
            let mut added_now = Vec::new();
            for &v in seeds {
                if existing_values.contains(&v) {
                    continue;
                }
                let variant = format!("{}_{}", sig.name, v);
                value_map.push(ValueMapEntry {
                    name: variant,
                    value: v,
                });
                added_now.push(v);
            }
            if added_now.is_empty() {
                continue;
            }
            // Refresh the variants list to mirror value_map order.
            let mut variants = sig.variants.clone().unwrap_or_default();
            for &v in &added_now {
                variants.push(format!("{}_{}", sig.name, v));
            }
            sig.abstraction = SignalAbstraction::Enum;
            sig.variants = Some(variants);
            sig.value_map = Some(value_map);
            applied.push((sig.name.clone(), added_now));
        }
        applied
    }
}

/// R-S7 helper — split a predicate-name string of the shape
/// `<signal>_<integer-suffix>` into `(signal, value)`. Uses the
/// declared-signals set to disambiguate (e.g. `boot_fsm_ns_5` against
/// declared `boot_fsm_ns` → splits at the last `_` boundary that
/// matches a declared signal name).
///
/// Returns `None` when the predicate name does not parse this way —
/// e.g. typedef-derived variants like `boot_fsm_ns_BOOT_IDLE` (non-
/// numeric suffix), or names where no prefix matches a declared
/// signal.
fn split_signal_integer_suffix(
    predicate: &str,
    signal_names: &std::collections::HashSet<String>,
) -> Option<(String, i64)> {
    // Try every underscore split position; prefer the longest prefix
    // that matches a declared signal (handles signal names containing
    // underscores like `boot_fsm_ns`).
    let bytes = predicate.as_bytes();
    let mut best: Option<(String, i64)> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'_' {
            continue;
        }
        let prefix = &predicate[..i];
        let suffix = &predicate[i + 1..];
        if !signal_names.contains(prefix) {
            continue;
        }
        let Ok(value) = suffix.parse::<i64>() else {
            continue;
        };
        // Prefer the longest matching prefix.
        if best
            .as_ref()
            .map(|(p, _)| prefix.len() > p.len())
            .unwrap_or(true)
        {
            best = Some((prefix.to_string(), value));
        }
    }
    best
}

/// Annotation for an input port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputAnnotation {
    /// Port name.
    pub name: String,

    /// Whether to include this input as a label dimension.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub preserve: bool,

    /// Abstraction strategy (default: boolean for 1-bit).
    #[serde(default)]
    pub abstraction: SignalAbstraction,

    /// Upper bound for `bounded_counter`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<i64>,

    /// Explicit enum variants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<String>>,

    /// Value map entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_map: Option<Vec<ValueMapEntry>>,

    /// Override the label prefix used in transitions. When set, transitions
    /// for this input use `label_name` instead of `name` in the generated
    /// labels. Used by multi-module connections to create shared labels
    /// between driving and receiving modules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_name: Option<String>,

    /// R-Y2 (§Phase 8 §8.1) — per-input init policy override. Same
    /// semantics as `SignalAnnotation::init_policy`; used when an
    /// input port is the under-constrained constant the property
    /// depends on (e.g. fuse / strap bits in the Caliptra context).
    /// **Default `Inherit`** — strict additivity.
    #[serde(default, skip_serializing_if = "is_inherit_init_policy")]
    pub init_policy: InitPolicy,
}

/// Abstraction strategy for a signal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalAbstraction {
    /// 1-bit: true/false.
    Boolean,
    /// N-bit kept as individual bits (≤4 bits only).
    BitBlast,
    /// 0..bound counter with saturation.
    BoundedCounter,
    /// Named enum variants (optionally with value mapping).
    Enum,
    /// Let SMT discover significant values from guard analysis.
    #[default]
    Discover,
    /// Exclude from state space.
    Ignored,
}

/// A named value in a value map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueMapEntry {
    pub name: String,
    pub value: i64,
}

/// R-Y2 (§Phase 8 §8.1) — Per-signal init policy. Selects which
/// Yosys `setundef`-style treatment applies to *this signal* in
/// isolation, overriding the global `setundef_zero` / `setundef_anyseq`
/// / `setundef_anyconst` policy from `YosysOptions`.
///
/// The Yosys mechanism is the `(* anyconst *)` net attribute applied
/// per signal via the `setattr -mod -set anyconst 1 w:<signal>`
/// script command between `read_verilog` and `proc` passes. This
/// gives surgical control — `anyconst` only on the bug-relevant
/// register (e.g. `boot_fsm_ns` in the Caliptra fixture) while
/// other undefs stay `zero`.
///
/// **Default**: `Inherit` — apply the global policy from
/// `YosysOptions`. Explicit per-signal opt-in is the load-bearing
/// case for the Caliptra anchor per §Phase 8 §8.2.
///
/// **Strict additivity**: legacy sidecars without this field
/// continue to load (default `Inherit`); existing fixtures'
/// verdicts unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitPolicy {
    /// Defer to the global `YosysOptions::setundef_*` flags. This
    /// is the legacy behaviour and the load-preserving default for
    /// sidecars that pre-date R-Y2.
    #[default]
    Inherit,
    /// Override: pin this signal's undefined bits to 0. Cheapest;
    /// matches the global `setundef -zero` semantics for this
    /// signal alone.
    Zero,
    /// Override: this signal's undefined bits become one
    /// nondeterministic constant input each. Solver picks any
    /// concrete value at init; value stays fixed for the run. The
    /// Caliptra anchor uses this on `boot_fsm_ns` (3 bits → 8 init
    /// choices) while other undefs stay `zero`.
    Anyconst,
    /// Override: this signal's undefined bits become free symbolic
    /// choices each cycle. Per-signal `$anyseq` cells; small
    /// per-signal cost.
    Anyseq,
}

impl InitPolicy {
    /// Returns the Yosys per-signal attribute name + value pair,
    /// or `None` for `Inherit` (the global policy applies).
    /// Used by the Yosys script-builder to emit `setattr` commands.
    pub fn yosys_attribute(self) -> Option<(&'static str, u32)> {
        match self {
            InitPolicy::Inherit => None,
            InitPolicy::Zero => Some(("init", 0)), // emitted as `setattr -set init 0`
            InitPolicy::Anyconst => Some(("anyconst", 1)),
            InitPolicy::Anyseq => Some(("anyseq", 1)),
        }
    }
}

/// A property to verify.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyAnnotation {
    /// Property identifier.
    pub id: String,

    /// Mu-calculus formula body.
    /// Optional when `template_ref` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,

    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Property role: "guarantee" (default), "assumption", or "standalone".
    #[serde(default = "default_guarantee", skip_serializing_if = "is_guarantee")]
    pub role: String,

    /// Reference to a property template from the template catalog.
    /// When present, the template is instantiated to produce a mu-calculus formula.
    /// If both `formula` and `template_ref` are present, `formula` takes precedence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_ref: Option<crate::adapter::templates::TemplateRef>,
}

/// SMT-discovered values for a signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredValues {
    /// Discovered significant values with provenance.
    pub values: Vec<DiscoveredValue>,

    /// Name for the catch-all variant (values not in the map).
    #[serde(default = "default_other")]
    pub catch_all: String,
}

/// A single discovered value with provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredValue {
    /// The concrete numeric value.
    pub value: i64,

    /// User-editable variant name.
    pub name: String,

    /// How this value was discovered (e.g., "case label at line 38",
    /// "SMT: guard (cmd * 2 == 6) at line 45").
    #[serde(default)]
    pub from: Option<String>,
}

// ---------------------------------------------------------------------------
// Multi-module sidecar format (mununu_sv_multi_v1)
// ---------------------------------------------------------------------------

/// Top-level annotation for a multi-module sidecar.
///
/// References multiple `.sv` source files, declares inter-module connections,
/// and specifies composition mode and cross-module properties.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiModuleSvAnnotation {
    /// Schema version identifier (should be `"mununu_sv_multi_v1"`).
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// Per-module configurations.
    pub modules: Vec<ModuleEntry>,

    /// Inter-module port connections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connections: Vec<ConnectionSpec>,

    /// Composition configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition: Option<CompositionConfig>,

    /// Cross-module properties (target the composed automaton).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<PropertyAnnotation>,

    /// Cross-connection discovered values (keyed by `"module.port"`).
    /// Populated by `mununu sv discover`; never hand-written.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub discovered_values: HashMap<String, DiscoveredValues>,

    /// Submodules to treat as black boxes (closed IP, vendor
    /// wrappers, etc.). For each listed module, the adapter parses the
    /// source file to capture the port list, then auto-emits
    /// `<module>.interface.json` + `<module>.gap_report.json` sidecars
    /// alongside its CTXDSL output — the same JSON shape the yosys
    /// frontend emits on `(* blackbox *)` detection (Document B task
    /// B3). The custom-SV path uses this explicit list because its
    /// parser does not recognise the `(* blackbox *)` attribute today;
    /// the user opts in by naming the modules here. Black-box entries
    /// are **not** built as Kripke automata — only their port lists
    /// are extracted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blackbox_modules: Vec<BlackboxModuleEntry>,
}

/// A black-box submodule entry in a multi-module SV sidecar.
///
/// The custom-SV adapter parses the source file to extract the module's
/// port list (used to build the `BlackBoxInterface` sidecar emission)
/// but does **not** build a Kripke automaton for it. The chaotic-stub
/// semantics are entirely conveyed via the emitted JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboxModuleEntry {
    /// Module name (must match an SV module declaration in `source`).
    pub name: String,

    /// Path to the `.sv` source file (relative to the sidecar).
    pub source: String,
}

/// Configuration for a single module within a multi-module sidecar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEntry {
    /// Module name (must match the SV module declaration).
    pub name: String,

    /// Path to the `.sv` source file (relative to the sidecar).
    pub source: String,

    /// Clock domain name (for validation; all modules in synchronous
    /// composition must share the same domain or omit this field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_domain: Option<String>,

    /// Register/internal signal annotations (same format as single-module).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<SignalAnnotation>,

    /// Input port annotations (only non-connected inputs; connected
    /// inputs get their domain from the connection spec).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputAnnotation>,

    /// Signals explicitly marked as controllable within this module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllable: Vec<String>,

    /// Module parameter overrides.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parameters: HashMap<String, i64>,

    /// Module-local discovered values (for non-connected signals).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub discovered_values: HashMap<String, DiscoveredValues>,
}

/// A connection between two module ports.
///
/// Declares that `from` (an output port of one module) drives `to`
/// (an input port of another module). The shared abstraction is declared
/// once and applied uniformly to both sides.
///
/// # Controllability
///
/// The driving module (source of `from`) owns the signal. In the composed
/// automaton, transitions on this connection are controllable by the driving
/// module. The receiving module's transitions synchronize on the shared label
/// without independently asserting controllability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionSpec {
    /// Source port: `"module_name.port_name"` (must be an output port).
    pub from: String,

    /// Destination port: `"module_name.port_name"` (must be an input port).
    pub to: String,

    /// Shared abstraction for the connected signal.
    pub abstraction: SignalAbstraction,

    /// Upper bound (for `bounded_counter` abstraction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<i64>,

    /// Enum variants (for `enum` abstraction).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variants: Option<Vec<String>>,

    /// Value map (for `enum` abstraction with numeric mapping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_map: Option<Vec<ValueMapEntry>>,

    /// Human-readable note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Composition configuration for multi-module verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionConfig {
    /// Composition mode: `"synchronous"` (default) or `"asynchronous"`.
    #[serde(default = "default_synchronous")]
    pub mode: String,

    /// Name for the composed automaton in CTXDSL.
    pub name: String,
}

impl ConnectionSpec {
    /// Parse the `from` field into (module_name, port_name).
    pub fn parse_from(&self) -> Option<(&str, &str)> {
        self.from.split_once('.')
    }

    /// Parse the `to` field into (module_name, port_name).
    pub fn parse_to(&self) -> Option<(&str, &str)> {
        self.to.split_once('.')
    }
}

fn default_synchronous() -> String {
    "synchronous".to_string()
}

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn default_guarantee() -> String {
    "guarantee".to_string()
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_guarantee(v: &str) -> bool {
    v == "guarantee"
}

fn default_other() -> String {
    "OTHER".to_string()
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

/// Look for `<stem>.mununu.json` next to the given `.sv` file.
pub fn find_sidecar(sv_path: &Path) -> Option<std::path::PathBuf> {
    let stem = sv_path.file_stem()?.to_str()?;
    let dir = sv_path.parent()?;
    let sidecar = dir.join(format!("{stem}.mununu.json"));
    if sidecar.exists() {
        Some(sidecar)
    } else {
        None
    }
}

/// Load and parse a `.mununu.json` sidecar file (single-module format).
pub fn load_annotation(path: &Path) -> Result<SvAnnotation, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("failed to parse '{}': {e}", path.display()))
}

/// Detect whether a `.mununu.json` file uses the multi-module format.
///
/// Returns `true` if the JSON contains a top-level `"modules"` array key.
pub fn is_multi_module(path: &Path) -> Result<bool, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("failed to parse '{}': {e}", path.display()))?;
    Ok(value.get("modules").is_some_and(|v| v.is_array()))
}

/// Load and parse a multi-module `.mununu.json` sidecar file.
pub fn load_multi_annotation(path: &Path) -> Result<MultiModuleSvAnnotation, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let ann: MultiModuleSvAnnotation = serde_json::from_str(&content).map_err(|e| {
        format!(
            "failed to parse multi-module sidecar '{}': {e}",
            path.display()
        )
    })?;
    validate_multi_annotation(&ann)?;
    Ok(ann)
}

/// Validate a multi-module annotation for structural correctness.
fn validate_multi_annotation(ann: &MultiModuleSvAnnotation) -> Result<(), String> {
    if ann.modules.is_empty() {
        return Err("multi-module sidecar must have at least one module".to_string());
    }

    // Check for duplicate module names
    let mut seen_modules = std::collections::HashSet::new();
    for entry in &ann.modules {
        if !seen_modules.insert(&entry.name) {
            return Err(format!("duplicate module name '{}'", entry.name));
        }
    }

    // Validate connections reference existing modules
    for conn in &ann.connections {
        let (from_mod, _from_port) = conn.parse_from().ok_or_else(|| {
            format!(
                "invalid connection 'from' format: '{}' (expected module.port)",
                conn.from
            )
        })?;
        let (to_mod, _to_port) = conn.parse_to().ok_or_else(|| {
            format!(
                "invalid connection 'to' format: '{}' (expected module.port)",
                conn.to
            )
        })?;

        if !seen_modules.contains(&from_mod.to_string()) {
            return Err(format!(
                "connection references unknown module '{}' in from: '{}'",
                from_mod, conn.from
            ));
        }
        if !seen_modules.contains(&to_mod.to_string()) {
            return Err(format!(
                "connection references unknown module '{}' in to: '{}'",
                to_mod, conn.to
            ));
        }

        if from_mod == to_mod {
            return Err(format!(
                "connection cannot be within the same module: '{}' → '{}'",
                conn.from, conn.to
            ));
        }
    }

    // Validate clock domains if composition is synchronous
    if let Some(ref comp) = ann.composition
        && comp.mode == "synchronous"
    {
        let domains: Vec<&str> = ann
            .modules
            .iter()
            .filter_map(|m| m.clock_domain.as_deref())
            .collect();
        if domains.len() > 1 {
            let first = domains[0];
            for d in &domains[1..] {
                if *d != first {
                    return Err(format!(
                        "synchronous composition requires same clock domain, \
                         but found '{}' and '{}'. Use \"mode\": \"asynchronous\" \
                         or align clock domains.",
                        first, d
                    ));
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Merging with inline @mununu comments
// ---------------------------------------------------------------------------

use super::ast::{DomainAnnotationKind, Module, MununuPropertyKind};
use crate::adapter::domain::{AbstractValue, AbstractionType, FieldDomain};

/// Merged configuration from sidecar + inline comments.
/// Sidecar entries take precedence over inline comments on conflicts.
pub struct MergedConfig {
    /// Signal domains keyed by name (registers + input ports).
    pub signal_domains: HashMap<String, SignalConfig>,
    /// Input port domains keyed by name.
    pub input_domains: HashMap<String, SignalConfig>,
    /// Controllable signal names.
    pub controllable: Vec<String>,
    /// Properties to verify.
    pub properties: Vec<PropertyAnnotation>,
    /// Parameter overrides.
    pub parameters: HashMap<String, i64>,
    /// Whether kripke mode is forced.
    pub force_kripke: bool,
    /// Whether this config came from a sidecar (explicit opt-in)
    /// or from inline annotations (additive — unlisted signals are auto-detected).
    pub from_sidecar: bool,
}

/// Resolved configuration for a single signal.
pub struct SignalConfig {
    pub preserve: bool,
    pub domain: FieldDomain,
    pub value_map: Vec<(String, i64)>,
    pub combinational: bool,
    /// Override the label prefix used in transitions for this input.
    /// When `Some`, the Kripke builder uses this name instead of the
    /// signal's own name for generating transition labels. This enables
    /// shared labels across multi-module connections.
    pub label_name: Option<String>,
}

/// Build a merged config from an optional sidecar and the parsed module.
pub fn merge_config(annotation: Option<&SvAnnotation>, module: &Module) -> MergedConfig {
    if let Some(ann) = annotation {
        merge_from_sidecar(ann, module)
    } else {
        merge_from_inline(module)
    }
}

fn merge_from_sidecar(ann: &SvAnnotation, _module: &Module) -> MergedConfig {
    let mut signal_domains = HashMap::new();
    let mut input_domains = HashMap::new();

    // Process signal annotations
    for sig in &ann.signals {
        let (domain, value_map) = resolve_signal_domain(sig, ann);
        signal_domains.insert(
            sig.name.clone(),
            SignalConfig {
                preserve: sig.preserve,
                domain,
                value_map,
                combinational: sig.combinational,
                label_name: None,
            },
        );
    }

    // Process input annotations
    for inp in &ann.inputs {
        let (domain, value_map) = resolve_input_domain(inp, ann);
        input_domains.insert(
            inp.name.clone(),
            SignalConfig {
                preserve: inp.preserve,
                domain,
                value_map,
                combinational: false,
                label_name: inp.label_name.clone(),
            },
        );
    }

    // Properties from sidecar
    let properties = ann.properties.clone();

    MergedConfig {
        signal_domains,
        input_domains,
        controllable: ann.controllable.clone(),
        properties,
        parameters: ann.parameters.clone(),
        force_kripke: true, // sidecar always implies kripke mode
        from_sidecar: true,
    }
}

/// Resolve a sidecar `SignalAnnotation` into a [`FieldDomain`].
///
/// Delegates to the shared resolver in [`crate::adapter::sidecar`] so the
/// SV-direct path and the BTOR2-via-Yosys path consume the **same**
/// abstraction logic. Phase-1 sub-deliverable 1 of the RTL roadmap; see
/// [`as-a-business-and-velvety-stallman.md`](https://github.com/vscorza/mununu/blob/main/.claude/plans/as-a-business-and-velvety-stallman.md)
/// §5.
fn resolve_signal_domain(
    sig: &SignalAnnotation,
    ann: &SvAnnotation,
) -> (FieldDomain, Vec<(String, i64)>) {
    crate::adapter::sidecar::resolve_to_field_domain(sig, ann)
}

fn resolve_input_domain(
    inp: &InputAnnotation,
    ann: &SvAnnotation,
) -> (FieldDomain, Vec<(String, i64)>) {
    if !inp.preserve {
        return (
            FieldDomain {
                name: inp.name.clone(),
                abstraction: AbstractionType::Ignored,
                bound: None,
                lower_bound: None,
                variants: None,
                initial: AbstractValue::Counter(0),
            },
            vec![],
        );
    }

    // Reuse signal resolution via a temporary SignalAnnotation
    let sig = SignalAnnotation {
        name: inp.name.clone(),
        preserve: inp.preserve,
        abstraction: inp.abstraction.clone(),
        bound: inp.bound,
        variants: inp.variants.clone(),
        value_map: inp.value_map.clone(),
        combinational: false,
        init_policy: inp.init_policy,
        type_name: None,
        note: None,
    };
    resolve_signal_domain(&sig, ann)
}

/// Build a MergedConfig from inline `@mununu` comments only (no sidecar).
fn merge_from_inline(module: &Module) -> MergedConfig {
    let mut signal_domains = HashMap::new();

    // Domain annotations from inline comments
    for ann in &module.domain_annotations {
        let (domain, value_map) = match &ann.domain_kind {
            DomainAnnotationKind::Boolean => (
                FieldDomain {
                    name: ann.register_name.clone(),
                    abstraction: AbstractionType::Boolean,
                    bound: None,
                    lower_bound: None,
                    variants: None,
                    initial: AbstractValue::Bool(false),
                },
                vec![],
            ),
            DomainAnnotationKind::BoundedCounter { lower, upper } => (
                FieldDomain {
                    name: ann.register_name.clone(),
                    abstraction: AbstractionType::BoundedCounter,
                    bound: Some(*upper),
                    lower_bound: None,
                    variants: None,
                    initial: AbstractValue::Counter(*lower),
                },
                vec![],
            ),
            DomainAnnotationKind::Enum {
                variants,
                value_map,
            } => (
                FieldDomain {
                    name: ann.register_name.clone(),
                    abstraction: AbstractionType::EnumValues,
                    bound: None,
                    lower_bound: None,
                    variants: Some(variants.clone()),
                    initial: AbstractValue::Variant(variants.first().cloned().unwrap_or_default()),
                },
                value_map.clone(),
            ),
            DomainAnnotationKind::Ignored => (
                FieldDomain {
                    name: ann.register_name.clone(),
                    abstraction: AbstractionType::Ignored,
                    bound: None,
                    lower_bound: None,
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                vec![],
            ),
        };
        signal_domains.insert(
            ann.register_name.clone(),
            SignalConfig {
                preserve: domain.abstraction != AbstractionType::Ignored,
                domain,
                value_map,
                combinational: false,
                label_name: None,
            },
        );
    }

    // Properties from inline comments
    let properties: Vec<PropertyAnnotation> = module
        .mununu_properties
        .iter()
        .map(|p| PropertyAnnotation {
            id: p.name.clone(),
            formula: Some(p.formula.clone()),
            description: None,
            role: match p.kind {
                MununuPropertyKind::Assume => "assumption".to_string(),
                _ => "guarantee".to_string(),
            },
            template_ref: None,
        })
        .collect();

    // Parameters from module
    let parameters: HashMap<String, i64> = module
        .parameters
        .iter()
        .map(|p| (p.name.clone(), p.default_value))
        .collect();

    // Auto-detect combinational output ports driven by `always_comb` and add
    // them to signal_domains so the Kripke builder treats them as
    // combinational signals (computed each cycle from the comb logic, not as
    // sequential registers). Without this, an `output logic foo` driven from
    // `always_comb` would be silently treated as a stateful register that
    // never gets updated.
    //
    // We deliberately do NOT auto-add `assign`-driven outputs here: the
    // legacy FSM-extraction path handles them implicitly (output is treated
    // as a pure function of state), and forcing those modules into the
    // Kripke path would change their state space and break existing tests.
    // Modules that need `assign`-driven outputs in the Kripke path can opt
    // in via an inline `@mununu domain` annotation or via a sidecar.
    //
    // Skip ports whose domains were already declared via an inline
    // `@mununu domain foo: ...` annotation — explicit always wins.
    use super::ast::PortDirection;
    for port in &module.ports {
        if port.direction != PortDirection::Output {
            continue;
        }
        if signal_domains.contains_key(&port.name) {
            continue;
        }
        let driven_by_always_comb = always_comb_writes_signal(module, port.name.as_str());
        if !driven_by_always_comb {
            continue;
        }
        let (domain, value_map) = if port.width == 1 {
            (
                FieldDomain {
                    name: port.name.clone(),
                    abstraction: AbstractionType::Boolean,
                    bound: None,
                    lower_bound: None,
                    variants: None,
                    initial: AbstractValue::Bool(false),
                },
                vec![],
            )
        } else if port.width <= 4 {
            (
                FieldDomain {
                    name: port.name.clone(),
                    abstraction: AbstractionType::BoundedCounter,
                    bound: Some((1i64 << port.width) - 1),
                    lower_bound: None,
                    variants: None,
                    initial: AbstractValue::Counter(0),
                },
                vec![],
            )
        } else {
            // Wider combinational outputs without an explicit @mununu domain
            // annotation are skipped — the user must annotate them to bound
            // the state space.
            continue;
        };
        signal_domains.insert(
            port.name.clone(),
            SignalConfig {
                preserve: true,
                domain,
                value_map,
                combinational: true,
                label_name: None,
            },
        );
    }

    MergedConfig {
        signal_domains,
        input_domains: HashMap::new(),
        controllable: module.controllable_signals.clone(),
        properties,
        parameters,
        force_kripke: module.force_kripke,
        from_sidecar: false,
    }
}

// ---------------------------------------------------------------------------
// Sidecar generation helpers (shared between CLI and API)
// ---------------------------------------------------------------------------

/// Determine signal abstraction strategy from bit width.
///
/// - 1-bit → Boolean
/// - 2–4 bit → BoundedCounter (0..2^width-1)
/// - >4 bit → Discover (needs SMT)
pub fn abstract_width(width: usize) -> (SignalAbstraction, Option<i64>) {
    if width == 1 {
        (SignalAbstraction::Boolean, None)
    } else if width <= 4 {
        (
            SignalAbstraction::BoundedCounter,
            1i64.checked_shl(width as u32).map(|v| v - 1),
        )
    } else {
        (SignalAbstraction::Discover, None)
    }
}

/// Build signal annotations from a module's declarations and output ports.
///
/// Walks declarations to find enums and logic signals (excluding ports), then
/// detects combinational outputs (assign-driven output ports not already in
/// the signal list).
pub fn build_signal_annotations(module: &super::ast::Module) -> Vec<SignalAnnotation> {
    use super::ast::{Declaration, PortDirection};
    use std::collections::HashSet;

    let port_names: HashSet<&str> = module.ports.iter().map(|p| p.name.as_str()).collect();

    let mut signals = Vec::new();
    for decl in &module.declarations {
        match decl {
            Declaration::Enum {
                variants,
                var_name: Some(var),
                ..
            } => {
                signals.push(SignalAnnotation {
                    name: var.clone(),
                    preserve: true,
                    abstraction: SignalAbstraction::Enum,
                    bound: None,
                    variants: Some(variants.clone()),
                    value_map: None,
                    combinational: false,
                    init_policy: InitPolicy::Inherit,
                    type_name: None,
                    note: Some("auto-detected typedef enum".to_string()),
                });
            }
            Declaration::Logic { name, width } if !port_names.contains(name.as_str()) => {
                let (abstraction, bound) = abstract_width(*width);
                let note = match abstraction {
                    SignalAbstraction::Boolean => "1-bit flag",
                    SignalAbstraction::BoundedCounter => "small register — bounded counter",
                    _ => "wide register — run `mununu sv discover` to find significant values",
                };
                signals.push(SignalAnnotation {
                    name: name.clone(),
                    preserve: *width <= 4,
                    abstraction,
                    bound,
                    variants: None,
                    value_map: None,
                    combinational: false,
                    init_policy: InitPolicy::Inherit,
                    type_name: None,
                    note: Some(note.to_string()),
                });
            }
            _ => {}
        }
    }

    // Detect combinational output ports — driven either by `assign`
    // statements or by `always_comb` blocks. The latter is the canonical
    // SystemVerilog idiom for FSM next-state logic and conditional output
    // generation; without this detection, a `output logic foo` driven from
    // `always_comb` would be silently treated as a sequential register and
    // its combinational logic discarded.
    for port in &module.ports {
        if port.direction == PortDirection::Output && !signals.iter().any(|s| s.name == port.name) {
            let driven_by_assign = module.assigns.iter().any(|a| a.target.name() == port.name);
            let driven_by_always_comb = always_comb_writes_signal(module, port.name.as_str());
            if driven_by_assign || driven_by_always_comb {
                let (abstraction, bound) = abstract_width(port.width);
                let note = if driven_by_assign {
                    "combinational output (assign-driven)"
                } else {
                    "combinational output (always_comb-driven)"
                };
                signals.push(SignalAnnotation {
                    name: port.name.clone(),
                    preserve: true,
                    abstraction,
                    bound,
                    variants: None,
                    value_map: None,
                    combinational: true,
                    init_policy: InitPolicy::Inherit,
                    type_name: None,
                    note: Some(note.to_string()),
                });
            }
        }
    }

    signals
}

/// Returns true if any `always_comb` block in `module` contains a `BlockingAssign`
/// targeting `signal_name` (recursively walks `if`/`case`/`Block` statements).
fn always_comb_writes_signal(module: &super::ast::Module, signal_name: &str) -> bool {
    use super::ast::AlwaysBlock;
    for block in &module.always_blocks {
        if let AlwaysBlock::AlwaysComb { body } = block
            && statement_writes_signal(body, signal_name)
        {
            return true;
        }
    }
    false
}

fn statement_writes_signal(stmt: &super::ast::Statement, signal_name: &str) -> bool {
    use super::ast::Statement;
    match stmt {
        Statement::BlockingAssign { target, .. } => target.name() == signal_name,
        Statement::Block(stmts) => stmts
            .iter()
            .any(|s| statement_writes_signal(s, signal_name)),
        Statement::If {
            then_branch,
            else_branch,
            ..
        } => {
            statement_writes_signal(then_branch, signal_name)
                || else_branch
                    .as_ref()
                    .is_some_and(|e| statement_writes_signal(e, signal_name))
        }
        Statement::Case {
            branches, default, ..
        } => {
            branches
                .iter()
                .any(|b| statement_writes_signal(&b.body, signal_name))
                || default
                    .as_ref()
                    .is_some_and(|d| statement_writes_signal(d, signal_name))
        }
        Statement::NonblockingAssign { .. } => false,
    }
}

/// Build input annotations from a module's input ports, skipping clock/reset.
pub fn build_input_annotations(module: &super::ast::Module) -> Vec<InputAnnotation> {
    use super::ast::PortDirection;

    module
        .ports
        .iter()
        .filter(|p| {
            p.direction == PortDirection::Input
                && !["clk", "rst", "rst_n"].contains(&p.name.as_str())
        })
        .map(|p| {
            let (abstraction, bound) = abstract_width(p.width);
            InputAnnotation {
                name: p.name.clone(),
                preserve: true,
                abstraction,
                bound,
                variants: None,
                value_map: None,
                label_name: None,
                init_policy: InitPolicy::Inherit,
            }
        })
        .collect()
}

/// Merge newly discovered values into an existing `discovered_values` map,
/// deduplicating by value and sorting.
pub fn merge_discovered_values(
    target: &mut HashMap<String, DiscoveredValues>,
    results: HashMap<String, DiscoveredValues>,
) {
    for (signal, discovered) in results {
        let existing = target.entry(signal).or_insert_with(|| DiscoveredValues {
            values: vec![],
            catch_all: "OTHER".to_string(),
        });
        for new_val in &discovered.values {
            if !existing.values.iter().any(|v| v.value == new_val.value) {
                existing.values.push(new_val.clone());
            }
        }
        existing.values.sort_by_key(|v| v.value);
    }
}

/// Generate a complete `SvAnnotation` sidecar from a parsed module.
///
/// This is the core logic behind `mununu sv init` — auto-detects signals,
/// inputs, and generates a skeleton sidecar with sensible defaults.
pub fn generate_sidecar(module: &super::ast::Module) -> SvAnnotation {
    let signals = build_signal_annotations(module);
    let inputs = build_input_annotations(module);

    SvAnnotation {
        schema: Some("mununu_sv_annotation_v1".to_string()),
        module: module.name.clone(),
        source: None,
        signals,
        inputs,
        controllable: vec![],
        properties: vec![PropertyAnnotation {
            id: "safety".to_string(),
            formula: Some("nu X. ([] X)".to_string()),
            description: Some("No deadlock — all reachable states have successors".to_string()),
            role: "guarantee".to_string(),
            template_ref: None,
        }],
        discovered_values: HashMap::new(),
        parameters: HashMap::new(),
    }
}

/// A parsed sub-module for in-memory multi-module init (no filesystem access).
pub struct ParsedSubModule {
    /// Parsed module AST.
    pub module: super::ast::Module,
    /// Source filename (for the sidecar's `source` field).
    pub source_name: String,
}

/// Generate a multi-module `MultiModuleSvAnnotation` sidecar from a top-level
/// module and its sub-module sources. Works entirely in-memory.
///
/// This is the shared core logic behind both `mununu sv init --multi` (CLI)
/// and `POST /api/v1/sv/init` (API with `additional_sources`).
pub fn generate_multi_sidecar(
    top_module: &super::ast::Module,
    sub_modules: &HashMap<String, ParsedSubModule>,
) -> MultiModuleSvAnnotation {
    use super::ast::PortDirection;

    // Build wire map from instantiation port bindings
    let mut wire_map: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for inst in &top_module.instantiations {
        for conn in &inst.port_connections {
            wire_map.entry(conn.signal_name.clone()).or_default().push((
                inst.instance_name.clone(),
                inst.module_type.clone(),
                conn.port_name.clone(),
            ));
        }
    }

    // Derive connections from shared wires
    let mut connections = Vec::new();
    let mut connected_inputs: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for (wire_name, bindings) in &wire_map {
        if *wire_name == "clk" || *wire_name == "rst" || *wire_name == "rst_n" {
            continue;
        }
        let mut outputs = Vec::new();
        let mut inputs_on_wire = Vec::new();

        for (inst_name, mod_type, port_name) in bindings {
            if let Some(sub) = sub_modules.get(mod_type)
                && let Some(port) = sub.module.ports.iter().find(|p| p.name == *port_name)
            {
                match port.direction {
                    PortDirection::Output => {
                        outputs.push((inst_name, mod_type, port_name, port.width));
                    }
                    PortDirection::Input => {
                        inputs_on_wire.push((inst_name, mod_type, port_name, port.width));
                    }
                    _ => {}
                }
            }
        }

        for (out_inst, out_mod, out_port, width) in &outputs {
            for (in_inst, in_mod, in_port, _) in &inputs_on_wire {
                let (abstraction, bound) = abstract_width(*width);
                connections.push(ConnectionSpec {
                    from: format!("{}.{}", out_mod, out_port),
                    to: format!("{}.{}", in_mod, in_port),
                    abstraction,
                    bound,
                    variants: None,
                    value_map: None,
                    note: Some(format!(
                        "wire '{}': {}.{} -> {}.{}",
                        wire_name, out_inst, out_port, in_inst, in_port
                    )),
                });
                connected_inputs.insert((in_mod.to_string(), in_port.to_string()));
            }
        }
    }

    // Build module entries (one per unique instantiated module type)
    let mut module_entries = Vec::new();
    let mut seen_types = std::collections::HashSet::new();
    for inst in &top_module.instantiations {
        if !seen_types.insert(inst.module_type.clone()) {
            continue;
        }
        if let Some(sub) = sub_modules.get(&inst.module_type) {
            let signals = build_signal_annotations(&sub.module);
            let all_inputs = build_input_annotations(&sub.module);
            let remaining_inputs: Vec<InputAnnotation> = all_inputs
                .into_iter()
                .filter(|inp| {
                    !connected_inputs.contains(&(inst.module_type.clone(), inp.name.clone()))
                })
                .collect();
            let parameters = sub
                .module
                .parameters
                .iter()
                .map(|p| (p.name.clone(), p.default_value))
                .collect();
            module_entries.push(ModuleEntry {
                name: inst.module_type.clone(),
                source: sub.source_name.clone(),
                clock_domain: None,
                signals,
                inputs: remaining_inputs,
                controllable: vec![],
                parameters,
                discovered_values: HashMap::new(),
            });
        }
    }

    MultiModuleSvAnnotation {
        schema: Some("mununu_sv_multi_v1".to_string()),
        modules: module_entries,
        connections,
        composition: Some(CompositionConfig {
            mode: "synchronous".to_string(),
            name: "system".to_string(),
        }),
        properties: vec![PropertyAnnotation {
            id: "safety".to_string(),
            formula: Some("nu X. ([] X)".to_string()),
            description: Some("No deadlock — all states have successors".to_string()),
            role: "guarantee".to_string(),
            template_ref: None,
        }],
        discovered_values: HashMap::new(),
        blackbox_modules: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- R-Y2 (§Phase 8 §8.1) — per-signal init policy ----

    #[test]
    fn init_policy_defaults_to_inherit() {
        assert!(matches!(InitPolicy::default(), InitPolicy::Inherit));
    }

    #[test]
    fn init_policy_yosys_attribute_inherit_is_none() {
        assert!(InitPolicy::Inherit.yosys_attribute().is_none());
    }

    #[test]
    fn init_policy_yosys_attribute_anyconst() {
        assert_eq!(
            InitPolicy::Anyconst.yosys_attribute(),
            Some(("anyconst", 1))
        );
    }

    #[test]
    fn init_policy_yosys_attribute_anyseq() {
        assert_eq!(InitPolicy::Anyseq.yosys_attribute(), Some(("anyseq", 1)));
    }

    #[test]
    fn init_policy_yosys_attribute_zero() {
        assert_eq!(InitPolicy::Zero.yosys_attribute(), Some(("init", 0)));
    }

    #[test]
    fn legacy_sidecar_without_init_policy_loads_with_inherit() {
        // R-Y2 strict additivity: a sidecar that pre-dates R-Y2
        // (no init_policy fields) must deserialise cleanly with
        // init_policy = Inherit on every signal/input.
        let json = r#"{
            "$schema": "mununu_sv_annotation_v1",
            "module": "legacy",
            "signals": [
                { "name": "reg_a", "abstraction": "boolean" }
            ],
            "inputs": [
                { "name": "in_b", "abstraction": "boolean" }
            ]
        }"#;
        let ann: SvAnnotation = serde_json::from_str(json).expect("legacy sidecar parses");
        assert_eq!(ann.signals.len(), 1);
        assert!(matches!(ann.signals[0].init_policy, InitPolicy::Inherit));
        assert_eq!(ann.inputs.len(), 1);
        assert!(matches!(ann.inputs[0].init_policy, InitPolicy::Inherit));
        // init_policy_overrides() should return empty for an
        // all-inherit sidecar.
        assert!(ann.init_policy_overrides().is_empty());
    }

    #[test]
    fn sidecar_with_anyconst_override_round_trips() {
        let json = r#"{
            "$schema": "mununu_sv_annotation_v1",
            "module": "caliptra",
            "signals": [
                {
                    "name": "boot_fsm_ns",
                    "abstraction": "boolean",
                    "init_policy": "anyconst"
                },
                { "name": "other_reg", "abstraction": "boolean" }
            ]
        }"#;
        let ann: SvAnnotation = serde_json::from_str(json).expect("parses");
        assert!(matches!(ann.signals[0].init_policy, InitPolicy::Anyconst));
        assert!(matches!(ann.signals[1].init_policy, InitPolicy::Inherit));
        let overrides = ann.init_policy_overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].0, "boot_fsm_ns");
        assert!(matches!(overrides[0].1, InitPolicy::Anyconst));
    }

    #[test]
    fn init_policy_overrides_orders_signals_then_inputs() {
        let json = r#"{
            "$schema": "mununu_sv_annotation_v1",
            "module": "demo",
            "signals": [
                { "name": "sig_a", "abstraction": "boolean", "init_policy": "anyconst" },
                { "name": "sig_b", "abstraction": "boolean" },
                { "name": "sig_c", "abstraction": "boolean", "init_policy": "anyseq" }
            ],
            "inputs": [
                { "name": "in_x", "abstraction": "boolean", "init_policy": "zero" },
                { "name": "in_y", "abstraction": "boolean" }
            ]
        }"#;
        let ann: SvAnnotation = serde_json::from_str(json).expect("parses");
        let overrides = ann.init_policy_overrides();
        // signals first (in declaration order, skipping Inherit), then inputs.
        let names: Vec<&str> = overrides.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["sig_a", "sig_c", "in_x"]);
        // policies preserved
        assert!(matches!(overrides[0].1, InitPolicy::Anyconst));
        assert!(matches!(overrides[1].1, InitPolicy::Anyseq));
        assert!(matches!(overrides[2].1, InitPolicy::Zero));
    }

    #[test]
    fn parse_minimal_sidecar() {
        let json = r#"{
            "$schema": "mununu_sv_annotation_v1",
            "module": "fifo",
            "signals": [
                {"name": "state", "abstraction": "enum", "variants": ["IDLE", "WRITING", "READING"]},
                {"name": "fill", "abstraction": "bounded_counter", "bound": 4}
            ],
            "properties": [
                {"id": "safety", "formula": "nu X. ([] X)"}
            ]
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        assert_eq!(ann.module, "fifo");
        assert_eq!(ann.signals.len(), 2);
        assert_eq!(ann.signals[0].abstraction, SignalAbstraction::Enum);
        assert_eq!(ann.signals[1].bound, Some(4));
        assert_eq!(ann.properties.len(), 1);
    }

    #[test]
    fn parse_discover_with_discovered_values() {
        let json = r#"{
            "module": "alu",
            "signals": [
                {"name": "cmd", "abstraction": "discover"}
            ],
            "discovered_values": {
                "cmd": {
                    "values": [
                        {"value": 0, "name": "NOP", "from": "case label at line 38"},
                        {"value": 1, "name": "LOAD", "from": "case label at line 39"}
                    ],
                    "catch_all": "OTHER"
                }
            },
            "properties": []
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        let discovered = ann.discovered_values.get("cmd").unwrap();
        assert_eq!(discovered.values.len(), 2);
        assert_eq!(discovered.values[0].name, "NOP");
        assert_eq!(discovered.values[0].value, 0);
        assert_eq!(discovered.catch_all, "OTHER");
    }

    #[test]
    fn parse_full_sidecar() {
        let json = r#"{
            "$schema": "mununu_sv_annotation_v1",
            "module": "alu",
            "source": "alu.sv",
            "signals": [
                {"name": "acc", "abstraction": "bounded_counter", "bound": 7},
                {"name": "cmd", "abstraction": "discover", "note": "opcode register"},
                {"name": "data_buf", "preserve": false}
            ],
            "inputs": [
                {"name": "start", "abstraction": "boolean"},
                {"name": "operand", "abstraction": "bounded_counter", "bound": 3},
                {"name": "data_in", "preserve": false}
            ],
            "controllable": ["acc"],
            "properties": [
                {"id": "safety", "formula": "nu X. ([] X)", "description": "No deadlock"},
                {"id": "reset_clears", "formula": "nu X. ([] X)", "role": "guarantee"}
            ],
            "parameters": {"WIDTH": 8},
            "discovered_values": {}
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        assert_eq!(ann.module, "alu");
        assert_eq!(ann.signals.len(), 3);
        assert!(!ann.signals[2].preserve); // data_buf excluded
        assert_eq!(ann.inputs.len(), 3);
        assert!(!ann.inputs[2].preserve); // data_in excluded
        assert_eq!(ann.controllable, vec!["acc"]);
        assert_eq!(ann.parameters.get("WIDTH"), Some(&8));
    }

    #[test]
    fn resolve_discover_with_values() {
        let json = r#"{
            "module": "test",
            "signals": [
                {"name": "cmd", "abstraction": "discover"}
            ],
            "discovered_values": {
                "cmd": {
                    "values": [
                        {"value": 0, "name": "NOP"},
                        {"value": 3, "name": "START"}
                    ],
                    "catch_all": "OTHER"
                }
            },
            "properties": []
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        let (domain, value_map) = resolve_signal_domain(&ann.signals[0], &ann);

        assert_eq!(domain.abstraction, AbstractionType::EnumValues);
        assert_eq!(
            domain.variants.as_ref().unwrap(),
            &["NOP", "START", "OTHER"]
        );
        assert_eq!(
            value_map,
            vec![("NOP".to_string(), 0), ("START".to_string(), 3)]
        );
    }

    #[test]
    fn resolve_discover_without_values() {
        let json = r#"{
            "module": "test",
            "signals": [
                {"name": "cmd", "abstraction": "discover"}
            ],
            "properties": []
        }"#;

        let ann: SvAnnotation = serde_json::from_str(json).unwrap();
        let (domain, _) = resolve_signal_domain(&ann.signals[0], &ann);

        // No discovered values yet → Ignored (user must run discover first)
        assert_eq!(domain.abstraction, AbstractionType::Ignored);
    }

    // ---------------------------------------------------------------
    // Multi-module sidecar format tests
    // ---------------------------------------------------------------

    #[test]
    fn parse_multi_module_minimal() {
        let json = r#"{
            "$schema": "mununu_sv_multi_v1",
            "modules": [
                {
                    "name": "arbiter",
                    "source": "arbiter.sv",
                    "signals": [
                        {"name": "state", "abstraction": "enum", "variants": ["IDLE", "GRANT"]}
                    ]
                },
                {
                    "name": "datapath",
                    "source": "datapath.sv",
                    "signals": [
                        {"name": "busy", "abstraction": "boolean"}
                    ]
                }
            ],
            "connections": [
                {"from": "arbiter.grant", "to": "datapath.start", "abstraction": "boolean"}
            ],
            "composition": {"mode": "synchronous", "name": "system"},
            "properties": [
                {"id": "no_deadlock", "formula": "nu X. ([] X)"}
            ]
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        assert_eq!(ann.modules.len(), 2);
        assert_eq!(ann.modules[0].name, "arbiter");
        assert_eq!(ann.modules[1].name, "datapath");
        assert_eq!(ann.connections.len(), 1);
        assert_eq!(ann.connections[0].parse_from(), Some(("arbiter", "grant")));
        assert_eq!(ann.connections[0].parse_to(), Some(("datapath", "start")));
        assert_eq!(ann.composition.as_ref().unwrap().mode, "synchronous");
        assert_eq!(ann.properties.len(), 1);
    }

    #[test]
    fn parse_multi_module_with_discovery() {
        let json = r#"{
            "modules": [
                {"name": "ctrl", "source": "ctrl.sv", "signals": []},
                {"name": "exec", "source": "exec.sv", "signals": []}
            ],
            "connections": [
                {
                    "from": "ctrl.cmd_out",
                    "to": "exec.cmd_in",
                    "abstraction": "discover"
                }
            ],
            "composition": {"name": "system"},
            "discovered_values": {
                "ctrl.cmd_out": {
                    "values": [
                        {"value": 0, "name": "NOP", "from": "SMT: guard at line 15"},
                        {"value": 3, "name": "EXEC", "from": "SMT: guard at line 22"}
                    ],
                    "catch_all": "OTHER"
                }
            }
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        assert_eq!(ann.connections[0].abstraction, SignalAbstraction::Discover);
        let disc = ann.discovered_values.get("ctrl.cmd_out").unwrap();
        assert_eq!(disc.values.len(), 2);
        assert_eq!(disc.values[1].name, "EXEC");
        assert_eq!(disc.catch_all, "OTHER");
    }

    #[test]
    fn validate_multi_module_duplicate_name() {
        let json = r#"{
            "modules": [
                {"name": "foo", "source": "a.sv", "signals": []},
                {"name": "foo", "source": "b.sv", "signals": []}
            ]
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let err = validate_multi_annotation(&ann).unwrap_err();
        assert!(err.contains("duplicate module name"));
    }

    #[test]
    fn validate_multi_module_unknown_module_in_connection() {
        let json = r#"{
            "modules": [
                {"name": "a", "source": "a.sv", "signals": []}
            ],
            "connections": [
                {"from": "a.out", "to": "b.inp", "abstraction": "boolean"}
            ]
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let err = validate_multi_annotation(&ann).unwrap_err();
        assert!(err.contains("unknown module 'b'"));
    }

    #[test]
    fn validate_multi_module_same_module_connection() {
        let json = r#"{
            "modules": [
                {"name": "a", "source": "a.sv", "signals": []}
            ],
            "connections": [
                {"from": "a.out", "to": "a.inp", "abstraction": "boolean"}
            ]
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let err = validate_multi_annotation(&ann).unwrap_err();
        assert!(err.contains("cannot be within the same module"));
    }

    #[test]
    fn validate_multi_module_clock_domain_mismatch() {
        let json = r#"{
            "modules": [
                {"name": "a", "source": "a.sv", "clock_domain": "clk_fast", "signals": []},
                {"name": "b", "source": "b.sv", "clock_domain": "clk_slow", "signals": []}
            ],
            "composition": {"mode": "synchronous", "name": "system"}
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let err = validate_multi_annotation(&ann).unwrap_err();
        assert!(err.contains("synchronous composition requires same clock domain"));
    }

    #[test]
    fn validate_multi_module_async_different_clocks_ok() {
        let json = r#"{
            "modules": [
                {"name": "a", "source": "a.sv", "clock_domain": "clk_fast", "signals": []},
                {"name": "b", "source": "b.sv", "clock_domain": "clk_slow", "signals": []}
            ],
            "composition": {"mode": "asynchronous", "name": "system"}
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        assert!(validate_multi_annotation(&ann).is_ok());
    }

    #[test]
    fn connection_spec_parse_helpers() {
        let conn = ConnectionSpec {
            from: "arbiter.grant_a".to_string(),
            to: "datapath.start".to_string(),
            abstraction: SignalAbstraction::Boolean,
            bound: None,
            variants: None,
            value_map: None,
            note: None,
        };
        assert_eq!(conn.parse_from(), Some(("arbiter", "grant_a")));
        assert_eq!(conn.parse_to(), Some(("datapath", "start")));
    }

    #[test]
    fn multi_module_with_enum_connection() {
        let json = r#"{
            "modules": [
                {"name": "ctrl", "source": "ctrl.sv", "signals": []},
                {"name": "mem", "source": "mem.sv", "signals": []}
            ],
            "connections": [
                {
                    "from": "ctrl.cmd",
                    "to": "mem.cmd",
                    "abstraction": "enum",
                    "variants": ["READ", "WRITE", "IDLE"],
                    "value_map": [
                        {"name": "READ", "value": 1},
                        {"name": "WRITE", "value": 2},
                        {"name": "IDLE", "value": 0}
                    ],
                    "note": "Memory command bus"
                }
            ],
            "composition": {"name": "system"}
        }"#;

        let ann: MultiModuleSvAnnotation = serde_json::from_str(json).unwrap();
        let conn = &ann.connections[0];
        assert_eq!(conn.abstraction, SignalAbstraction::Enum);
        assert_eq!(conn.variants.as_ref().unwrap().len(), 3);
        assert_eq!(conn.value_map.as_ref().unwrap().len(), 3);
        assert_eq!(conn.note.as_deref(), Some("Memory command bus"));
    }

    // ---- R-S5 (§Phase 9 §9.1) — type-driven valuation auto-widening ----

    fn caliptra_typedefs()
    -> std::collections::HashMap<String, super::super::typedef_extract::TypedefEnum> {
        super::super::typedef_extract::extract_typedef_enums(
            r#"typedef enum logic [2:0] {
                BOOT_IDLE   = 3'b000,
                BOOT_FUSE   = 3'b001,
                BOOT_FW_RST = 3'b010,
                BOOT_WAIT   = 3'b011,
                BOOT_DONE   = 3'b100
            } boot_fsm_state_e;"#,
        )
    }

    fn empty_sv_annotation() -> SvAnnotation {
        SvAnnotation {
            schema: Some("mununu_sv_annotation_v1".into()),
            module: "test".into(),
            source: None,
            signals: vec![],
            inputs: vec![],
            controllable: vec![],
            properties: vec![],
            discovered_values: HashMap::new(),
            parameters: HashMap::new(),
        }
    }

    fn signal_with_type(
        name: &str,
        type_name: &str,
        abstraction: SignalAbstraction,
    ) -> SignalAnnotation {
        SignalAnnotation {
            name: name.into(),
            preserve: true,
            abstraction,
            bound: None,
            variants: None,
            value_map: None,
            combinational: false,
            init_policy: InitPolicy::Inherit,
            type_name: Some(type_name.into()),
            note: None,
        }
    }

    #[test]
    fn r_s5_widens_discover_with_named_and_unmatched_variants() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(signal_with_type(
            "boot_fsm_ns",
            "boot_fsm_state_e",
            SignalAbstraction::Discover,
        ));
        let applied = ann.apply_type_driven_widening(&caliptra_typedefs());

        assert_eq!(applied.len(), 1);
        assert_eq!(
            applied[0],
            ("boot_fsm_ns".into(), "boot_fsm_state_e".into(), 8)
        );

        let sig = &ann.signals[0];
        assert_eq!(sig.abstraction, SignalAbstraction::Enum);
        let variants = sig.variants.as_ref().unwrap();
        // 5 named + 3 unmatched, sorted by encoding value
        assert_eq!(variants.len(), 8);
        assert_eq!(variants[0], "BOOT_IDLE");
        assert_eq!(variants[4], "BOOT_DONE");
        assert_eq!(variants[5], "UNMATCHED_5");
        assert_eq!(variants[7], "UNMATCHED_7");

        let vm = sig.value_map.as_ref().unwrap();
        assert_eq!(vm.len(), 8);
        assert_eq!(vm[0].value, 0);
        assert_eq!(vm[7].value, 7);
    }

    #[test]
    fn r_s5_skips_signals_without_type_name() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(SignalAnnotation {
            name: "wait_count".into(),
            preserve: true,
            abstraction: SignalAbstraction::Discover,
            bound: None,
            variants: None,
            value_map: None,
            combinational: false,
            init_policy: InitPolicy::Inherit,
            type_name: None,
            note: None,
        });
        let applied = ann.apply_type_driven_widening(&caliptra_typedefs());
        assert!(applied.is_empty());
        assert_eq!(ann.signals[0].abstraction, SignalAbstraction::Discover);
    }

    #[test]
    fn r_s5_skips_signals_with_unknown_type() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(signal_with_type(
            "unknown_signal",
            "no_such_type_t",
            SignalAbstraction::Discover,
        ));
        let applied = ann.apply_type_driven_widening(&caliptra_typedefs());
        assert!(applied.is_empty());
    }

    #[test]
    fn r_s5_skips_signals_with_explicit_variants() {
        let mut ann = empty_sv_annotation();
        let mut sig = signal_with_type("boot_fsm_ns", "boot_fsm_state_e", SignalAbstraction::Enum);
        sig.variants = Some(vec!["E0".into(), "E1".into()]);
        sig.value_map = Some(vec![
            ValueMapEntry {
                name: "E0".into(),
                value: 0,
            },
            ValueMapEntry {
                name: "E1".into(),
                value: 1,
            },
        ]);
        ann.signals.push(sig);
        let applied = ann.apply_type_driven_widening(&caliptra_typedefs());
        // User-supplied variants are NEVER overwritten.
        assert!(applied.is_empty());
        assert_eq!(ann.signals[0].variants.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn r_s5_skips_signals_with_non_widenable_abstraction() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(signal_with_type(
            "some_flag",
            "boot_fsm_state_e",
            SignalAbstraction::Boolean,
        ));
        let applied = ann.apply_type_driven_widening(&caliptra_typedefs());
        assert!(applied.is_empty());
    }

    #[test]
    fn r_s5_widens_enum_with_empty_variants() {
        let mut ann = empty_sv_annotation();
        let sig = signal_with_type("boot_fsm_ns", "boot_fsm_state_e", SignalAbstraction::Enum);
        // variants and value_map are None — treated as empty, eligible for widening
        ann.signals.push(sig);
        let applied = ann.apply_type_driven_widening(&caliptra_typedefs());
        assert_eq!(applied.len(), 1);
        assert_eq!(ann.signals[0].variants.as_ref().unwrap().len(), 8);
    }

    // ---- R-S7 (§Phase 9 §9.1) — property-syntactic predicate seeding ----

    #[test]
    fn r_s7_split_signal_integer_suffix_basic() {
        let signals: std::collections::HashSet<String> =
            ["count".to_string(), "boot_fsm_ns".to_string()]
                .into_iter()
                .collect();
        assert_eq!(
            split_signal_integer_suffix("count_5", &signals),
            Some(("count".to_string(), 5))
        );
        assert_eq!(
            split_signal_integer_suffix("boot_fsm_ns_7", &signals),
            Some(("boot_fsm_ns".to_string(), 7))
        );
        // Longest-prefix-match wins
        assert_eq!(
            split_signal_integer_suffix("boot_fsm_ns_42", &signals),
            Some(("boot_fsm_ns".to_string(), 42))
        );
        // Non-numeric suffix returns None (typedef variants handled by R-S5)
        assert_eq!(
            split_signal_integer_suffix("boot_fsm_ns_BOOT_IDLE", &signals),
            None
        );
        // Unknown prefix returns None
        assert_eq!(split_signal_integer_suffix("unknown_5", &signals), None);
    }

    #[test]
    fn r_s7_seeds_integer_predicates_from_formula() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(SignalAnnotation {
            name: "count".into(),
            preserve: true,
            abstraction: SignalAbstraction::Discover,
            bound: None,
            variants: None,
            value_map: None,
            combinational: false,
            init_policy: InitPolicy::Inherit,
            type_name: None,
            note: None,
        });
        ann.properties.push(PropertyAnnotation {
            id: "p".into(),
            formula: Some("nu X. ((!count_3) && ([] X))".into()),
            description: None,
            role: "guarantee".into(),
            template_ref: None,
        });
        let applied = ann.apply_property_syntactic_seeding();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].0, "count");
        assert_eq!(applied[0].1, vec![3]);
        let sig = &ann.signals[0];
        assert_eq!(sig.abstraction, SignalAbstraction::Enum);
        let vm = sig.value_map.as_ref().unwrap();
        assert_eq!(vm.len(), 1);
        assert_eq!(vm[0].name, "count_3");
        assert_eq!(vm[0].value, 3);
    }

    #[test]
    fn r_s7_skips_signals_with_explicit_value_map() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(SignalAnnotation {
            name: "count".into(),
            preserve: true,
            abstraction: SignalAbstraction::Enum,
            bound: None,
            variants: Some(vec!["ZERO".into(), "ONE".into()]),
            value_map: Some(vec![
                ValueMapEntry {
                    name: "ZERO".into(),
                    value: 0,
                },
                ValueMapEntry {
                    name: "ONE".into(),
                    value: 1,
                },
            ]),
            combinational: false,
            init_policy: InitPolicy::Inherit,
            type_name: None,
            note: None,
        });
        ann.properties.push(PropertyAnnotation {
            id: "p".into(),
            formula: Some("nu X. ((!count_3) && ([] X))".into()),
            description: None,
            role: "guarantee".into(),
            template_ref: None,
        });
        let applied = ann.apply_property_syntactic_seeding();
        // count_3 is new (3 isn't in {0, 1}), so it gets added —
        // additive, never overwrites existing.
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].1, vec![3]);
        assert_eq!(ann.signals[0].value_map.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn r_s7_skips_non_widenable_abstractions() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(SignalAnnotation {
            name: "flag".into(),
            preserve: true,
            abstraction: SignalAbstraction::Boolean,
            bound: None,
            variants: None,
            value_map: None,
            combinational: false,
            init_policy: InitPolicy::Inherit,
            type_name: None,
            note: None,
        });
        ann.properties.push(PropertyAnnotation {
            id: "p".into(),
            formula: Some("nu X. ((!flag_3) && ([] X))".into()),
            description: None,
            role: "guarantee".into(),
            template_ref: None,
        });
        let applied = ann.apply_property_syntactic_seeding();
        // Boolean abstraction can't take per-value discriminators; skipped.
        assert!(applied.is_empty());
    }

    #[test]
    fn r_s7_dedupes_existing_value_map_entries() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(SignalAnnotation {
            name: "count".into(),
            preserve: true,
            abstraction: SignalAbstraction::Enum,
            bound: None,
            variants: Some(vec!["count_3".into()]),
            value_map: Some(vec![ValueMapEntry {
                name: "count_3".into(),
                value: 3,
            }]),
            combinational: false,
            init_policy: InitPolicy::Inherit,
            type_name: None,
            note: None,
        });
        ann.properties.push(PropertyAnnotation {
            id: "p".into(),
            formula: Some("nu X. ((!count_3) && ([] X))".into()),
            description: None,
            role: "guarantee".into(),
            template_ref: None,
        });
        let applied = ann.apply_property_syntactic_seeding();
        // count_3 already in value_map → no-op.
        assert!(applied.is_empty());
    }

    #[test]
    fn r_s5_deterministic_order_signals_in_declaration_order() {
        let mut ann = empty_sv_annotation();
        ann.signals.push(signal_with_type(
            "sig_b",
            "boot_fsm_state_e",
            SignalAbstraction::Discover,
        ));
        ann.signals.push(signal_with_type(
            "sig_a",
            "boot_fsm_state_e",
            SignalAbstraction::Discover,
        ));
        let applied = ann.apply_type_driven_widening(&caliptra_typedefs());
        assert_eq!(applied.len(), 2);
        // Reported in sidecar declaration order, not alphabetical
        assert_eq!(applied[0].0, "sig_b");
        assert_eq!(applied[1].0, "sig_a");
    }
}
