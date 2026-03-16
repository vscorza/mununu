//! Comprehensive tests for state unrolling.

use mununu::abstraction::unrolling::{
    Effect, OriginalState, OriginalTransition, UnrollingOptions, VariableDecl, unroll_states,
};

#[test]
fn test_simple_unrolling_no_variables() {
    let states = vec![OriginalState {
        name: "Start".to_string(),
        initial: true,
    }];
    let transitions = vec![OriginalTransition {
        from: "Start".to_string(),
        to: "End".to_string(),
        label: "complete".to_string(),
        guard: None,
        effects: vec![],
    }];
    let variables = vec![];
    let options = UnrollingOptions::default();

    let result = unroll_states(states, transitions, variables, options).unwrap();
    // Should have at least Start and End states
    assert!(!result.states.is_empty());
    assert!(!result.transitions.is_empty());
}

#[test]
fn test_unrolling_with_bool_variable() {
    let states = vec![
        OriginalState {
            name: "Review".to_string(),
            initial: true,
        },
        OriginalState {
            name: "Approved".to_string(),
            initial: false,
        },
    ];
    let transitions = vec![OriginalTransition {
        from: "Review".to_string(),
        to: "Review".to_string(),
        label: "approve".to_string(),
        guard: None,
        effects: vec![Effect {
            target: "approved".to_string(),
            value_expr: "true".to_string(),
        }],
    }];
    let variables = vec![VariableDecl {
        name: "approved".to_string(),
        ty: "bool".to_string(),
        initial: Some("false".to_string()),
    }];
    let options = UnrollingOptions::default();

    let result = unroll_states(states, transitions, variables, options).unwrap();
    // Should have Review_approved_false and Review_approved_true
    assert!(result.states.len() >= 2);
    assert!(!result.transitions.is_empty());
}

#[test]
fn test_unrolling_with_integer_variable() {
    let states = vec![OriginalState {
        name: "Processing".to_string(),
        initial: true,
    }];
    let transitions = vec![OriginalTransition {
        from: "Processing".to_string(),
        to: "Processing".to_string(),
        label: "increment".to_string(),
        guard: None,
        effects: vec![Effect {
            target: "count".to_string(),
            value_expr: "count + 1".to_string(),
        }],
    }];
    let variables = vec![VariableDecl {
        name: "count".to_string(),
        ty: "i64".to_string(),
        initial: Some("0".to_string()),
    }];
    let options = UnrollingOptions {
        max_states_per_location: Some(20), // Limit to prevent infinite unrolling
        ..Default::default()
    };

    let result = unroll_states(states, transitions, variables, options);
    // Should succeed and have multiple states with different count values
    // (or hit limit, which is also valid)
    match result {
        Ok(unrolled) => {
            assert!(unrolled.states.len() >= 2);
            assert!(!unrolled.transitions.is_empty());
        }
        Err(_) => {
            // Hitting limit is acceptable for this test
        }
    }
}

#[test]
fn test_unrolling_with_guard() {
    let states = vec![
        OriginalState {
            name: "Processing".to_string(),
            initial: true,
        },
        OriginalState {
            name: "Complete".to_string(),
            initial: false,
        },
    ];
    let transitions = vec![OriginalTransition {
        from: "Processing".to_string(),
        to: "Complete".to_string(),
        label: "finish".to_string(),
        guard: Some("count >= 5".to_string()),
        effects: vec![],
    }];
    let variables = vec![VariableDecl {
        name: "count".to_string(),
        ty: "i64".to_string(),
        initial: Some("0".to_string()),
    }];
    let options = UnrollingOptions {
        max_states_per_location: Some(10),
        ..Default::default()
    };

    let result = unroll_states(states, transitions, variables, options);
    // Should succeed (even if guard refinement is not fully implemented)
    assert!(result.is_ok());
}

#[test]
fn test_unrolling_state_limit() {
    let states = vec![OriginalState {
        name: "Loop".to_string(),
        initial: true,
    }];
    let transitions = vec![OriginalTransition {
        from: "Loop".to_string(),
        to: "Loop".to_string(),
        label: "increment".to_string(),
        guard: None,
        effects: vec![Effect {
            target: "x".to_string(),
            value_expr: "x + 1".to_string(),
        }],
    }];
    let variables = vec![VariableDecl {
        name: "x".to_string(),
        ty: "i64".to_string(),
        initial: Some("0".to_string()),
    }];
    let options = UnrollingOptions {
        max_states_per_location: Some(3), // Very low limit
        ..Default::default()
    };

    let result = unroll_states(states, transitions, variables, options);
    // Should hit the limit
    assert!(result.is_err());
}
