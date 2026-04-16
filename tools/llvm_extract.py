#!/usr/bin/env python3
"""llvm_extract.py — Extract .espec.json from LLVM IR with GEP-based analysis.

Traces GEP (GetElementPtr) → load/store chains to extract actual guards
and effects on struct fields, producing transitions instead of self-loops.

Handles intra-procedural analysis: for each method, tracks which fields
are read (guards) and written (effects) via GEP offset resolution.

Usage:
    rustc --edition 2021 --crate-type=lib -C opt-level=0 --emit=llvm-ir source.rs -o source.ll
    python3 tools/llvm_extract.py source.ll --output spec.espec.json
"""

import argparse
import itertools
import json
import re
import sys


def demangle_rust(name: str) -> str:
    """Best-effort demangling of Rust symbol names."""
    m = re.match(r'_ZN(.+)E', name)
    if not m:
        return name
    encoded = m.group(1)
    parts = []
    i = 0
    while i < len(encoded):
        num = ""
        while i < len(encoded) and encoded[i].isdigit():
            num += encoded[i]
            i += 1
        if num:
            length = int(num)
            segment = encoded[i:i + length]
            if not (len(segment) > 2 and segment[0] == 'h' and
                    all(c in '0123456789abcdef' for c in segment[1:])):
                parts.append(segment)
            i += length
        else:
            break
    return "::".join(parts)


def parse_llvm_ir(ir_text: str) -> dict:
    """Parse LLVM IR into functions with SSA-level GEP/load/store/branch info."""
    module = {
        "source_filename": None,
        "struct_types": {},
        "functions": [],
    }

    lines = ir_text.strip().split("\n")
    current_fn = None
    current_bb = None

    for line in lines:
        stripped = line.strip()

        # Source filename
        m = re.match(r'source_filename = "(.+)"', stripped)
        if m:
            module["source_filename"] = m.group(1)

        # Struct type definitions: %"name" = type { i8, i8, ... }
        m = re.match(r'%"?(.+?)"?\s*=\s*type\s*\{(.+)\}', stripped)
        if m:
            name = m.group(1)
            fields = [f.strip() for f in m.group(2).split(",")]
            module["struct_types"][name] = fields

        # Function definition
        m = re.match(r'define\s+\S+\s+@(\S+)\((.+?)\).*\{', stripped)
        if m:
            mangled = m.group(1)
            params = m.group(2)
            # Check if first param is a self-pointer (ptr align N %self)
            has_self = "%self" in params
            current_fn = {
                "mangled": mangled,
                "demangled": demangle_rust(mangled),
                "has_self": has_self,
                "basic_blocks": {},
                "gep_map": {},  # %result -> field_offset
            }
            module["functions"].append(current_fn)
            current_bb = None
            continue

        if current_fn is not None:
            if stripped == "}":
                current_fn = None
                current_bb = None
                continue

            # Basic block label
            m = re.match(r'^(\w+):', stripped)
            if m:
                current_bb = m.group(1)
                current_fn["basic_blocks"][current_bb] = {
                    "instructions": [],
                    "terminator": None,
                }
                continue

            if current_bb is None:
                # Implicit entry block
                current_bb = "entry"
                current_fn["basic_blocks"][current_bb] = {
                    "instructions": [],
                    "terminator": None,
                }

            bb = current_fn["basic_blocks"][current_bb]

            # GEP: %r = getelementptr inbounds i8, ptr %self, i64 OFFSET
            m = re.match(
                r'%(\w+)\s*=\s*getelementptr\s+inbounds\s+\w+,\s*ptr\s+%self,\s*i64\s+(\d+)',
                stripped,
            )
            if m:
                result, offset = m.group(1), int(m.group(2))
                current_fn["gep_map"][result] = offset
                bb["instructions"].append(("gep", result, offset))
                continue

            # Load: %r = load i8, ptr %source
            m = re.match(r'%(\w+)\s*=\s*load\s+(\w+),\s*ptr\s+%(\w+)', stripped)
            if m:
                result, typ, source = m.group(1), m.group(2), m.group(3)
                bb["instructions"].append(("load", result, source, typ))
                continue

            # Trunc: %r = trunc ... i8 %val to i1
            m = re.match(r'%(\w+)\s*=\s*trunc\s+\S+\s+\w+\s+%(\w+)\s+to\s+(\w+)', stripped)
            if m:
                result, source, to_type = m.group(1), m.group(2), m.group(3)
                bb["instructions"].append(("trunc", result, source, to_type))
                continue

            # Store: store i8 VALUE, ptr %dest
            m = re.match(r'store\s+(\w+)\s+(\d+),\s*ptr\s+%(\w+)', stripped)
            if m:
                typ, value, dest = m.group(1), int(m.group(2)), m.group(3)
                bb["instructions"].append(("store", dest, value, typ))
                continue

            # Store from self: store i8 VALUE, ptr %self
            m = re.match(r'store\s+(\w+)\s+(\d+),\s*ptr\s+%self', stripped)
            if m:
                typ, value = m.group(1), int(m.group(2))
                bb["instructions"].append(("store", "__self__", value, typ))
                continue

            # Conditional branch: br i1 %cond, label %t, label %f
            m = re.match(r'br\s+i1\s+%(\w+),\s*label\s+%(\w+),\s*label\s+%(\w+)', stripped)
            if m:
                cond, true_bb, false_bb = m.group(1), m.group(2), m.group(3)
                bb["terminator"] = ("br_cond", cond, true_bb, false_bb)
                continue

            # Unconditional branch: br label %target
            m = re.match(r'br\s+label\s+%(\w+)', stripped)
            if m:
                bb["terminator"] = ("br", m.group(1))
                continue

            # Return
            if stripped.startswith("ret "):
                bb["terminator"] = ("ret",)
                continue

            # Unreachable
            if stripped == "unreachable":
                bb["terminator"] = ("unreachable",)

    return module


def analyze_method(fn: dict, struct_fields: list) -> dict:
    """Analyze a function to extract guards and effects on struct fields.

    Returns: {
        "guards": [(field_offset, "must_be_true"/"must_be_false")],
        "effects": [(field_offset, value)],
        "is_panic": bool,
    }
    """
    gep_map = fn["gep_map"]
    bbs = fn["basic_blocks"]

    # Resolve which SSA values correspond to field loads
    ssa_field_map = {}  # %ssa_name -> field_offset

    # First pass: map GEP results and loads to field offsets
    for bb_name, bb in bbs.items():
        for inst in bb["instructions"]:
            if inst[0] == "gep":
                _, result, offset = inst
                ssa_field_map[result] = offset
            elif inst[0] == "load":
                _, result, source, _ = inst
                # Load from self directly → offset 0
                if source == "self":
                    ssa_field_map[result] = 0
                elif source in gep_map:
                    ssa_field_map[result] = gep_map[source]
            elif inst[0] == "trunc":
                _, result, source, _ = inst
                if source in ssa_field_map:
                    ssa_field_map[result] = ssa_field_map[source]

    # Find the entry block
    entry_bb = None
    for name in ("start", "entry"):
        if name in bbs:
            entry_bb = name
            break
    if entry_bb is None and bbs:
        entry_bb = list(bbs.keys())[0]

    if entry_bb is None:
        return {"guards": [], "effects": [], "is_panic": False}

    # Analyze the entry block's terminator for guards
    guards = []
    entry = bbs[entry_bb]
    if entry["terminator"] and entry["terminator"][0] == "br_cond":
        _, cond_var, true_bb, false_bb = entry["terminator"]
        if cond_var in ssa_field_map:
            field_offset = ssa_field_map[cond_var]
            # Determine if true branch is early exit (panic/return)
            true_is_exit = is_early_exit_bb(bbs, true_bb)
            false_is_exit = is_early_exit_bb(bbs, false_bb)

            if true_is_exit and not false_is_exit:
                # Early return when field is true → guard: field must be false
                guards.append((field_offset, "must_be_false"))
            elif false_is_exit and not true_is_exit:
                # Early return when field is false → guard: field must be true
                guards.append((field_offset, "must_be_true"))

    # Collect effects: stores to struct fields
    effects = []
    for bb_name, bb in bbs.items():
        # Skip panic/unreachable blocks
        if bb["terminator"] and bb["terminator"][0] == "unreachable":
            continue
        for inst in bb["instructions"]:
            if inst[0] == "store":
                _, dest, value, typ = inst
                offset = None
                if dest in ("__self__", "self"):
                    offset = 0
                elif dest in gep_map:
                    offset = gep_map[dest]
                if offset is not None and 0 <= offset < len(struct_fields):
                    effects.append((offset, value))

    # Check if function is a panic wrapper
    is_panic = all(
        bb.get("terminator", (None,))[0] in ("unreachable", "ret", None)
        for bb in bbs.values()
    )

    return {"guards": guards, "effects": effects, "is_panic": is_panic}


def is_early_exit_bb(bbs: dict, bb_name: str, visited: set = None) -> bool:
    """Check if a basic block is an early exit (ret without effects, or unreachable/panic).

    A block that stores to fields and then returns is NOT an early exit —
    it's the normal completion path. An early exit is: return without stores,
    or unreachable (panic).
    """
    if visited is None:
        visited = set()
    if bb_name in visited:
        return False
    visited.add(bb_name)

    if bb_name not in bbs:
        return False
    bb = bbs[bb_name]
    term = bb.get("terminator")
    if not term:
        return False

    # Unreachable is always an early exit (panic path)
    if term[0] == "unreachable":
        return True

    # Return is an early exit only if the block has no stores
    if term[0] == "ret":
        has_stores = any(inst[0] == "store" for inst in bb["instructions"])
        return not has_stores

    # Follow unconditional branches (within depth limit)
    # but only if this block has no stores (otherwise it's an effect path, not early exit)
    if term[0] == "br" and len(visited) < 3:
        has_stores = any(inst[0] == "store" for inst in bb["instructions"])
        if not has_stores:
            return is_early_exit_bb(bbs, term[1], visited)

    return False


def build_espec(module: dict, struct_name: str = None) -> dict:
    """Build .espec.json with GEP-derived transitions."""
    # Find methods on the target struct
    method_functions = []
    target_struct = struct_name

    for fn in module["functions"]:
        demangled = fn["demangled"]
        if "::" in demangled and fn["has_self"]:
            parts = demangled.split("::")
            if len(parts) >= 2:
                if target_struct is None:
                    target_struct = parts[-2]
                if parts[-2] == target_struct:
                    method_functions.append(fn)

    if not target_struct:
        target_struct = "Module"

    # Find struct type — explicit or inferred from GEP offsets
    struct_key = None
    for key in module["struct_types"]:
        if target_struct.lower() in key.lower():
            struct_key = key
            break

    struct_fields = module["struct_types"].get(struct_key, []) if struct_key else []

    # If no explicit struct type, infer fields from GEP offsets across methods
    if not struct_fields:
        all_offsets = set()
        for fn in method_functions:
            for offset in fn["gep_map"].values():
                all_offsets.add(offset)
            # Also include offset 0 if self is loaded directly
            for bb in fn["basic_blocks"].values():
                for inst in bb["instructions"]:
                    if inst[0] == "load" and inst[2] == "self":
                        all_offsets.add(0)
                    if inst[0] == "store" and inst[1] == "__self__":
                        all_offsets.add(0)
        if all_offsets:
            max_offset = max(all_offsets)
            struct_fields = ["i8"] * (max_offset + 1)  # Assume byte-sized fields
            print(f"  Inferred {len(struct_fields)} fields from GEP offsets: {sorted(all_offsets)}", file=sys.stderr)

    # Identify boolean fields (i8 that represent Rust bool)
    bool_field_indices = [i for i, f in enumerate(struct_fields) if f.strip() in ("i1", "i8")]
    if not bool_field_indices:
        bool_field_indices = list(range(min(2, len(struct_fields))))

    # Create state space from boolean field cross-product
    if bool_field_indices:
        combos = list(itertools.product([False, True], repeat=len(bool_field_indices)))
        states = []
        for combo in combos:
            name = "_".join(
                f"f{idx}_{'T' if v else 'F'}" for idx, v in zip(bool_field_indices, combo)
            )
            states.append({
                "name": name,
                "initial": all(not v for v in combo),
                "_values": {idx: v for idx, v in zip(bool_field_indices, combo)},
            })
    else:
        states = [{"name": "S0", "initial": True, "_values": {}}]

    # Analyze each method and build transitions
    transitions = []
    seen = set()
    controllable = []

    for fn in method_functions:
        method_name = fn["demangled"].split("::")[-1]
        label = f"ev_{method_name}"
        analysis = analyze_method(fn, struct_fields)

        print(f"  {method_name}: guards={analysis['guards']}, effects={analysis['effects']}", file=sys.stderr)

        for state in states:
            vals = state["_values"]

            # Check guards
            guards_ok = True
            for field_offset, condition in analysis["guards"]:
                if field_offset in vals:
                    field_val = vals[field_offset]
                    if condition == "must_be_true" and not field_val:
                        guards_ok = False
                    elif condition == "must_be_false" and field_val:
                        guards_ok = False

            if not guards_ok:
                continue

            # Apply effects
            new_vals = dict(vals)
            for field_offset, value in analysis["effects"]:
                if field_offset in new_vals:
                    new_vals[field_offset] = bool(value)

            # Find target state
            target_name = state["name"]
            for s in states:
                if s["_values"] == new_vals:
                    target_name = s["name"]
                    break

            key = (state["name"], target_name, label)
            if key not in seen:
                seen.add(key)
                transitions.append({
                    "from": state["name"],
                    "to": target_name,
                    "label": label,
                })

    # Add noop self-loops
    for state in states:
        transitions.append({"from": state["name"], "to": state["name"], "label": "noop"})

    # Prune unreachable
    initial_name = next((s["name"] for s in states if s["initial"]), states[0]["name"])
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

    # Clean internal _values from state objects
    for s in states:
        s.pop("_values", None)

    automaton_id = f"{target_struct}FSM"
    all_labels = sorted(set(t["label"] for t in transitions))

    return {
        "$schema": "extraction_spec_v1",
        "source": {
            "repo": None,
            "commit": None,
            "file": module.get("source_filename"),
            "issue": f"Extracted from LLVM IR with GEP analysis. Struct: {target_struct}. "
                     f"Methods: {[fn['demangled'].split('::')[-1] for fn in method_functions]}. "
                     f"States: {len(states)} reachable.",
        },
        "state_fields": [],
        "methods": [],
        "bugs": [],
        "model_config": {
            "context_name": target_struct.lower(),
            "controllable_labels": controllable,
            "uncontrollable_labels": all_labels,
            "automata": [{
                "id": automaton_id,
                "states": states,
                "controllable_labels": controllable,
                "transitions": transitions,
                "note": f"LLVM IR GEP-based extraction. {len(method_functions)} methods, "
                        f"{len(bool_field_indices)} boolean fields.",
            }],
            "properties": [{
                "id": "safety",
                "formula": "nu X. ([] X)",
                "over": automaton_id,
                "description": "Trivial safety",
            }],
        },
    }


def main():
    parser = argparse.ArgumentParser(description="Extract .espec.json from LLVM IR")
    parser.add_argument("input", help="LLVM IR file (.ll)")
    parser.add_argument("--output", "-o", help="Output .espec.json path")
    parser.add_argument("--struct", help="Target struct name (auto-detected if omitted)")
    args = parser.parse_args()

    with open(args.input) as f:
        ir_text = f.read()

    module = parse_llvm_ir(ir_text)
    print(f"Source: {module['source_filename']}", file=sys.stderr)
    print(f"Struct types: {list(module['struct_types'].keys())}", file=sys.stderr)
    print(f"Functions: {len(module['functions'])}", file=sys.stderr)

    espec = build_espec(module, args.struct)

    json_str = json.dumps(espec, indent=2)
    if args.output:
        with open(args.output, "w") as f:
            f.write(json_str)
        print(f"Written to {args.output}", file=sys.stderr)
    else:
        print(json_str)


if __name__ == "__main__":
    main()
