#!/usr/bin/env python3
"""a2a_to_xstate.py — Convert A2A Agent Cards to XState JSON with Mununu annotations.

Given one or more A2A Agent Cards (JSON), generates an XState v5 statechart
modeling the task lifecycle for each agent and a top-level orchestrator.

Each agent's skills become controllable invocation events. Task completion,
failure, and timeout are uncontrollable environment events. Multiple agents
are composed as parallel regions under an orchestrator.

Usage:

    python3 a2a_to_xstate.py --input card1.json card2.json --output protocol.xstate.json

Or programmatically:

    from a2a_to_xstate import a2a_to_xstate
    import json

    cards = [json.load(open("card1.json")), json.load(open("card2.json"))]
    xstate_json = a2a_to_xstate(cards)
    print(json.dumps(xstate_json, indent=2))
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from typing import Any


def _sanitize(name: str) -> str:
    return re.sub(r"[^a-zA-Z0-9_]", "_", name).strip("_").lower()


def _agent_name(card: dict) -> str:
    return _sanitize(card.get("name", "agent"))


def a2a_to_xstate(
    agent_cards: list[dict],
    *,
    machine_id: str = "a2a_protocol",
    properties: list[dict] | None = None,
) -> dict:
    """Convert A2A Agent Cards to an XState orchestration model.

    Each agent gets a task-lifecycle state machine (idle → queued →
    in_progress → completed | failed). Skills become invocation events.
    Multiple agents are wrapped in a parallel composition.
    """
    if not agent_cards:
        raise ValueError("At least one Agent Card required")

    ctrl: list[str] = []
    unctrl: list[str] = []

    # Build per-agent regions
    agent_regions: dict[str, dict] = {}

    for card in agent_cards:
        aname = _agent_name(card)
        skills = card.get("skills", [])

        # States for this agent's task lifecycle
        idle_s = f"idle_{aname}"
        queued_s = f"queued_{aname}"
        in_progress_s = f"in_progress_{aname}"
        completed_s = f"completed_{aname}"
        failed_s = f"failed_{aname}"

        # Skill invocation events (controllable) — transition from idle to queued
        invoke_events: dict[str, str] = {}
        for skill in skills:
            sid = _sanitize(skill.get("id", skill.get("name", "skill")))
            ev = f"INVOKE_{aname}_{sid}".upper()
            invoke_events[ev] = queued_s
            ctrl.append(ev)

        # If no skills, provide a generic invoke
        if not invoke_events:
            ev = f"INVOKE_{aname}".upper()
            invoke_events[ev] = queued_s
            ctrl.append(ev)

        # Cancel event (controllable)
        cancel_ev = f"CANCEL_{aname}".upper()
        ctrl.append(cancel_ev)

        # Environment events (uncontrollable)
        start_ev = f"START_{aname}".upper()
        complete_ev = f"COMPLETED_{aname}".upper()
        fail_ev = f"FAILED_{aname}".upper()
        timeout_ev = f"TIMEOUT_{aname}".upper()
        unctrl.extend([start_ev, complete_ev, fail_ev, timeout_ev])

        # Reset event — return to idle after completion/failure
        reset_ev = f"RESET_{aname}".upper()
        ctrl.append(reset_ev)

        a_states = {
            idle_s: {"on": invoke_events},
            queued_s: {
                "on": {
                    start_ev: in_progress_s,
                    cancel_ev: idle_s,
                    timeout_ev: failed_s,
                }
            },
            in_progress_s: {
                "on": {
                    complete_ev: completed_s,
                    fail_ev: failed_s,
                    timeout_ev: failed_s,
                }
            },
            completed_s: {"on": {reset_ev: idle_s}},
            failed_s: {"on": {reset_ev: idle_s}},
        }

        agent_regions[aname] = {
            "initial": idle_s,
            "states": a_states,
        }

    # Build top-level structure
    if len(agent_cards) == 1:
        # Single agent: flat machine
        aname = _agent_name(agent_cards[0])
        region = agent_regions[aname]
        states = region["states"]
        initial = region["initial"]
    else:
        # Multiple agents: parallel composition
        states = {
            "system": {
                "type": "parallel",
                "states": agent_regions,
            }
        }
        initial = "system"

    # Properties
    props = list(properties or [])
    if not props:
        props.append({
            "name": "safety_invariant",
            "formula": "nu X. ([] X)",
            "role": "guarantee",
        })

        # For multi-agent: mutex (no two agents in_progress simultaneously)
        if len(agent_cards) >= 2:
            names = [_agent_name(c) for c in agent_cards]
            for i in range(len(names)):
                for j in range(i + 1, len(names)):
                    props.append({
                        "name": f"mutex_{names[i]}_{names[j]}",
                        "formula": (
                            f"nu X. ((!in_progress_{names[i]} || !in_progress_{names[j]})"
                            f" && ([] X))"
                        ),
                        "role": "guarantee",
                    })

    return {
        "id": machine_id,
        "initial": initial,
        "states": states,
        "__mununu": {
            "controllable": sorted(set(ctrl)),
            "uncontrollable": sorted(set(unctrl)),
            "properties": props,
        },
    }


# --- CLI ---


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Convert A2A Agent Cards to XState JSON"
    )
    parser.add_argument(
        "--input", nargs="+", required=True, help="Agent Card JSON files"
    )
    parser.add_argument("--output", help="Output XState JSON file")
    parser.add_argument("--id", default="a2a_protocol", help="Machine ID")
    args = parser.parse_args()

    cards = []
    for path in args.input:
        with open(path) as f:
            cards.append(json.load(f))

    result = a2a_to_xstate(cards, machine_id=args.id)
    out = json.dumps(result, indent=2) + "\n"

    if args.output:
        with open(args.output, "w") as f:
            f.write(out)
        print(f"Written to {args.output}", file=sys.stderr)
    else:
        print(out)


if __name__ == "__main__":
    main()
