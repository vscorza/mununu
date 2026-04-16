#!/usr/bin/env python3
"""llvm_extract.py — Extract .espec.json from LLVM IR.

Parses LLVM IR (.ll file) produced by rustc --emit=llvm-ir or clang -emit-llvm
and extracts function definitions as transition labels. State is derived from
struct fields when possible.

This is a basic LLVM extractor — it identifies functions and basic blocks
but does NOT perform points-to analysis or call graph construction (that
requires SVF). It's suitable for single-module extraction where the struct
layout and method signatures are visible in the IR.

Usage:
    rustc --edition 2021 --crate-type=lib --emit=llvm-ir source.rs -o source.ll
    python3 tools/llvm_extract.py source.ll --output spec.espec.json

    clang -emit-llvm -S source.c -o source.ll
    python3 tools/llvm_extract.py source.ll --output spec.espec.json
"""

import argparse
import json
import re
import sys


def parse_llvm_ir(ir_text: str) -> dict:
    """Parse LLVM IR into a simplified representation."""
    module = {
        "source_filename": None,
        "struct_types": {},
        "functions": [],
    }

    lines = ir_text.strip().split("\n")
    current_fn = None

    for line in lines:
        stripped = line.strip()

        # Source filename
        m = re.match(r'source_filename = "(.+)"', stripped)
        if m:
            module["source_filename"] = m.group(1)

        # Struct type definitions
        m = re.match(r'%"?(.+?)"?\s*=\s*type\s*\{(.+)\}', stripped)
        if m:
            name = m.group(1)
            fields = [f.strip() for f in m.group(2).split(",")]
            module["struct_types"][name] = fields

        # Function definitions
        m = re.match(r'define\s+\S+\s+@(\S+)\((.+?)\).*\{', stripped)
        if m:
            mangled_name = m.group(1)
            params = m.group(2)
            current_fn = {
                "mangled_name": mangled_name,
                "demangled": demangle_rust(mangled_name),
                "params": params,
                "basic_blocks": 0,
                "stores": [],
                "loads": [],
                "branches": 0,
                "calls": [],
            }
            module["functions"].append(current_fn)
            continue

        if current_fn is not None:
            if stripped == "}":
                current_fn = None
                continue

            # Count basic blocks
            if re.match(r'\w+:', stripped):
                current_fn["basic_blocks"] += 1

            # Store instructions (writes to struct fields)
            if "store" in stripped:
                current_fn["stores"].append(stripped)

            # Load instructions (reads from struct fields)
            if "load" in stripped:
                current_fn["loads"].append(stripped)

            # Branch instructions
            if stripped.startswith("br "):
                current_fn["branches"] += 1

            # Call instructions
            m = re.search(r'call\s+\S+\s+@(\S+)', stripped)
            if m:
                current_fn["calls"].append(m.group(1))

    return module


def demangle_rust(name: str) -> str:
    """Best-effort demangling of Rust symbol names."""
    # Remove _ZN prefix and hash suffix
    m = re.match(r'_ZN(.+)E', name)
    if not m:
        return name

    encoded = m.group(1)
    parts = []
    i = 0
    while i < len(encoded):
        # Read length prefix
        num = ""
        while i < len(encoded) and encoded[i].isdigit():
            num += encoded[i]
            i += 1
        if num:
            length = int(num)
            # Check if this is a hash (17h followed by hex)
            segment = encoded[i:i + length]
            if not (len(segment) > 2 and segment[0] == 'h' and all(c in '0123456789abcdef' for c in segment[1:])):
                parts.append(segment)
            i += length
        else:
            break

    return "::".join(parts)


def build_espec(module: dict, struct_name: str = None) -> dict:
    """Build .espec.json from parsed LLVM IR."""
    # Find functions that look like methods on a struct
    method_functions = []
    target_struct = struct_name

    for fn in module["functions"]:
        demangled = fn["demangled"]
        if "::" in demangled:
            parts = demangled.split("::")
            if len(parts) >= 2:
                if target_struct is None:
                    target_struct = parts[-2]
                if parts[-2] == target_struct:
                    method_functions.append({
                        "name": parts[-1],
                        "basic_blocks": fn["basic_blocks"],
                        "stores": len(fn["stores"]),
                        "loads": len(fn["loads"]),
                        "branches": fn["branches"],
                    })

    if not target_struct:
        target_struct = "Module"

    automaton_id = f"{target_struct}FSM"
    context_name = target_struct.lower()

    # Build states from struct type fields (if available)
    struct_key = None
    for key in module["struct_types"]:
        if target_struct.lower() in key.lower():
            struct_key = key
            break

    states = [{"name": "S0", "initial": True}]
    if struct_key:
        fields = module["struct_types"][struct_key]
        # For boolean fields (i1 in LLVM IR), create state dimensions
        bool_fields = [i for i, f in enumerate(fields) if f.strip() == "i1"]
        if bool_fields:
            # Cross-product of boolean fields
            import itertools
            combos = list(itertools.product([False, True], repeat=len(bool_fields)))
            states = []
            for combo in combos:
                name = "_".join(f"f{i}_{'T' if v else 'F'}" for i, v in zip(bool_fields, combo))
                states.append({
                    "name": name,
                    "initial": all(not v for v in combo),
                })

    # Build transitions from methods
    transitions = []
    controllable = []
    for method in method_functions:
        label = f"ev_{method['name']}"
        # Methods with stores are effects; methods without are queries
        has_effect = method["stores"] > 0
        has_guard = method["branches"] > 0

        for state in states:
            transitions.append({
                "from": state["name"],
                "to": state["name"],  # simplified: self-loop (proper analysis needs dataflow)
                "label": label,
            })

    # Add noop
    for state in states:
        transitions.append({"from": state["name"], "to": state["name"], "label": "noop"})

    return {
        "$schema": "extraction_spec_v1",
        "source": {
            "repo": None,
            "commit": None,
            "file": module.get("source_filename"),
            "issue": f"Extracted from LLVM IR. Struct: {target_struct}. Methods: {[m['name'] for m in method_functions]}.",
        },
        "state_fields": [],
        "methods": [],
        "bugs": [],
        "model_config": {
            "context_name": context_name,
            "controllable_labels": controllable,
            "uncontrollable_labels": [f"ev_{m['name']}" for m in method_functions] + ["noop"],
            "automata": [{
                "id": automaton_id,
                "states": states,
                "controllable_labels": controllable,
                "transitions": transitions,
                "note": f"Extracted from LLVM IR. {len(method_functions)} methods, {len(states)} states.",
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
    for fn in module["functions"]:
        print(f"  {fn['demangled']} ({fn['basic_blocks']} BBs, {len(fn['stores'])} stores, {fn['branches']} branches)", file=sys.stderr)

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
