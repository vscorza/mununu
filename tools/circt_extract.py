#!/usr/bin/env python3
"""circt_extract.py — Extract .espec.json reactive system from CIRCT MLIR.

Builds a reactive transition system from hw/comb/seq dialect MLIR:
- Every seq.firreg register is a state dimension
- Each clock step evaluates the combinational cone (mux/icmp/and/or/xor)
  to compute the next-state function
- Input ports are uncontrollable labels
- State space is the cross-product of all register values

Usage:
    circt-verilog design.sv | python3 tools/circt_extract.py --output spec.espec.json
"""

import argparse
import itertools
import json
import re
import sys


def parse_mlir(mlir_text: str) -> dict:
    """Parse CIRCT MLIR output into a simplified SSA representation."""
    module = {
        "name": None,
        "inputs": [],
        "outputs": [],
        "ops": {},       # SSA name -> (op_type, operands_dict)
        "registers": [],
    }

    lines = mlir_text.strip().split("\n")
    for line in lines:
        line = line.strip()

        # Module declaration
        m = re.match(r'hw\.module @(\w+)\((.+?)\)\s*\{', line)
        if m:
            module["name"] = m.group(1)
            ports = m.group(2)
            for port in re.findall(r'(in|out)\s+%(\w+)\s*:\s*(\w+)', ports):
                direction, name, typ = port
                entry = {"name": name, "type": typ}
                if direction == "in":
                    module["inputs"].append(entry)
                else:
                    module["outputs"].append(entry)

        # Constants: %name = hw.constant value [: type]
        m = re.match(r'%(\S+)\s*=\s*hw\.constant\s+(.+?)(?:\s*:\s*(.+))?$', line)
        if m:
            name, value = m.group(1), m.group(2).strip()
            typ = m.group(3) or ("i1" if value in ("true", "false") else "unknown")
            module["ops"][name] = ("constant", {"value": value, "type": typ})

        # State register: %name = seq.firreg %next clock %clk reset async %rst, %reset_val : type
        m = re.match(
            r'%(\w+)\s*=\s*seq\.firreg\s+%(\w+)\s+clock\s+%(\w+).*reset\s+\w+\s+%(\w+),\s*%(\S+)\s*:\s*(.+)',
            line,
        )
        if m:
            reg = {
                "name": m.group(1),
                "next": m.group(2),
                "clock": m.group(3),
                "reset_signal": m.group(4),
                "reset_value": m.group(5),
                "type": m.group(6).strip(),
            }
            module["registers"].append(reg)
            # Register is also an SSA value (current value)
            module["ops"][reg["name"]] = ("register", {"reg": reg})

        # comb.icmp: %r = comb.icmp op %a, %b : type
        m = re.match(r'%(\w+)\s*=\s*comb\.icmp\s+(\w+)\s+%(\w+),\s*%(\S+)\s*:', line)
        if m:
            module["ops"][m.group(1)] = ("icmp", {
                "op": m.group(2), "lhs": m.group(3), "rhs": m.group(4),
            })

        # comb.mux (with optional 'bin'): %r = comb.mux [bin] %sel, %t, %f : type
        m = re.match(
            r'%(\w+)\s*=\s*comb\.mux\s+(?:bin\s+)?%(\w+),\s*%(\S+),\s*%(\S+)\s*:', line
        )
        if m:
            module["ops"][m.group(1)] = ("mux", {
                "sel": m.group(2), "true": m.group(3), "false": m.group(4),
            })

        # comb.and / comb.or / comb.xor: %r = comb.OP %a, %b [, ...] : type
        for op_name in ("and", "or", "xor"):
            m = re.match(
                rf'%(\w+)\s*=\s*comb\.{op_name}\s+(.+?)\s*:', line
            )
            if m:
                operands = [o.strip().lstrip('%') for o in m.group(2).split(',')]
                module["ops"][m.group(1)] = (op_name, {"operands": operands})
                break

        # seq.to_clock (ignore, not needed for extraction)
        # hw.output (ignore, output values derived from state)

    return module


def evaluate(ops: dict, name: str, env: dict, cache: dict, depth: int = 0) -> int:
    """Evaluate an SSA value given register/input assignments in env.

    env maps SSA names of registers and inputs to concrete integer values.
    Returns an integer value for the expression.
    """
    if depth > 100:
        return 0  # prevent infinite recursion

    if name in cache:
        return cache[name]

    if name in env:
        cache[name] = env[name]
        return env[name]

    if name not in ops:
        cache[name] = 0
        return 0

    op_type, args = ops[name]

    if op_type == "constant":
        val = args["value"]
        if val == "true":
            result = 1
        elif val == "false":
            result = 0
        else:
            result = int(val)
        cache[name] = result
        return result

    if op_type == "register":
        # Current register value comes from env
        result = env.get(name, 0)
        cache[name] = result
        return result

    if op_type == "icmp":
        lhs = evaluate(ops, args["lhs"], env, cache, depth + 1)
        rhs = evaluate(ops, args["rhs"], env, cache, depth + 1)
        op = args["op"]
        if op in ("eq", "ceq"):
            result = 1 if lhs == rhs else 0
        elif op in ("ne", "cne"):
            result = 1 if lhs != rhs else 0
        elif op == "slt":
            result = 1 if lhs < rhs else 0
        elif op == "sgt":
            result = 1 if lhs > rhs else 0
        elif op == "sle":
            result = 1 if lhs <= rhs else 0
        elif op == "sge":
            result = 1 if lhs >= rhs else 0
        elif op == "ult":
            result = 1 if (lhs & 0xFFFFFFFF) < (rhs & 0xFFFFFFFF) else 0
        else:
            result = 0
        cache[name] = result
        return result

    if op_type == "mux":
        sel = evaluate(ops, args["sel"], env, cache, depth + 1)
        if sel:
            result = evaluate(ops, args["true"], env, cache, depth + 1)
        else:
            result = evaluate(ops, args["false"], env, cache, depth + 1)
        cache[name] = result
        return result

    if op_type == "and":
        vals = [evaluate(ops, o, env, cache, depth + 1) for o in args["operands"]]
        result = vals[0]
        for v in vals[1:]:
            result &= v
        cache[name] = result
        return result

    if op_type == "or":
        vals = [evaluate(ops, o, env, cache, depth + 1) for o in args["operands"]]
        result = vals[0]
        for v in vals[1:]:
            result |= v
        cache[name] = result
        return result

    if op_type == "xor":
        vals = [evaluate(ops, o, env, cache, depth + 1) for o in args["operands"]]
        result = vals[0]
        for v in vals[1:]:
            result ^= v
        cache[name] = result
        return result

    cache[name] = 0
    return 0


def extract_reactive_system(module: dict) -> dict:
    """Build a reactive transition system from the MLIR module.

    State dimensions: all seq.firreg registers
    Inputs: all non-clk/rst input ports
    Transition: for each (state, input) pair, evaluate the next-state function
    """
    if not module["registers"]:
        return {"error": "No state registers found"}

    # Identify state registers and their value ranges
    registers = []
    for reg in module["registers"]:
        typ = reg["type"]
        m = re.match(r'i(\d+)', typ)
        width = int(m.group(1)) if m else 1
        # For signed 2's complement: i2 has values -2, -1, 0, 1
        if width <= 4:
            n_values = 2 ** width
            values = list(range(-(n_values // 2), n_values // 2))
        else:
            values = [0, 1]  # abstract large registers to boolean
        registers.append({
            "name": reg["name"],
            "next": reg["next"],
            "reset_value": reg["reset_value"],
            "width": width,
            "values": values,
        })

    # Identify non-clock/reset inputs
    inputs = [
        inp for inp in module["inputs"]
        if inp["name"] not in ("clk", "rst")
    ]
    input_names = [inp["name"] for inp in inputs]

    # Map register reset values to integers
    ops = module["ops"]
    for reg in registers:
        rv = reg["reset_value"]
        if rv in ops and ops[rv][0] == "constant":
            val_str = ops[rv][1]["value"]
            reg["reset_int"] = 0 if val_str == "false" else (1 if val_str == "true" else int(val_str))
        else:
            reg["reset_int"] = 0

    # Enumerate state space
    all_reg_values = [reg["values"] for reg in registers]
    all_states = list(itertools.product(*all_reg_values))

    # Name states: S_val0_val1_... (use register values)
    def state_name(vals):
        parts = []
        for reg, val in zip(registers, vals):
            parts.append(f"{reg['name']}_{val}")
        return "_".join(parts)

    initial_vals = tuple(reg["reset_int"] for reg in registers)
    initial_name = state_name(initial_vals)

    # Enumerate input combinations (i1 inputs → {0, 1})
    input_values = [[0, 1] for _ in inputs]
    all_input_combos = list(itertools.product(*input_values)) if inputs else [()]

    # Build transitions
    transitions = []
    seen_transitions = set()

    for state_vals in all_states:
        src_name = state_name(state_vals)

        for input_vals in all_input_combos:
            # Build environment: register values + input values
            env = {}
            for reg, val in zip(registers, state_vals):
                env[reg["name"]] = val
            for inp, val in zip(inputs, input_vals):
                env[inp["name"]] = val

            # Evaluate next-state for each register
            cache = {}
            next_vals = []
            for reg in registers:
                nv = evaluate(ops, reg["next"], env, cache)
                # Clamp to valid range
                if nv not in reg["values"]:
                    # Wrap: for i2, values are -2..1
                    n_values = 2 ** reg["width"]
                    nv = ((nv + n_values // 2) % n_values) - n_values // 2
                next_vals.append(nv)

            dst_name = state_name(tuple(next_vals))

            # Create label from input combination
            if input_names:
                label_parts = [f"{name}_{val}" for name, val in zip(input_names, input_vals)]
                label = "ev_" + "_".join(label_parts)
            else:
                label = "tick"

            key = (src_name, dst_name, label)
            if key not in seen_transitions:
                seen_transitions.add(key)
                transitions.append({
                    "from": src_name,
                    "to": dst_name,
                    "label": label,
                })

    # Build state list
    states = []
    for vals in all_states:
        name = state_name(vals)
        states.append({
            "name": name,
            "initial": vals == initial_vals,
        })

    # Prune unreachable states via BFS
    reachable = set()
    queue = [initial_name]
    while queue:
        current = queue.pop(0)
        if current in reachable:
            continue
        reachable.add(current)
        for t in transitions:
            if t["from"] == current and t["to"] not in reachable:
                queue.append(t["to"])

    states = [s for s in states if s["name"] in reachable]
    transitions = [t for t in transitions if t["from"] in reachable and t["to"] in reachable]

    return {
        "module_name": module["name"],
        "states": states,
        "transitions": transitions,
        "inputs": input_names,
        "registers": [r["name"] for r in registers],
        "initial": initial_name,
        "total_enumerated": len(all_states),
        "reachable": len(reachable),
    }


def build_espec(fsm: dict, module_name: str) -> dict:
    """Build .espec.json from extracted reactive system."""
    automaton_id = module_name or "FSM"
    context_name = (module_name or "circt_extracted").lower()

    # All labels are uncontrollable (environment drives inputs)
    all_labels = sorted(set(t["label"] for t in fsm["transitions"]))

    return {
        "$schema": "extraction_spec_v1",
        "source": {
            "repo": None,
            "commit": None,
            "file": None,
            "issue": f"Reactive system extracted from CIRCT MLIR for module {module_name}. "
                     f"Registers: {fsm.get('registers', [])}. "
                     f"States: {fsm.get('reachable', '?')}/{fsm.get('total_enumerated', '?')} reachable.",
        },
        "state_fields": [],
        "methods": [],
        "bugs": [],
        "model_config": {
            "context_name": context_name,
            "controllable_labels": [],
            "uncontrollable_labels": all_labels + ["noop"],
            "automata": [{
                "id": automaton_id,
                "states": fsm["states"],
                "controllable_labels": [],
                "transitions": fsm["transitions"] + [
                    {"from": s["name"], "to": s["name"], "label": "noop"}
                    for s in fsm["states"]
                ],
                "note": f"Reactive system from CIRCT. Registers: {fsm.get('registers', [])}. "
                        f"{fsm.get('reachable', '?')}/{fsm.get('total_enumerated', '?')} states reachable.",
            }],
            "properties": [{
                "id": "safety",
                "formula": "nu X. ([] X)",
                "over": automaton_id,
                "description": "Trivial safety — all reachable states satisfy",
            }],
        },
    }


def main():
    parser = argparse.ArgumentParser(
        description="Extract .espec.json reactive system from CIRCT MLIR output"
    )
    parser.add_argument(
        "input", nargs="?",
        help="MLIR file (or stdin if omitted)",
    )
    parser.add_argument(
        "--output", "-o",
        help="Output .espec.json path (default: stdout)",
    )
    args = parser.parse_args()

    if args.input:
        with open(args.input) as f:
            mlir_text = f.read()
    else:
        mlir_text = sys.stdin.read()

    module = parse_mlir(mlir_text)
    if not module["name"]:
        print("Error: No hw.module found in MLIR", file=sys.stderr)
        sys.exit(1)

    print(f"Module: {module['name']}", file=sys.stderr)
    print(f"Inputs: {[i['name'] for i in module['inputs']]}", file=sys.stderr)
    print(f"Outputs: {[o['name'] for o in module['outputs']]}", file=sys.stderr)
    print(f"Registers: {[r['name'] for r in module['registers']]}", file=sys.stderr)
    print(f"SSA ops: {len(module['ops'])}", file=sys.stderr)

    fsm = extract_reactive_system(module)
    if "error" in fsm:
        print(f"Error: {fsm['error']}", file=sys.stderr)
        sys.exit(1)

    print(f"States: {fsm['reachable']}/{fsm['total_enumerated']} reachable", file=sys.stderr)
    print(f"Transitions: {len(fsm['transitions'])}", file=sys.stderr)
    print(f"Initial: {fsm['initial']}", file=sys.stderr)

    espec = build_espec(fsm, module["name"])

    json_str = json.dumps(espec, indent=2)
    if args.output:
        with open(args.output, "w") as f:
            f.write(json_str)
        print(f"Written to {args.output}", file=sys.stderr)
    else:
        print(json_str)


if __name__ == "__main__":
    main()
