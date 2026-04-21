//! Shared state enumeration utilities for adapters.
//!
//! Provides cross-product state enumeration, state naming, and BFS
//! reachability pruning — algorithms used by the SystemVerilog Kripke
//! builder, extraction state space deriver, and Promela variable automata.

use crate::adapter::domain::{AbstractState, AbstractionType, FieldDomain};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Enumerate all abstract states from the cross-product of field domains.
///
/// Fields with `AbstractionType::Ignored` are excluded. If no active fields
/// remain, returns a single empty state (representing a trivial 1-state space).
pub fn enumerate_cross_product(fields: &[&FieldDomain]) -> Vec<AbstractState> {
    let active_fields: Vec<&&FieldDomain> = fields
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

/// Generate a state name from field values (e.g., `flag_T_count_2`).
///
/// Fields with `AbstractionType::Ignored` are excluded. Returns `"s0"` if
/// the state is empty (trivial 1-state space).
pub fn make_state_name(state: &AbstractState) -> String {
    if state.is_empty() {
        return "s0".to_string();
    }
    state
        .iter()
        .map(|(k, v)| format!("{}_{}", k, v.display_short()))
        .collect::<Vec<_>>()
        .join("_")
}

/// Generate a state name using field ordering from a `FieldDomain` slice.
///
/// Like [`make_state_name`] but preserves the field order from `fields`
/// rather than relying on `BTreeMap` key order (which is alphabetical).
/// Fields with `AbstractionType::Ignored` are excluded.
pub fn make_state_name_ordered(state: &AbstractState, fields: &[FieldDomain]) -> String {
    let parts: Vec<String> = fields
        .iter()
        .filter(|f| f.abstraction != AbstractionType::Ignored)
        .filter_map(|f| {
            state
                .get(&f.name)
                .map(|v| format!("{}_{}", f.name, v.display_short()))
        })
        .collect();

    if parts.is_empty() {
        "s0".to_string()
    } else {
        parts.join("_")
    }
}

/// Compute the set of reachable state names via BFS from an initial state.
///
/// Builds an adjacency list from `(source, target)` pairs and performs
/// breadth-first traversal from `initial`. Returns the set of reachable
/// state names (including `initial`).
pub fn bfs_reachable<S, T>(initial: &str, edges: &[(S, T)]) -> HashSet<String>
where
    S: AsRef<str>,
    T: AsRef<str>,
{
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for (src, tgt) in edges {
        adj.entry(src.as_ref()).or_default().push(tgt.as_ref());
    }

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(initial.to_string());
    queue.push_back(initial.to_string());

    while let Some(current) = queue.pop_front() {
        if let Some(neighbors) = adj.get(current.as_str()) {
            for &next in neighbors {
                if visited.insert(next.to_string()) {
                    queue.push_back(next.to_string());
                }
            }
        }
    }

    visited
}

/// Build the initial abstract state from field domains (using each field's `initial` value).
pub fn initial_state_from_fields(fields: &[FieldDomain]) -> AbstractState {
    fields
        .iter()
        .filter(|f| f.abstraction != AbstractionType::Ignored)
        .map(|f| (f.name.clone(), f.initial.clone()))
        .collect()
}

/// Compute total state space cardinality from field domains.
///
/// Returns `None` if the product overflows `usize`.
pub fn state_space_size(fields: &[&FieldDomain]) -> Option<usize> {
    let active: Vec<usize> = fields
        .iter()
        .filter(|f| f.abstraction != AbstractionType::Ignored)
        .map(|f| f.cardinality())
        .collect();

    if active.is_empty() {
        return Some(1);
    }

    active.iter().try_fold(1usize, |acc, &c| acc.checked_mul(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::domain::{AbstractValue, AbstractionType};

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

    fn ignored_field(name: &str) -> FieldDomain {
        FieldDomain {
            name: name.to_string(),
            abstraction: AbstractionType::Ignored,
            bound: None,
            lower_bound: None,
            variants: None,
            initial: AbstractValue::Counter(0),
        }
    }

    #[test]
    fn cross_product_booleans() {
        let a = bool_field("a", false);
        let b = bool_field("b", false);
        let fields: Vec<&FieldDomain> = vec![&a, &b];
        let states = enumerate_cross_product(&fields);
        assert_eq!(states.len(), 4); // 2 x 2
    }

    #[test]
    fn cross_product_mixed() {
        let flag = bool_field("flag", false);
        let count = counter_field("count", 2); // 0, 1, 2 = 3 values
        let fields: Vec<&FieldDomain> = vec![&flag, &count];
        let states = enumerate_cross_product(&fields);
        assert_eq!(states.len(), 6); // 2 x 3
    }

    #[test]
    fn cross_product_ignores_ignored() {
        let a = bool_field("a", false);
        let ignored = ignored_field("b");
        let fields: Vec<&FieldDomain> = vec![&a, &ignored];
        let states = enumerate_cross_product(&fields);
        assert_eq!(states.len(), 2); // only "a" contributes
    }

    #[test]
    fn cross_product_empty_fields() {
        let fields: Vec<&FieldDomain> = vec![];
        let states = enumerate_cross_product(&fields);
        assert_eq!(states.len(), 1);
        assert!(states[0].is_empty());
    }

    #[test]
    fn state_name_generation() {
        let mut state = BTreeMap::new();
        state.insert("flag".to_string(), AbstractValue::Bool(true));
        state.insert("count".to_string(), AbstractValue::Counter(2));
        let name = make_state_name(&state);
        assert_eq!(name, "count_2_flag_T"); // BTreeMap sorts alphabetically
    }

    #[test]
    fn state_name_empty() {
        let state = BTreeMap::new();
        assert_eq!(make_state_name(&state), "s0");
    }

    #[test]
    fn bfs_simple_chain() {
        let edges = vec![
            ("A".to_string(), "B".to_string()),
            ("B".to_string(), "C".to_string()),
            ("D".to_string(), "E".to_string()), // unreachable from A
        ];
        let reachable = bfs_reachable("A", &edges);
        assert!(reachable.contains("A"));
        assert!(reachable.contains("B"));
        assert!(reachable.contains("C"));
        assert!(!reachable.contains("D"));
        assert!(!reachable.contains("E"));
    }

    #[test]
    fn bfs_with_cycle() {
        let edges = vec![
            ("A".to_string(), "B".to_string()),
            ("B".to_string(), "A".to_string()),
            ("B".to_string(), "C".to_string()),
        ];
        let reachable = bfs_reachable("A", &edges);
        assert_eq!(reachable.len(), 3);
    }

    #[test]
    fn initial_state_from_fields_basic() {
        let fields = vec![bool_field("x", true), counter_field("y", 5)];
        let init = initial_state_from_fields(&fields);
        assert_eq!(init.get("x"), Some(&AbstractValue::Bool(true)));
        assert_eq!(init.get("y"), Some(&AbstractValue::Counter(0)));
    }

    #[test]
    fn state_space_size_basic() {
        let a = bool_field("a", false);
        let b = counter_field("b", 3); // 4 values: 0,1,2,3
        let fields: Vec<&FieldDomain> = vec![&a, &b];
        assert_eq!(state_space_size(&fields), Some(8)); // 2 x 4
    }

    #[test]
    fn state_space_size_with_ignored() {
        let a = bool_field("a", false);
        let ignored = ignored_field("b");
        let fields: Vec<&FieldDomain> = vec![&a, &ignored];
        assert_eq!(state_space_size(&fields), Some(2));
    }
}
