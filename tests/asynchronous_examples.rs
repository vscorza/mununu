use mununu::clts::{Clts, DefaultLabelIdx, DefaultStateIdx};
use mununu::examples::asynchronous;

fn skip_in_ci(test_name: &str) -> bool {
    if std::env::var("CI").is_ok() {
        eprintln!("skipping slow test `{}` on CI", test_name);
        true
    } else {
        false
    }
}

fn single_transition_labels(
    clts: &Clts<DefaultStateIdx, DefaultLabelIdx>,
    state: &str,
) -> Vec<Vec<String>> {
    let id = clts.state_id(state).unwrap();
    clts.outgoing(id)
        .iter()
        .map(|t| {
            let mut labels: Vec<_> = t
                .labels()
                .iter()
                .flat_map(|id| clts.label_payload(*id).unwrap().iter().cloned())
                .collect();
            labels.sort();
            labels
        })
        .collect()
}

#[test]
fn producer_consumer_has_independent_actions() {
    if skip_in_ci("producer_consumer_has_independent_actions") {
        return;
    }
    let clts = asynchronous::producer_consumer_buffer();
    let empty = single_transition_labels(&clts, "empty");
    let full = single_transition_labels(&clts, "full");
    assert_eq!(empty, vec![vec!["produce".to_string()]]);
    assert_eq!(full, vec![vec!["consume".to_string()]]);
}

#[test]
fn two_phase_handshake_exposes_all_edges() {
    if skip_in_ci("two_phase_handshake_exposes_all_edges") {
        return;
    }
    let clts = asynchronous::two_phase_handshake();
    let idle = single_transition_labels(&clts, "idle");
    assert!(
        idle.iter()
            .any(|labels| labels == &vec!["req+".to_string()])
    );
    let req_high = single_transition_labels(&clts, "req_high");
    assert!(
        req_high
            .iter()
            .any(|labels| labels == &vec!["ack+".to_string()])
    );
    let returning = single_transition_labels(&clts, "returning");
    assert!(
        returning
            .iter()
            .any(|labels| labels == &vec!["ack-".to_string()])
    );
}

#[test]
fn peterson_models_critical_release() {
    if skip_in_ci("peterson_models_critical_release") {
        return;
    }
    let clts = asynchronous::peterson_mutual_exclusion();
    let crit = single_transition_labels(&clts, "critical");
    assert_eq!(crit, vec![vec!["release".to_string()]]);
}

#[test]
fn token_ring_passes_token() {
    if skip_in_ci("token_ring_passes_token") {
        return;
    }
    let clts = asynchronous::token_ring_three();
    let node0 = single_transition_labels(&clts, "node0");
    assert_eq!(node0, vec![vec!["pass_0_1".to_string()]]);
    let node2 = single_transition_labels(&clts, "node2");
    assert_eq!(node2, vec![vec!["pass_2_0".to_string()]]);
}

#[test]
fn bounded_buffer_marks_overflow() {
    if skip_in_ci("bounded_buffer_marks_overflow") {
        return;
    }
    let clts = asynchronous::bounded_buffer_overflow();
    let overflow = clts.state_id("overflow").unwrap();
    assert!(
        clts.state_variables(overflow)
            .contains(&"data_lost".to_string())
    );
    let overflow_transitions = clts.outgoing(overflow);
    assert_eq!(overflow_transitions.len(), 1);
    let labels: Vec<_> = overflow_transitions[0]
        .labels()
        .iter()
        .flat_map(|id| clts.label_payload(*id).unwrap().iter().cloned())
        .collect();
    assert_eq!(labels, vec!["drop".to_string()]);
}
