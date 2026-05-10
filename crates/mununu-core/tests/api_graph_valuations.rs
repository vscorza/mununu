use mununu_core::api::graph::generate_graphs;
use mununu_core::api::models::{GraphElementData, GraphType};
use mununu_core::context_dsl;
use mununu_core::context_dsl::realize::realize;

const TRAFFIC_LIGHT_VALUATIONS: &str =
    include_str!("../../../examples/hw/traffic_light_valuations.ctxdsl");

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
fn dsl_graph_includes_state_valuations() {
    let doc = context_dsl::parse(TRAFFIC_LIGHT_VALUATIONS).expect("CTXDSL parses");
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
    // A minimal context with no `valuations { ... }` blocks. Each state node
    // should serialize without the `valuations` field (Option::None).
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
