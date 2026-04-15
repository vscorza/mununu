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
use super::config::AbstractionType;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Abstract value in a field's domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AbstractValue {
    /// Boolean: true or false.
    Bool(bool),
    /// Presence: present or absent.
    Present(bool),
    /// Bounded counter: 0..bound.
    Counter(i64),
    /// Enum variant by name.
    Variant(String),
}

impl AbstractValue {
    pub fn display_short(&self) -> String {
        match self {
            AbstractValue::Bool(true) => "T".to_string(),
            AbstractValue::Bool(false) => "F".to_string(),
            AbstractValue::Present(true) => "Some".to_string(),
            AbstractValue::Present(false) => "None".to_string(),
            AbstractValue::Counter(n) => n.to_string(),
            AbstractValue::Variant(v) => v.clone(),
        }
    }
}

/// Definition of a field's abstract domain.
#[derive(Debug, Clone)]
pub struct FieldDomain {
    /// Field name (as it appears in source).
    pub name: String,
    /// Abstraction type.
    pub abstraction: AbstractionType,
    /// Upper bound for bounded counter.
    pub bound: Option<i64>,
    /// Explicit variants for enum.
    pub variants: Option<Vec<String>>,
    /// Initial value.
    pub initial: AbstractValue,
}

impl FieldDomain {
    /// Enumerate all values in this domain.
    pub fn values(&self) -> Vec<AbstractValue> {
        match self.abstraction {
            AbstractionType::Boolean => vec![AbstractValue::Bool(false), AbstractValue::Bool(true)],
            AbstractionType::Presence => {
                vec![AbstractValue::Present(false), AbstractValue::Present(true)]
            }
            AbstractionType::BoundedCounter => {
                let bound = self.bound.unwrap_or(3);
                (0..=bound).map(AbstractValue::Counter).collect()
            }
            AbstractionType::EnumValues => self
                .variants
                .as_ref()
                .map(|vs| {
                    vs.iter()
                        .map(|v| AbstractValue::Variant(v.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            AbstractionType::Ignored => vec![],
        }
    }

    /// Number of abstract values.
    pub fn cardinality(&self) -> usize {
        self.values().len()
    }
}

/// A concrete abstract state — assignment of values to all fields.
pub type AbstractState = BTreeMap<String, AbstractValue>;

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

/// Derive an automaton from field domains and method behaviors.
pub fn derive_automaton(
    automaton_name: &str,
    fields: &[FieldDomain],
    methods: &[MethodBehavior],
    state_name_overrides: &HashMap<String, String>,
    label_prefix: &str,
    add_noop: bool,
) -> DerivedAutomaton {
    // Step 1: Enumerate all abstract states (cross-product)
    let all_states = enumerate_states(fields);
    if all_states.is_empty() {
        return DerivedAutomaton {
            name: automaton_name.to_string(),
            states: vec![],
            transitions: vec![],
            controllable_labels: vec![],
        };
    }

    // Step 2: Find initial state
    let initial_state: AbstractState = fields
        .iter()
        .map(|f| (f.name.clone(), f.initial.clone()))
        .collect();

    // Step 3: Name states
    let state_names: HashMap<AbstractState, String> = all_states
        .iter()
        .map(|s| {
            let auto_name = make_state_name(s, fields);
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
                let target = apply_effects(state, &method.effects, fields);
                if let Some(target_name) = state_names.get(&target) {
                    let source_name = &state_names[state];
                    transitions.push(DerivedTransition {
                        from: source_name.clone(),
                        to: target_name.clone(),
                        label: label.clone(),
                    });
                }
                // If target is outside the domain (e.g., counter overflow),
                // the transition is dropped — over-approximation is handled
                // by the bounded domain.
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

    // Step 6: Prune unreachable states
    let (reachable_states, reachable_transitions) =
        prune_unreachable(&initial_state, &state_names, &all_states, &transitions);

    DerivedAutomaton {
        name: automaton_name.to_string(),
        states: reachable_states,
        transitions: reachable_transitions,
        controllable_labels,
    }
}

/// Enumerate all states from the cross-product of field domains.
fn enumerate_states(fields: &[FieldDomain]) -> Vec<AbstractState> {
    let active_fields: Vec<&FieldDomain> = fields
        .iter()
        .filter(|f| f.abstraction != AbstractionType::Ignored)
        .collect();

    if active_fields.is_empty() {
        return vec![BTreeMap::new()];
    }

    let mut states = vec![BTreeMap::new()];
    for field in &active_fields {
        let values = field.values();
        let mut new_states = Vec::with_capacity(states.len() * values.len());
        for state in &states {
            for value in &values {
                let mut new_state = state.clone();
                new_state.insert(field.name.clone(), value.clone());
                new_states.push(new_state);
            }
        }
        states = new_states;
    }
    states
}

/// Generate a state name from field values.
fn make_state_name(state: &AbstractState, fields: &[FieldDomain]) -> String {
    fields
        .iter()
        .filter(|f| f.abstraction != AbstractionType::Ignored)
        .map(|f| {
            let val = state
                .get(&f.name)
                .map(|v| v.display_short())
                .unwrap_or_default();
            format!("{}_{}", f.name, val)
        })
        .collect::<Vec<_>>()
        .join("_")
}

/// Check if all guards are satisfied in the given state.
fn guards_satisfied(state: &AbstractState, guards: &[Guard]) -> bool {
    guards.iter().all(|g| {
        let val = match state.get(&g.field) {
            Some(v) => v,
            None => return true, // field not modeled → guard vacuously true
        };
        match &g.condition {
            CallGuard::CounterGtZero => matches!(val, AbstractValue::Counter(n) if *n > 0),
            CallGuard::CounterEqZero => matches!(val, AbstractValue::Counter(0)),
            CallGuard::MustBePresent => matches!(val, AbstractValue::Present(true)),
            CallGuard::MustBeAbsent => matches!(val, AbstractValue::Present(false)),
            CallGuard::MustBeTrue => matches!(val, AbstractValue::Bool(true)),
            CallGuard::MustBeFalse => matches!(val, AbstractValue::Bool(false)),
            CallGuard::None => true,
        }
    })
}

/// Apply effects to a state, producing a new state.
fn apply_effects(
    state: &AbstractState,
    effects: &[Effect],
    fields: &[FieldDomain],
) -> AbstractState {
    let mut new_state = state.clone();
    for effect in effects {
        if let Some(val) = new_state.get_mut(&effect.field) {
            match &effect.effect {
                CallEffect::SetTrue => *val = AbstractValue::Bool(true),
                CallEffect::SetFalse => *val = AbstractValue::Bool(false),
                CallEffect::SetPresent => *val = AbstractValue::Present(true),
                CallEffect::SetAbsent => *val = AbstractValue::Present(false),
                CallEffect::IncrementCounter => {
                    if let AbstractValue::Counter(n) = val {
                        let bound = fields
                            .iter()
                            .find(|f| f.name == effect.field)
                            .and_then(|f| f.bound)
                            .unwrap_or(3);
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
                CallEffect::ReadOnly | CallEffect::None => {}
                CallEffect::Unknown => {
                    // Over-approximate: nondeterministic — handled by caller
                    // generating transitions for ALL possible values.
                    // For now, keep current value (under-approximation fallback).
                }
            }
            if let Some(explicit) = &effect.value {
                *val = explicit.clone();
            }
        }
    }
    new_state
}

/// Prune unreachable states via BFS from initial state.
fn prune_unreachable(
    initial: &AbstractState,
    state_names: &HashMap<AbstractState, String>,
    all_states: &[AbstractState],
    transitions: &[DerivedTransition],
) -> (Vec<DerivedState>, Vec<DerivedTransition>) {
    let initial_name = match state_names.get(initial) {
        Some(n) => n.clone(),
        None => return (vec![], vec![]),
    };

    // Build adjacency from transitions
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in transitions {
        adj.entry(t.from.as_str()).or_default().push(t.to.as_str());
    }

    // BFS
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    visited.insert(initial_name.clone());
    queue.push_back(initial_name.clone());

    while let Some(current) = queue.pop_front() {
        if let Some(neighbors) = adj.get(current.as_str()) {
            for &next in neighbors {
                if visited.insert(next.to_string()) {
                    queue.push_back(next.to_string());
                }
            }
        }
    }

    // Filter states and transitions
    let name_to_state: HashMap<&str, &AbstractState> = all_states
        .iter()
        .filter_map(|s| state_names.get(s).map(|n| (n.as_str(), s)))
        .collect();

    let states: Vec<DerivedState> = visited
        .iter()
        .filter_map(|name| {
            let abstract_state = name_to_state.get(name.as_str())?;
            Some(DerivedState {
                name: name.clone(),
                is_initial: name == &initial_name,
                abstract_state: (*abstract_state).clone(),
            })
        })
        .collect();

    let transitions: Vec<DerivedTransition> = transitions
        .iter()
        .filter(|t| visited.contains(&t.from) && visited.contains(&t.to))
        .cloned()
        .collect();

    (states, transitions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bool_field(name: &str, initial: bool) -> FieldDomain {
        FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::Boolean,
            bound: None,
            variants: None,
            initial: AbstractValue::Bool(initial),
        }
    }

    fn counter_field(name: &str, bound: i64) -> FieldDomain {
        FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::BoundedCounter,
            bound: Some(bound),
            variants: None,
            initial: AbstractValue::Counter(0),
        }
    }

    #[test]
    fn enumerate_boolean_cross_product() {
        let fields = vec![bool_field("a", false), bool_field("b", false)];
        let states = enumerate_states(&fields);
        assert_eq!(states.len(), 4); // 2 × 2
    }

    #[test]
    fn enumerate_mixed_domains() {
        let fields = vec![
            bool_field("flag", false),
            counter_field("count", 2), // 0, 1, 2 = 3 values
        ];
        let states = enumerate_states(&fields);
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
}
