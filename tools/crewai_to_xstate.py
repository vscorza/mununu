#!/usr/bin/env python3
"""crewai_to_xstate.py — Convert CrewAI Crew definitions to XState JSON with Mununu annotations.

Converts a CrewAI Crew (agents + tasks + process type) into XState v5 JSON
that can be imported by Mununu's XState adapter for formal verification.

Usage with CrewAI:

    from crewai import Agent, Task, Crew, Process
    from crewai_to_xstate import crewai_to_xstate
    import json

    researcher = Agent(role="researcher", goal="...", backstory="...")
    analyst = Agent(role="analyst", goal="...", backstory="...")
    t1 = Task(description="Research", agent=researcher, expected_output="...")
    t2 = Task(description="Analyze", agent=analyst, expected_output="...", context=[t1])
    crew = Crew(agents=[researcher, analyst], tasks=[t1, t2], process=Process.sequential)

    xstate_json = crewai_to_xstate(crew)
    print(json.dumps(xstate_json, indent=2))

Standalone usage (from dict representation):

    python3 crewai_to_xstate.py --input crew.json --output workflow.xstate.json

The dict representation is:
    {"agents": [{"role": "researcher", "allow_delegation": false, "tools": ["web_search"]}, ...],
     "tasks": [{"name": "research", "agent_role": "researcher", "context": []}, ...],
     "process": "sequential"}
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from typing import Any


def _sanitize(name: str) -> str:
    return re.sub(r"[^a-zA-Z0-9_]", "_", name).strip("_").lower()


# --- Sequential process ---


def _build_sequential(agents: list[dict], tasks: list[dict]) -> dict:
    """Build a linear state chain: task_0 → task_1 → ... → done."""
    states: dict[str, dict] = {}
    all_events: list[str] = []
    ctrl: list[str] = []
    unctrl: list[str] = []

    for i, task in enumerate(tasks):
        role = _sanitize(task.get("agent_role", f"agent_{i}"))
        tname = task.get("name") or f"task_{i}"
        sname = f"{_sanitize(tname)}_{role}"
        fail_sname = f"failed_{_sanitize(tname)}"

        complete_ev = f"COMPLETE_{_sanitize(tname)}".upper()
        fail_ev = f"FAIL_{_sanitize(tname)}".upper()
        retry_ev = f"RETRY_{_sanitize(tname)}".upper()

        # Next state
        if i + 1 < len(tasks):
            next_role = _sanitize(tasks[i + 1].get("agent_role", f"agent_{i+1}"))
            next_tname = tasks[i + 1].get("name") or f"task_{i+1}"
            next_sname = f"{_sanitize(next_tname)}_{next_role}"
        else:
            next_sname = "done"

        states[sname] = {
            "on": {
                complete_ev: next_sname,
                fail_ev: fail_sname,
            }
        }
        states[fail_sname] = {
            "on": {retry_ev: sname}
        }

        unctrl.extend([complete_ev, fail_ev])
        ctrl.append(retry_ev)
        all_events.extend([complete_ev, fail_ev, retry_ev])

    states["done"] = {}

    # Initial state = first task
    first_task = tasks[0]
    first_role = _sanitize(first_task.get("agent_role", "agent_0"))
    first_tname = first_task.get("name") or "task_0"
    initial = f"{_sanitize(first_tname)}_{first_role}"

    return states, initial, ctrl, unctrl


# --- Hierarchical process ---


def _build_hierarchical(agents: list[dict], tasks: list[dict]) -> dict:
    """Build supervisor + worker pattern with parallel regions."""
    unique_roles = []
    seen = set()
    for a in agents:
        r = _sanitize(a.get("role", "agent"))
        if r not in seen:
            unique_roles.append(r)
            seen.add(r)

    # Supervisor region
    sup_states: dict[str, dict] = {}
    ctrl: list[str] = []
    unctrl: list[str] = []

    activate_events = {r: f"ACTIVATE_{r}".upper() for r in unique_roles}
    sup_states["idle"] = {"on": {"TASK_ARRIVE": "dispatching"}}
    dispatch_on = {}
    for r in unique_roles:
        dispatch_on[activate_events[r]] = "waiting"
        ctrl.append(activate_events[r])
    sup_states["dispatching"] = {"on": dispatch_on}
    sup_states["waiting"] = {
        "on": {
            "TASK_COMPLETE": "idle",
            "AGENT_FAIL": "recovering",
            "TIMEOUT": "recovering",
        }
    }
    sup_states["recovering"] = {"on": {"RETRY": "dispatching"}}

    unctrl.extend(["TASK_ARRIVE", "TASK_COMPLETE", "AGENT_FAIL", "TIMEOUT"])
    ctrl.append("RETRY")

    # Worker regions
    worker_regions: dict[str, dict] = {}
    for r in unique_roles:
        act_ev = activate_events[r]

        w_states = {
            f"idle_{r}": {"on": {act_ev: f"working_{r}"}},
            f"working_{r}": {
                "on": {
                    "TASK_COMPLETE": f"idle_{r}",
                    "AGENT_FAIL": f"idle_{r}",
                }
            },
        }

        # Delegation edges
        agent_def = next((a for a in agents if _sanitize(a.get("role", "")) == r), {})
        if agent_def.get("allow_delegation", False):
            for other_r in unique_roles:
                if other_r != r:
                    deleg_ev = f"DELEGATE_{r}_TO_{other_r}".upper()
                    w_states[f"working_{r}"]["on"][deleg_ev] = f"idle_{r}"
                    ctrl.append(deleg_ev)

        worker_regions[r] = {
            "initial": f"idle_{r}",
            "states": w_states,
        }

    # Compose as parallel
    parallel_states = {"supervisor": {"initial": "idle", "states": sup_states}}
    parallel_states.update(worker_regions)

    outer_states = {
        "system": {
            "type": "parallel",
            "states": parallel_states,
        }
    }

    return outer_states, "system", ctrl, unctrl


# --- Main conversion ---


def crewai_to_xstate(
    crew: Any = None,
    *,
    crew_dict: dict | None = None,
    machine_id: str = "crewai_workflow",
    properties: list[dict] | None = None,
) -> dict:
    """Convert a CrewAI Crew to XState JSON with __mununu annotations.

    Accepts either a ``crewai.Crew`` instance (via *crew*) or a plain dict
    (via *crew_dict*) with keys ``agents``, ``tasks``, ``process``.
    """
    agents, tasks, process = _extract_crew(crew, crew_dict)

    if process == "hierarchical":
        states, initial, ctrl, unctrl = _build_hierarchical(agents, tasks)
    else:
        states, initial, ctrl, unctrl = _build_sequential(agents, tasks)

    props = list(properties or [])
    if not props:
        props.append({
            "name": "safety_invariant",
            "formula": "nu X. ([] X)",
            "role": "guarantee",
        })
        if process == "sequential":
            # Liveness: done is reachable
            props.append({
                "name": "can_finish",
                "formula": "mu X. (done || <> X)",
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


def _extract_crew(
    crew: Any, crew_dict: dict | None
) -> tuple[list[dict], list[dict], str]:
    """Extract agents, tasks, process from Crew or dict."""
    if crew_dict is not None:
        return (
            crew_dict.get("agents", []),
            crew_dict.get("tasks", []),
            crew_dict.get("process", "sequential"),
        )

    if crew is None:
        raise ValueError("Provide either crew or crew_dict")

    agents = []
    for a in crew.agents:
        agents.append({
            "role": getattr(a, "role", "agent"),
            "allow_delegation": getattr(a, "allow_delegation", False),
            "tools": [t.name if hasattr(t, "name") else str(t) for t in getattr(a, "tools", [])],
        })

    tasks = []
    for i, t in enumerate(crew.tasks):
        agent_role = getattr(t.agent, "role", f"agent_{i}") if t.agent else f"agent_{i}"
        tasks.append({
            "name": getattr(t, "name", None) or getattr(t, "description", f"task_{i}")[:30],
            "agent_role": agent_role,
            "context": [],  # context deps not easily extractable at runtime
        })

    process = "sequential"
    if hasattr(crew, "process"):
        p = crew.process
        process = p.value if hasattr(p, "value") else str(p)

    return agents, tasks, process


# --- CLI ---


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Convert CrewAI dict representation to XState JSON"
    )
    parser.add_argument("--input", required=True, help="Input JSON file (crew dict)")
    parser.add_argument("--output", help="Output XState JSON file")
    parser.add_argument("--id", default="crewai_workflow", help="Machine ID")
    args = parser.parse_args()

    with open(args.input) as f:
        crew_dict = json.load(f)

    result = crewai_to_xstate(crew_dict=crew_dict, machine_id=args.id)
    out = json.dumps(result, indent=2) + "\n"

    if args.output:
        with open(args.output, "w") as f:
            f.write(out)
        print(f"Written to {args.output}", file=sys.stderr)
    else:
        print(out)


if __name__ == "__main__":
    main()
