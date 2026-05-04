//! State space derivation — converts abstract field domains + method
//! guards/effects into explicit automaton states and transitions.
//!
//! The core algorithm:
//! 1. Enumerate states from the cross-product of field abstract domains
//! 2. For each method, determine which states satisfy its guards
//! 3. Compute target states by applying effects
//! 4. Prune unreachable states from initial state
//! 5. Generate labels and noop self-loops

use super::call_summary::{CallEffect, CallGuard};
use crate::adapter::domain::{AbstractState, AbstractValue, FieldDomain};
use crate::adapter::state_enum;
use std::collections::HashMap;

/// A guard condition extracted from source.
#[derive(Debug, Clone)]
pub struct Guard {
    /// Field this guard checks.
    pub field: String,
    /// Required condition.
    pub condition: CallGuard,
}

/// An effect extracted from source.
#[derive(Debug, Clone)]
pub struct Effect {
    /// Field this effect modifies.
    pub field: String,
    /// How the field is modified.
    pub effect: CallEffect,
    /// Explicit value to set (for SetTrue/SetFalse/SetPresent/SetAbsent and variants).
    pub value: Option<AbstractValue>,
}

/// A method's extracted behavior.
#[derive(Debug, Clone)]
pub struct MethodBehavior {
    /// Method name.
    pub name: String,
    /// Guards that must be satisfied for this method to fire.
    pub guards: Vec<Guard>,
    /// Effects on state fields.
    pub effects: Vec<Effect>,
    /// Whether this method is controllable.
    pub controllable: bool,
    /// Source line range for traceability.
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
}

/// Derived automaton: states + transitions + labels.
#[derive(Debug, Clone)]
pub struct DerivedAutomaton {
    /// Automaton name.
    pub name: String,
    /// Named states (index corresponds to state ID).
    pub states: Vec<DerivedState>,
    /// Transitions.
    pub transitions: Vec<DerivedTransition>,
    /// Controllable labels.
    pub controllable_labels: Vec<String>,
    /// Diagnostic warnings produced during derivation (degenerate model,
    /// state-space size hints). Empty for typical extractions.
    pub warnings: Vec<String>,
}

/// A derived state.
#[derive(Debug, Clone)]
pub struct DerivedState {
    pub name: String,
    pub is_initial: bool,
    /// The abstract state this corresponds to.
    pub abstract_state: AbstractState,
}

/// A derived transition.
#[derive(Debug, Clone)]
pub struct DerivedTransition {
    pub from: String,
    pub to: String,
    pub label: String,
}

/// State-space size threshold above which a warning is emitted.
const STATE_SPACE_WARN_THRESHOLD: usize = 1 << 12;
/// State-space size threshold above which the extractor refuses to enumerate
/// (matches the SystemVerilog adapter at `kripke.rs:207`).
const STATE_SPACE_HARD_LIMIT: usize = 1 << 18;

/// Derive an automaton from field domains and method behaviors.
///
/// Before cross-product enumeration, this checks the estimated state-space
/// size against two thresholds:
/// - **2^12 = 4,096** — emits a `[mununu] WARN` line on stderr so the user
///   is aware that verification will be slow.
/// - **2^18 = 262,144** — returns an empty automaton and emits a `[mununu]
///   ERROR` line on stderr to prevent OOM. The caller treats an empty result
///   as "extraction skipped"; the user must coarsen field abstractions or set
///   narrower bounds.
pub fn derive_automaton(
    automaton_name: &str,
    fields: &[FieldDomain],
    methods: &[MethodBehavior],
    state_name_overrides: &HashMap<String, String>,
    label_prefix: &str,
    add_noop: bool,
) -> DerivedAutomaton {
    // Step 0: pre-enumeration state-space size check (priority_roadmap §2.5).
    // Catches cross-product blowups BEFORE allocating the full state vector.
    // Uses eprintln rather than a structured warning channel because
    // `derive_automaton` does not currently thread an AdapterWarning vec; this
    // keeps the change additive. Future work: surface as AdapterWarning.
    let field_refs: Vec<&FieldDomain> = fields.iter().collect();
    if let Some(estimated) = state_enum::state_space_size(&field_refs) {
        if estimated > STATE_SPACE_HARD_LIMIT {
            eprintln!(
                "[mununu] ERROR: automaton '{}' estimated at {} states, exceeds hard limit \
                 ({}). Coarsen field abstractions or set narrower bounds in the extraction \
                 config. Returning empty automaton.",
                automaton_name, estimated, STATE_SPACE_HARD_LIMIT
            );
            return DerivedAutomaton {
                name: automaton_name.to_string(),
                states: vec![],
                transitions: vec![],
                controllable_labels: vec![],
                warnings: vec![],
            };
        }
        if estimated > STATE_SPACE_WARN_THRESHOLD {
            eprintln!(
                "[mununu] WARN: automaton '{}' estimated at {} states (threshold {}); \
                 verification may be slow.",
                automaton_name, estimated, STATE_SPACE_WARN_THRESHOLD
            );
        }
    }

    // Step 1: Enumerate all abstract states (cross-product)
    let all_states = state_enum::enumerate_cross_product(&field_refs);
    if all_states.is_empty() {
        return DerivedAutomaton {
            name: automaton_name.to_string(),
            states: vec![],
            transitions: vec![],
            controllable_labels: vec![],
            warnings: vec![],
        };
    }

    // Step 2: Find initial state
    let initial_state = state_enum::initial_state_from_fields(fields);

    // Step 3: Name states
    let state_names: HashMap<AbstractState, String> = all_states
        .iter()
        .map(|s| {
            let auto_name = state_enum::make_state_name_ordered(s, fields);
            let name = state_name_overrides
                .get(&auto_name)
                .cloned()
                .unwrap_or(auto_name);
            (s.clone(), name)
        })
        .collect();

    // Step 4: Compute transitions
    let mut transitions = Vec::new();
    let mut controllable_labels = Vec::new();

    for method in methods {
        let label = format!("{label_prefix}{}", method.name);
        if method.controllable && !controllable_labels.contains(&label) {
            controllable_labels.push(label.clone());
        }

        for state in &all_states {
            if guards_satisfied(state, &method.guards) {
                let targets = apply_effects_havoc(state, &method.effects, fields);
                let source_name = &state_names[state];
                for target in &targets {
                    if let Some(target_name) = state_names.get(target) {
                        transitions.push(DerivedTransition {
                            from: source_name.clone(),
                            to: target_name.clone(),
                            label: label.clone(),
                        });
                    }
                    // If target is outside the domain (e.g., counter overflow),
                    // the transition is dropped.
                }
            }
        }
    }

    // Step 5: Add noop self-loops
    if add_noop {
        for state in &all_states {
            let name = &state_names[state];
            transitions.push(DerivedTransition {
                from: name.clone(),
                to: name.clone(),
                label: "noop".to_string(),
            });
        }
    }

    // Step 6: Prune unreachable states via shared BFS
    let initial_name = state_names.get(&initial_state).cloned().unwrap_or_default();
    let edges: Vec<(&str, &str)> = transitions
        .iter()
        .map(|t| (t.from.as_str(), t.to.as_str()))
        .collect();
    let reachable = state_enum::bfs_reachable(&initial_name, &edges);

    let name_to_state: HashMap<&str, &AbstractState> = all_states
        .iter()
        .filter_map(|s| state_names.get(s).map(|n| (n.as_str(), s)))
        .collect();

    let reachable_states: Vec<DerivedState> = reachable
        .iter()
        .filter_map(|name| {
            let abstract_state = name_to_state.get(name.as_str())?;
            Some(DerivedState {
                name: name.clone(),
                is_initial: *name == initial_name,
                abstract_state: (*abstract_state).clone(),
            })
        })
        .collect();

    let reachable_transitions: Vec<DerivedTransition> = transitions
        .into_iter()
        .filter(|t| reachable.contains(&t.from) && reachable.contains(&t.to))
        .collect();

    let mut warnings = Vec::new();
    // GAP-005b: count transitions that actually mutate the state — i.e.,
    // go from a source state to a *different* target state. Pre-fix logic
    // counted any non-`noop` transition, which over-counted method-name-
    // labeled self-loops (e.g., `s0 --ev_close--> s0` on a 1-state model).
    // Self-loops contribute no real behavior, so the degenerate-model check
    // should ignore them. Surfaced by MCP-004 + MCP-005 re-validation
    // (independent reproductions); the warning failed to fire on the very
    // cases it was meant to catch.
    let state_mutating_transitions = reachable_transitions
        .iter()
        .filter(|t| t.from != t.to)
        .count();
    if reachable_states.len() <= 1 && state_mutating_transitions == 0 {
        warnings.push(format!(
            "[mununu] WARN: automaton '{}' is degenerate ({} states, {} state-mutating transitions). \
             Likely cause: state lives outside `self.*` / `this.*` (module-level ContextVar / \
             AsyncLocalStorage / shared handles, or in fields not currently scanned by the \
             extractor — abstract class, private with `?`, dataclass). Consider listing the \
             missing fields in state_fields.include or enabling state_fields.module_level.",
            automaton_name,
            reachable_states.len(),
            state_mutating_transitions,
        ));
    }

    DerivedAutomaton {
        name: automaton_name.to_string(),
        states: reachable_states,
        transitions: reachable_transitions,
        controllable_labels,
        warnings,
    }
}

/// Check if all guards are satisfied in the given state.
fn guards_satisfied(state: &AbstractState, guards: &[Guard]) -> bool {
    // GAP-005e/f: MustBeTrue / MustBeFalse use JavaScript/Python truthiness
    // semantics — they're produced by the extractor for `if (this.foo)` /
    // `if not self.foo` style guards, where the AST gives no signal about
    // whether `foo` is Boolean or Optional. So we accept both:
    //   MustBeTrue  matches Bool(true)  OR Present(true)
    //   MustBeFalse matches Bool(false) OR Present(false)
    // The dedicated MustBePresent / MustBeAbsent guards keep their strict
    // Presence-only semantics for callers that explicitly want them
    // (e.g., explicit `null` / `undefined` checks).
    //
    // Pre-fix bug: extracting `if (this._transport)` on a Presence-
    // abstracted field produced a `MustBeFalse` guard (after early-exit
    // inversion) that never matched the initial `Present(false)` state,
    // so no `ev_connect` transitions were emitted from the auto-extracted
    // model. Surfaced by MCP-004 re-validation.
    fn cond_satisfied(val: &AbstractValue, cond: &CallGuard) -> bool {
        match cond {
            CallGuard::CounterGtZero => matches!(val, AbstractValue::Counter(n) if *n > 0),
            CallGuard::CounterEqZero => matches!(val, AbstractValue::Counter(0)),
            CallGuard::MustBePresent => matches!(val, AbstractValue::Present(true)),
            CallGuard::MustBeAbsent => matches!(val, AbstractValue::Present(false)),
            CallGuard::MustBeTrue => matches!(
                val,
                AbstractValue::Bool(true) | AbstractValue::Present(true)
            ),
            CallGuard::MustBeFalse => matches!(
                val,
                AbstractValue::Bool(false) | AbstractValue::Present(false)
            ),
            CallGuard::MustEqual(variant) => {
                matches!(val, AbstractValue::Variant(v) if v == variant)
            }
            CallGuard::Disjunction(a, b) => cond_satisfied(val, a) || cond_satisfied(val, b),
            CallGuard::Conjunction(a, b) => cond_satisfied(val, a) && cond_satisfied(val, b),
            CallGuard::None => true,
        }
    }

    guards.iter().all(|g| {
        let val = match state.get(&g.field) {
            Some(v) => v,
            None => return true, // field not modeled → guard vacuously true
        };
        cond_satisfied(val, &g.condition)
    })
}

/// Apply effects to a state, returning all possible target states.
///
/// For deterministic effects (SetTrue, IncrementCounter, etc.) this returns
/// exactly one target state. For `Unknown` effects (L6 havoc semantics),
/// it returns one target per possible value of the affected field —
/// a sound over-approximation covering all behaviors the unknown call might have.
fn apply_effects_havoc(
    state: &AbstractState,
    effects: &[Effect],
    fields: &[FieldDomain],
) -> Vec<AbstractState> {
    let mut current_states = vec![state.clone()];

    for effect in effects {
        // Apply explicit value override first
        if let Some(explicit) = &effect.value {
            for s in &mut current_states {
                if let Some(val) = s.get_mut(&effect.field) {
                    *val = explicit.clone();
                }
            }
            continue;
        }

        match &effect.effect {
            CallEffect::Unknown => {
                // Havoc: branch into all possible values for this field
                let field_domain = fields.iter().find(|f| f.name == effect.field);
                if let Some(fd) = field_domain {
                    let possible_values = fd.values();
                    if possible_values.len() > 1 {
                        let mut new_states = Vec::new();
                        for s in &current_states {
                            for val in &possible_values {
                                let mut branched = s.clone();
                                if let Some(v) = branched.get_mut(&effect.field) {
                                    *v = val.clone();
                                }
                                new_states.push(branched);
                            }
                        }
                        current_states = new_states;
                        continue;
                    }
                }
                // Field not found or single-valued → keep current
            }
            _ => {
                // Deterministic effect: apply in-place to all branches
                for s in &mut current_states {
                    apply_single_effect(s, effect, fields);
                }
            }
        }
    }

    // Deduplicate
    current_states.sort();
    current_states.dedup();
    current_states
}

/// Apply a single deterministic effect to a state in-place.
fn apply_single_effect(state: &mut AbstractState, effect: &Effect, fields: &[FieldDomain]) {
    if let Some(val) = state.get_mut(&effect.field) {
        match &effect.effect {
            CallEffect::SetTrue => *val = AbstractValue::Bool(true),
            CallEffect::SetFalse => *val = AbstractValue::Bool(false),
            CallEffect::SetPresent => *val = AbstractValue::Present(true),
            CallEffect::SetAbsent => *val = AbstractValue::Present(false),
            CallEffect::IncrementCounter => {
                if let AbstractValue::Counter(n) = val {
                    // SOUNDNESS: over-approx above bound. Default 3 is a heuristic;
                    // user should set explicit bound in the extraction spec. States
                    // above the bound are collapsed to the saturated value, which is
                    // conservative for safety but may add spurious liveness paths.
                    let bound = fields
                        .iter()
                        .find(|f| f.name == effect.field)
                        .and_then(|f| f.bound)
                        .unwrap_or(crate::adapter::domain::DEFAULT_COUNTER_BOUND);
                    *val = AbstractValue::Counter((*n + 1).min(bound));
                }
            }
            CallEffect::DecrementCounter => {
                if let AbstractValue::Counter(n) = val {
                    *val = AbstractValue::Counter((*n - 1).max(0));
                }
            }
            CallEffect::ResetToZero => {
                if let AbstractValue::Counter(_) = val {
                    *val = AbstractValue::Counter(0);
                }
            }
            CallEffect::ReadOnly | CallEffect::None | CallEffect::Unknown => {}
        }
        if let Some(explicit) = &effect.value {
            *val = explicit.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::domain::AbstractionType;

    fn bool_field(name: &str, initial: bool) -> FieldDomain {
        FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::Boolean,
            bound: None,
            lower_bound: None,
            variants: None,
            initial: AbstractValue::Bool(initial),
        }
    }

    fn counter_field(name: &str, bound: i64) -> FieldDomain {
        FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::BoundedCounter,
            bound: Some(bound),
            lower_bound: None,
            variants: None,
            initial: AbstractValue::Counter(0),
        }
    }

    #[test]
    fn enumerate_boolean_cross_product() {
        let fields = [bool_field("a", false), bool_field("b", false)];
        let field_refs: Vec<&FieldDomain> = fields.iter().collect();
        let states = state_enum::enumerate_cross_product(&field_refs);
        assert_eq!(states.len(), 4); // 2 × 2
    }

    #[test]
    fn enumerate_mixed_domains() {
        let fields = [
            bool_field("flag", false),
            counter_field("count", 2), // 0, 1, 2 = 3 values
        ];
        let field_refs: Vec<&FieldDomain> = fields.iter().collect();
        let states = state_enum::enumerate_cross_product(&field_refs);
        assert_eq!(states.len(), 6); // 2 × 3
    }

    #[test]
    fn simple_transport_lifecycle() {
        // Model: _started (bool, init=false), _closed (bool, init=false)
        // Methods: start() [guard: !started, effect: started=true]
        //          close() [guard: !closed, effect: closed=true]
        let fields = vec![bool_field("_started", false), bool_field("_closed", false)];
        let methods = vec![
            MethodBehavior {
                name: "start".to_string(),
                guards: vec![Guard {
                    field: "_started".to_string(),
                    condition: CallGuard::MustBeFalse,
                }],
                effects: vec![Effect {
                    field: "_started".to_string(),
                    effect: CallEffect::SetTrue,
                    value: None,
                }],
                controllable: true,
                line_start: None,
                line_end: None,
            },
            MethodBehavior {
                name: "close".to_string(),
                guards: vec![Guard {
                    field: "_closed".to_string(),
                    condition: CallGuard::MustBeFalse,
                }],
                effects: vec![Effect {
                    field: "_closed".to_string(),
                    effect: CallEffect::SetTrue,
                    value: None,
                }],
                controllable: true,
                line_start: None,
                line_end: None,
            },
        ];

        let automaton =
            derive_automaton("Lifecycle", &fields, &methods, &HashMap::new(), "ev_", true);

        // 4 total states (2×2), but only 3 reachable from (F,F):
        // (F,F) → start → (T,F) → close → (T,T)
        // (F,F) → close → (F,T)
        // (F,T) is reachable via close from initial
        assert!(automaton.states.len() <= 4);
        assert!(automaton.states.iter().any(|s| s.is_initial));
        assert!(
            automaton
                .controllable_labels
                .contains(&"ev_start".to_string())
        );
        assert!(
            automaton
                .controllable_labels
                .contains(&"ev_close".to_string())
        );

        // Verify start transition exists from initial state
        let initial_name = automaton
            .states
            .iter()
            .find(|s| s.is_initial)
            .unwrap()
            .name
            .clone();
        let start_from_initial = automaton
            .transitions
            .iter()
            .any(|t| t.from == initial_name && t.label == "ev_start");
        assert!(start_from_initial);
    }

    #[test]
    fn counter_field_transitions() {
        let fields = vec![counter_field("map_size", 2)];
        let methods = vec![
            MethodBehavior {
                name: "add".to_string(),
                guards: vec![],
                effects: vec![Effect {
                    field: "map_size".to_string(),
                    effect: CallEffect::IncrementCounter,
                    value: None,
                }],
                controllable: false,
                line_start: None,
                line_end: None,
            },
            MethodBehavior {
                name: "clear".to_string(),
                guards: vec![],
                effects: vec![Effect {
                    field: "map_size".to_string(),
                    effect: CallEffect::ResetToZero,
                    value: None,
                }],
                controllable: true,
                line_start: None,
                line_end: None,
            },
        ];

        let automaton = derive_automaton(
            "MapTracker",
            &fields,
            &methods,
            &HashMap::new(),
            "ev_",
            false,
        );

        // 3 states: 0, 1, 2
        assert_eq!(automaton.states.len(), 3);

        // add from 0 → 1, from 1 → 2, from 2 → 2 (clamped at bound)
        let add_transitions: Vec<_> = automaton
            .transitions
            .iter()
            .filter(|t| t.label == "ev_add")
            .collect();
        assert_eq!(add_transitions.len(), 3);

        // clear from any → 0
        let clear_transitions: Vec<_> = automaton
            .transitions
            .iter()
            .filter(|t| t.label == "ev_clear")
            .collect();
        assert_eq!(clear_transitions.len(), 3);
    }

    #[test]
    fn unreachable_states_pruned() {
        // Field with 3 values but only 2 reachable
        let fields = vec![FieldDomain {
            name: "state".to_string(),
            abstraction: AbstractionType::EnumValues,
            bound: None,
            lower_bound: None,
            variants: Some(vec!["A".to_string(), "B".to_string(), "C".to_string()]),
            initial: AbstractValue::Variant("A".to_string()),
        }];
        // Only A→B transition exists; C is unreachable
        let methods = vec![MethodBehavior {
            name: "go".to_string(),
            guards: vec![Guard {
                field: "state".to_string(),
                condition: CallGuard::None, // applies in any state with Variant
            }],
            effects: vec![Effect {
                field: "state".to_string(),
                effect: CallEffect::None,
                value: Some(AbstractValue::Variant("B".to_string())),
            }],
            controllable: false,
            line_start: None,
            line_end: None,
        }];

        let automaton = derive_automaton("Test", &fields, &methods, &HashMap::new(), "ev_", false);

        // Only A and B should be reachable (C has no incoming transition from A or B)
        assert_eq!(automaton.states.len(), 2);
        let names: Vec<_> = automaton.states.iter().map(|s| s.name.as_str()).collect();
        assert!(names.iter().any(|n| n.contains("A")));
        assert!(names.iter().any(|n| n.contains("B")));
    }

    #[test]
    fn guard_blocks_transition() {
        let fields = vec![bool_field("locked", true)];
        let methods = vec![MethodBehavior {
            name: "unlock".to_string(),
            guards: vec![Guard {
                field: "locked".to_string(),
                condition: CallGuard::MustBeTrue,
            }],
            effects: vec![Effect {
                field: "locked".to_string(),
                effect: CallEffect::SetFalse,
                value: None,
            }],
            controllable: true,
            line_start: None,
            line_end: None,
        }];

        let automaton = derive_automaton("Lock", &fields, &methods, &HashMap::new(), "ev_", false);

        // unlock only fires from locked=true → locked=false
        let unlock_transitions: Vec<_> = automaton
            .transitions
            .iter()
            .filter(|t| t.label == "ev_unlock")
            .collect();
        assert_eq!(unlock_transitions.len(), 1);
        // from locked_T → locked_F
        assert!(unlock_transitions[0].from.contains("T"));
        assert!(unlock_transitions[0].to.contains("F"));
    }

    /// Tier A1 — pre-enumeration state-space hard limit.
    ///
    /// Twenty boolean fields → 2^20 = 1,048,576 states, well over the 2^18
    /// hard limit. The pre-check should return an empty automaton without
    /// allocating the full state vector.
    #[test]
    fn pre_enum_state_space_hard_limit_returns_empty() {
        let fields: Vec<FieldDomain> = (0..20)
            .map(|i| bool_field(&format!("f_{i}"), false))
            .collect();
        let automaton = derive_automaton("TooBig", &fields, &[], &HashMap::new(), "ev_", false);
        assert!(
            automaton.states.is_empty(),
            "expected empty automaton when state space exceeds hard limit, got {} states",
            automaton.states.len()
        );
        assert!(automaton.transitions.is_empty());
    }

    /// Tier A1 — pre-enumeration state-space warning threshold.
    ///
    /// 13 boolean fields = 2^13 = 8,192 states, over the 2^12 warn threshold
    /// but under the 2^18 hard limit. Should still produce a valid automaton
    /// but log via `tracing::warn!`.
    #[test]
    fn pre_enum_state_space_warn_threshold_still_succeeds() {
        let fields: Vec<FieldDomain> = (0..13)
            .map(|i| bool_field(&format!("f_{i}"), false))
            .collect();
        let automaton = derive_automaton("Big", &fields, &[], &HashMap::new(), "ev_", false);
        // No methods → only initial-state's noop self-loops would exist.
        // What matters here is that we got back a non-empty automaton (i.e.,
        // the warning didn't short-circuit) and the state count fits.
        // 2^13 with no transitions becomes 1 reachable state after pruning.
        assert!(!automaton.states.is_empty());
    }

    /// GAP-005 step 0 — degenerate-model warning fires on a 1-state / 0-transition
    /// derivation. This is the canonical "extractor only saw `self.*` and missed
    /// module-level state" outcome.
    #[test]
    fn degenerate_model_emits_warning() {
        // One bool field, no methods — produces a single reachable state with
        // no non-noop transitions.
        let fields = [bool_field("only_field", false)];
        let automaton = derive_automaton(
            "Degenerate",
            &fields,
            &[],
            &HashMap::new(),
            "ev_",
            false, // no noop self-loops, so transitions count is genuinely 0
        );
        assert_eq!(automaton.states.len(), 1, "expected 1 reachable state");
        assert_eq!(
            automaton.transitions.len(),
            0,
            "expected 0 transitions for a degenerate model"
        );
        assert_eq!(
            automaton.warnings.len(),
            1,
            "expected exactly one degenerate-model warning"
        );
        assert!(
            automaton.warnings[0].contains("degenerate"),
            "warning should mention 'degenerate', got: {}",
            automaton.warnings[0]
        );
        assert!(
            automaton.warnings[0].contains("Degenerate"),
            "warning should reference the automaton name 'Degenerate', got: {}",
            automaton.warnings[0]
        );
    }

    /// GAP-005 step 0 — non-degenerate models produce no degenerate warning.
    /// Regression guard: ensure the warning doesn't fire on healthy automata.
    #[test]
    fn non_degenerate_emits_no_warning() {
        let fields = [bool_field("started", false), bool_field("closed", false)];
        let methods = [
            MethodBehavior {
                name: "start".to_string(),
                guards: vec![Guard {
                    field: "started".to_string(),
                    condition: CallGuard::MustBeFalse,
                }],
                effects: vec![Effect {
                    field: "started".to_string(),
                    effect: CallEffect::SetTrue,
                    value: None,
                }],
                controllable: true,
                line_start: None,
                line_end: None,
            },
            MethodBehavior {
                name: "close".to_string(),
                guards: vec![Guard {
                    field: "closed".to_string(),
                    condition: CallGuard::MustBeFalse,
                }],
                effects: vec![Effect {
                    field: "closed".to_string(),
                    effect: CallEffect::SetTrue,
                    value: None,
                }],
                controllable: true,
                line_start: None,
                line_end: None,
            },
        ];
        let automaton = derive_automaton(
            "Lifecycle",
            &fields,
            &methods,
            &HashMap::new(),
            "ev_",
            false,
        );
        assert!(
            automaton.states.len() > 1,
            "expected multi-state automaton, got {}",
            automaton.states.len()
        );
        assert!(
            !automaton.transitions.is_empty(),
            "expected at least one transition"
        );
        assert!(
            automaton.warnings.is_empty(),
            "expected no warnings for non-degenerate automaton, got: {:?}",
            automaton.warnings
        );
    }

    /// GAP-005 step 0 — noop self-loops don't suppress the degenerate warning.
    /// A 1-state automaton with only noop self-loops is still degenerate from
    /// the user's perspective (no real behavior captured); the count of
    /// non-noop transitions is what matters.
    #[test]
    fn degenerate_with_noop_self_loops_still_warns() {
        let fields = [bool_field("only_field", false)];
        let automaton = derive_automaton(
            "DegenerateWithNoops",
            &fields,
            &[],
            &HashMap::new(),
            "ev_",
            true, // noop self-loops enabled
        );
        // Noop self-loop adds 1 transition per state, but they shouldn't
        // mask the degenerate condition.
        assert!(
            !automaton.warnings.is_empty(),
            "noop-only automaton should still trigger the degenerate warning"
        );
        assert!(automaton.warnings[0].contains("degenerate"));
    }

    /// GAP-005b — method-name-labeled self-loops don't mask the degenerate
    /// warning. Pre-fix bug (surfaced by MCP-004 + MCP-005 re-validation):
    /// the warning's trigger condition counted any non-`noop` transition,
    /// so a 1-state automaton with `s0 --ev_close--> s0` self-loops looked
    /// like "8 non-noop transitions" and the warning silently dropped.
    /// Fix: count transitions that go from a source state to a *different*
    /// target state (i.e., transitions that actually mutate state); self-
    /// loops contribute no real behavior regardless of label.
    #[test]
    fn degenerate_with_method_labeled_self_loops_still_warns() {
        let fields = [bool_field("only_field", false)];
        // A method whose effect is "set the only_field to false" — but
        // it's already false, so the effect is a self-loop (no state
        // change). Combined with `add_noop=false`, the only transitions
        // produced will be method-name-labeled self-loops.
        let methods = [MethodBehavior {
            name: "close".to_string(),
            guards: vec![Guard {
                field: "only_field".to_string(),
                condition: CallGuard::MustBeFalse,
            }],
            effects: vec![Effect {
                field: "only_field".to_string(),
                effect: CallEffect::SetFalse,
                value: None,
            }],
            controllable: true,
            line_start: None,
            line_end: None,
        }];
        let automaton = derive_automaton(
            "DegenerateWithMethodSelfLoops",
            &fields,
            &methods,
            &HashMap::new(),
            "ev_",
            false, // no noop layer — only the method-labeled self-loops
        );
        assert_eq!(
            automaton.states.len(),
            1,
            "expected 1 reachable state for this fixture"
        );
        // The method should have produced at least one transition (a
        // self-loop labeled `ev_close`), so this is the regression case.
        assert!(
            automaton.transitions.iter().any(|t| t.label == "ev_close"),
            "expected an ev_close transition (self-loop)"
        );
        assert!(
            !automaton.warnings.is_empty(),
            "warning should fire even though there's a method-labeled \
             self-loop — no transition mutates state, so this is degenerate"
        );
        assert!(automaton.warnings[0].contains("degenerate"));
    }
}
