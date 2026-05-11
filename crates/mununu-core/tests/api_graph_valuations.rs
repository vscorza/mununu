use mununu_core::api::graph::generate_graphs;
use mununu_core::api::models::{GraphElementData, GraphType};
use mununu_core::context_dsl;
use mununu_core::context_dsl::realize::realize;
use std::collections::{BTreeMap, HashMap};

const TRAFFIC_CTXDSL: &str = r#"
context traffic {
    alphabet { label tick; label expire; label sense; }
    automata {
        automaton TrafficLight {
            controllable { label expire; }
            states {
                state Green initial;
                state Yellow;
                state Red;
                state RedWait;
            }
            transitions {
                transition Green -> Green on label tick;
                transition Green -> Yellow on label expire;
                transition Yellow -> Yellow on label tick;
                transition Yellow -> Red on label expire;
                transition Red -> Red on label tick;
                transition Red -> RedWait on label expire;
                transition RedWait -> Green on label sense;
                transition RedWait -> RedWait on label tick;
            }
        }
    }
}
"#;

fn pair(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

fn assert_state_has_valuation(
    elements: &[mununu_core::api::models::GraphElement],
    state_label: &str,
    expected: &[(&str, &str)],
) {
    let node = elements
        .iter()
        .find(|el| match &el.data {
            GraphElementData::Node { label, .. } => label == state_label,
            _ => false,
        })
        .unwrap_or_else(|| panic!("state node `{}` not found", state_label));

    let valuations = match &node.data {
        GraphElementData::Node { valuations, .. } => valuations.as_ref().unwrap_or_else(|| {
            panic!(
                "state `{}` is missing the `valuations` payload",
                state_label
            )
        }),
        _ => unreachable!(),
    };

    for (k, v) in expected {
        assert_eq!(
            valuations.get(*k).map(String::as_str),
            Some(*v),
            "state `{}` valuation `{}` mismatch (full map: {:?})",
            state_label,
            k,
            valuations
        );
    }
}

#[test]
fn dsl_graph_includes_state_valuations_from_side_channel() {
    let mut doc = context_dsl::parse(TRAFFIC_CTXDSL).expect("CTXDSL parses");

    // Inject side-channel valuations as an adapter would (e.g., the BTOR2 reader
    // populates `ContextDoc.state_valuations` from the cross-product enumeration
    // of register values). The realize layer then registers them on the CLTS via
    // `Clts::with_valuation_for_state`.
    let mut per_state: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    per_state.insert(
        "Green".to_string(),
        [
            pair("is_red", "0"),
            pair("is_green", "1"),
            pair("is_yellow", "0"),
            pair("phase", "green"),
        ]
        .into_iter()
        .collect(),
    );
    per_state.insert(
        "Yellow".to_string(),
        [pair("is_yellow", "1"), pair("phase", "yellow")]
            .into_iter()
            .collect(),
    );
    per_state.insert(
        "Red".to_string(),
        [pair("is_red", "1"), pair("phase", "red")]
            .into_iter()
            .collect(),
    );
    per_state.insert(
        "RedWait".to_string(),
        [pair("is_red", "1"), pair("phase", "red_wait")]
            .into_iter()
            .collect(),
    );
    doc.state_valuations
        .insert("TrafficLight".to_string(), per_state);

    let realized = realize(&doc, &[]).expect("realization succeeds");

    let (graphs, _summary) = generate_graphs(
        &doc,
        &[],
        &realized,
        Some("TrafficLight"),
        &[GraphType::Dsl],
    )
    .expect("graph generation succeeds");

    assert_eq!(graphs.len(), 1, "exactly one graph for the DSL view");
    let elements = &graphs[0].elements;

    assert_state_has_valuation(
        elements,
        "Green",
        &[
            ("is_red", "0"),
            ("is_green", "1"),
            ("is_yellow", "0"),
            ("phase", "green"),
        ],
    );
    assert_state_has_valuation(
        elements,
        "Yellow",
        &[("is_yellow", "1"), ("phase", "yellow")],
    );
    assert_state_has_valuation(elements, "Red", &[("is_red", "1"), ("phase", "red")]);
    assert_state_has_valuation(
        elements,
        "RedWait",
        &[("is_red", "1"), ("phase", "red_wait")],
    );
}

#[test]
fn graph_omits_valuations_when_state_has_none() {
    // No `state_valuations` injection. Each state node should serialize without
    // the `valuations` field (Option::None → skipped by serde).
    const SRC: &str = r#"
context demo {
    alphabet { label a; }
    automata {
        automaton A {
            states { state s0 initial; state s1; }
            transitions { transition s0 -> s1 on label a; }
        }
    }
}
"#;
    let doc = context_dsl::parse(SRC).expect("context parses");
    let realized = realize(&doc, &[]).expect("realization succeeds");

    let (graphs, _summary) =
        generate_graphs(&doc, &[], &realized, Some("A"), &[GraphType::Dsl]).expect("graph ok");
    let elements = &graphs[0].elements;

    for el in elements {
        if let GraphElementData::Node {
            label, valuations, ..
        } = &el.data
            && (label == "s0" || label == "s1")
        {
            assert!(
                valuations.is_none(),
                "state `{}` should have no valuations, got {:?}",
                label,
                valuations
            );
        }
    }
}
