#!/usr/bin/env python3
"""langgraph_to_xstate.py — Convert LangGraph StateGraph to XState JSON with Mununu annotations.

Converts a compiled LangGraph graph into XState v5 JSON that can be imported
by Mununu's XState adapter for formal verification and controller synthesis.

Usage with LangGraph:

    from langgraph.graph import StateGraph, START, END
    from langgraph_to_xstate import langgraph_to_xstate
    import json

    builder = StateGraph(MyState)
    builder.add_node("router", router_fn)
    builder.add_node("billing", billing_fn)
    builder.add_node("tech", tech_fn)
    builder.add_edge(START, "router")
    builder.add_conditional_edges("router", route_fn, {"billing": "billing", "tech": "tech"})
    builder.add_edge("billing", END)
    builder.add_edge("tech", END)
    graph = builder.compile()

    xstate_json = langgraph_to_xstate(graph)
    print(json.dumps(xstate_json, indent=2))

Standalone usage (from dict representation):

    python3 langgraph_to_xstate.py --input graph.json --output workflow.xstate.json

The dict representation is:
    {"nodes": ["router", "billing", "tech"],
     "edges": [["__start__", "router"], ["billing", "__end__"], ["tech", "__end__"]],
     "conditional_edges": {"router": {"billing": "billing", "tech": "tech"}}}
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from typing import Any


# --- Heuristics for controllability classification ---

_UNCONTROLLABLE_PATTERNS = re.compile(
    r"human|user|tool_result|sensor|external|callback|webhook|timeout|error|fail",
    re.IGNORECASE,
)


def _is_env_event(event_name: str) -> bool:
    """Heuristic: events matching environment-like patterns are uncontrollable."""
    return bool(_UNCONTROLLABLE_PATTERNS.search(event_name))


def _sanitize(name: str) -> str:
    """Sanitize a name for use as an XState state/event identifier."""
    return re.sub(r"[^a-zA-Z0-9_]", "_", name).strip("_")


# --- Core conversion ---


def langgraph_to_xstate(
    graph: Any = None,
    *,
    graph_dict: dict | None = None,
    machine_id: str = "langgraph_workflow",
    controllable: list[str] | None = None,
    uncontrollable: list[str] | None = None,
    properties: list[dict] | None = None,
) -> dict:
    """Convert a LangGraph graph to XState JSON with __mununu annotations.

    Accepts either a compiled LangGraph ``CompiledStateGraph`` (via *graph*)
    or a plain dict representation (via *graph_dict*) with keys:
      - ``nodes``: list of node name strings
      - ``edges``: list of ``[source, target]`` pairs
      - ``conditional_edges``: dict mapping source node to ``{label: target}``

    Returns a dict that can be serialized with ``json.dumps``.
    """
    nodes, edges, cond_edges = _extract_structure(graph, graph_dict)

    if not nodes:
        raise ValueError("Graph has no nodes")

    # Determine initial state (target of __start__ edge)
    initial = None
    for src, tgt in edges:
        if src in ("__start__", "START"):
            initial = _sanitize(tgt)
            break
    if initial is None:
        initial = _sanitize(nodes[0])

    # Build XState states
    states: dict[str, dict] = {}
    all_events: list[str] = []

    for node in nodes:
        sname = _sanitize(node)
        if sname.startswith("__"):
            continue  # skip __start__, __end__

        on: dict[str, str] = {}

        # Unconditional edges from this node
        for src, tgt in edges:
            if _sanitize(src) != sname:
                continue
            tgt_s = _sanitize(tgt)
            if tgt_s.startswith("__"):
                tgt_s = "__done__"
            event = f"NEXT_{tgt_s}".upper()
            on[event] = tgt_s
            all_events.append(event)

        # Conditional edges from this node
        if node in cond_edges:
            for label, target in cond_edges[node].items():
                tgt_s = _sanitize(target)
                if tgt_s.startswith("__"):
                    tgt_s = "__done__"
                event = f"ROUTE_{_sanitize(label)}".upper()
                on[event] = tgt_s
                all_events.append(event)

        states[sname] = {"on": on} if on else {}

    # Add terminal state if referenced
    if "__done__" in {t for s in states.values() for t in (s.get("on") or {}).values()}:
        states["__done__"] = {}

    # Classify controllability
    ctrl_set = set(controllable or [])
    unctrl_set = set(uncontrollable or [])

    if not ctrl_set and not unctrl_set:
        # Apply heuristics
        for ev in all_events:
            if _is_env_event(ev):
                unctrl_set.add(ev)
            else:
                ctrl_set.add(ev)
        # If heuristics classified nothing, make routing controllable
        if not ctrl_set:
            ctrl_set = {ev for ev in all_events if ev.startswith("ROUTE_")}
            unctrl_set = set(all_events) - ctrl_set

    # Build properties
    props = list(properties or [])
    if not props:
        props.append({
            "name": "safety_invariant",
            "formula": "nu X. ([] X)",
            "role": "guarantee",
        })

    # Assemble XState JSON
    return {
        "id": machine_id,
        "initial": initial,
        "states": states,
        "__mununu": {
            "controllable": sorted(ctrl_set),
            "uncontrollable": sorted(unctrl_set),
            "properties": props,
        },
    }


def _extract_structure(
    graph: Any, graph_dict: dict | None
) -> tuple[list[str], list[tuple[str, str]], dict[str, dict[str, str]]]:
    """Extract nodes, edges, conditional_edges from graph or dict."""
    if graph_dict is not None:
        nodes = graph_dict.get("nodes", [])
        edges = [tuple(e) for e in graph_dict.get("edges", [])]
        cond = graph_dict.get("conditional_edges", {})
        return nodes, edges, cond

    if graph is None:
        raise ValueError("Provide either graph or graph_dict")

    # Introspect CompiledStateGraph via get_graph()
    try:
        draw = graph.get_graph()
    except AttributeError:
        raise TypeError(
            "Expected a CompiledStateGraph with get_graph() method. "
            "Pass graph_dict={'nodes':..., 'edges':..., 'conditional_edges':...} instead."
        )

    nodes = [n.id for n in draw.nodes if not n.id.startswith("__")]
    edges = []
    cond_edges: dict[str, dict[str, str]] = {}

    for edge in draw.edges:
        src = edge.source
        tgt = edge.target
        cond_label = getattr(edge, "conditional", None) or getattr(edge, "data", None)
        if cond_label and isinstance(cond_label, str):
            cond_edges.setdefault(src, {})[cond_label] = tgt
        else:
            edges.append((src, tgt))

    return nodes, edges, cond_edges


# --- CLI ---


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Convert LangGraph dict representation to XState JSON"
    )
    parser.add_argument("--input", required=True, help="Input JSON file (graph dict)")
    parser.add_argument("--output", help="Output XState JSON file (stdout if omitted)")
    parser.add_argument("--id", default="langgraph_workflow", help="Machine ID")
    args = parser.parse_args()

    with open(args.input) as f:
        graph_dict = json.load(f)

    result = langgraph_to_xstate(graph_dict=graph_dict, machine_id=args.id)
    out = json.dumps(result, indent=2) + "\n"

    if args.output:
        with open(args.output, "w") as f:
            f.write(out)
        print(f"Written to {args.output}", file=sys.stderr)
    else:
        print(out)


if __name__ == "__main__":
    main()
