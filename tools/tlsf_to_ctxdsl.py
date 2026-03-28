#!/usr/bin/env python3
"""tlsf_to_ctxdsl.py — Translate a subset of TLSF (Temporal Logic Synthesis Format)
into Mununu CTXDSL for evaluation with `mununu context eval/synth`.

Translation model (signal-state encoding):
  A TLSF specification with n boolean inputs {i1,...,in} and m boolean outputs
  {o1,...,om} is encoded as a single automaton `Signals` whose states are all
  2^(n+m) signal valuations.

  Uncontrollable labels: set_<input>, clr_<input>  (environment flips one input bit)
  Controllable   labels: set_<output>, clr_<output> (controller flips one output bit)

  Each state has one outgoing transition per label (to the state differing in that bit).

Supported TLSF subset:
  - Simple boolean INPUTS / OUTPUTS (no arrays, no parameterization)
  - ASSUMPTIONS / GUARANTEES sections with LTL formulas using:
      G, F, GF (G(F(...))), X (encoded as box), U (until), W (weak until),
      !, &&, ||, ->, atom (signal name)
  - INVARIANTS section: each invariant phi is treated as G(phi)
  - No PARAMETERS / DEFINITIONS / GLOBAL sections (use syfco to expand first)

Usage:
    python3 tools/tlsf_to_ctxdsl.py input.tlsf
    python3 tools/tlsf_to_ctxdsl.py input.tlsf --output out.ctxdsl
    python3 tools/tlsf_to_ctxdsl.py input.tlsf --dry-run   # print CTXDSL, skip writing

The formula body encodes the synthesis objective:
    (NOT assumptions) OR (invariants AND guarantees)
which for GR(1)-structured TLSF becomes the standard mu-calculus GR(1) formula.

Limitation: X (next) is encoded as box ([_] phi) — this is an over-approximation
valid for safety properties. For liveness with X, results may be approximate.
"""

import argparse
import itertools
import re
import sys
import textwrap
from typing import NamedTuple


# ── TLSF parser ───────────────────────────────────────────────────────────────

class TlsfSpec(NamedTuple):
    title: str
    inputs: list[str]
    outputs: list[str]
    assumptions: list[str]  # raw LTL strings
    invariants: list[str]   # raw LTL strings (each wrapped in G)
    guarantees: list[str]   # raw LTL strings
    status: str             # "realizable", "unrealizable", or ""


def _strip_comments(text: str) -> str:
    """Remove // line comments (but preserve // SYNTCOMP status lines)."""
    lines = []
    for line in text.splitlines():
        # keep SYNTCOMP status metadata
        if re.match(r'\s*//\s*(STATUS|REF_SIZE)', line):
            lines.append(line)
            continue
        idx = line.find('//')
        if idx >= 0:
            line = line[:idx]
        lines.append(line)
    return '\n'.join(lines)


def _extract_section(text: str, section: str) -> str | None:
    """Extract the content between SECTION_NAME { ... } (handles nested braces)."""
    pattern = re.compile(rf'\b{section}\s*\{{', re.IGNORECASE)
    m = pattern.search(text)
    if not m:
        return None
    start = m.end()
    depth = 1
    i = start
    while i < len(text) and depth > 0:
        if text[i] == '{':
            depth += 1
        elif text[i] == '}':
            depth -= 1
        i += 1
    return text[start:i - 1].strip()


def _extract_signals(section_body: str) -> list[str]:
    """Extract signal names from a section body like 'req;\n cancel;\n'."""
    signals = []
    for token in re.split(r'[\s;,]+', section_body):
        token = token.strip()
        if token and re.match(r'^[a-zA-Z_]\w*$', token):
            signals.append(token)
    return signals


def _split_formulas(body: str) -> list[str]:
    """Split a section body into individual LTL formula strings (semicolon-separated)."""
    formulas = []
    current = []
    depth = 0
    for ch in body:
        if ch in '({':
            depth += 1
            current.append(ch)
        elif ch in ')}':
            depth -= 1
            current.append(ch)
        elif ch == ';' and depth == 0:
            formula = ''.join(current).strip()
            if formula:
                formulas.append(formula)
            current = []
        else:
            current.append(ch)
    leftover = ''.join(current).strip()
    if leftover:
        formulas.append(leftover)
    return formulas


def parse_tlsf(text: str) -> TlsfSpec:
    text = _strip_comments(text)

    # Extract SYNTCOMP status from comments
    status_m = re.search(r'STATUS\s*:\s*(\w+)', text)
    status = status_m.group(1).lower() if status_m else ""

    # Title
    title_m = re.search(r'TITLE\s*:\s*"([^"]*)"', text, re.IGNORECASE)
    title = title_m.group(1) if title_m else "unknown"

    # Check for GLOBAL section (parameterized — warn and proceed)
    if _extract_section(text, 'GLOBAL'):
        print("WARNING: GLOBAL section detected (parameterized TLSF). "
              "Use syfco to expand parameters first. Attempting best-effort parse.",
              file=sys.stderr)

    main_body = _extract_section(text, 'MAIN') or text

    # Inputs / outputs
    inputs_body = _extract_section(main_body, 'INPUTS') or ''
    outputs_body = _extract_section(main_body, 'OUTPUTS') or ''
    inputs = _extract_signals(inputs_body)
    outputs = _extract_signals(outputs_body)

    # Assumptions / invariants / guarantees
    def _get_formulas(section_name: str) -> list[str]:
        body = _extract_section(main_body, section_name) or ''
        return [f for f in _split_formulas(body) if f.strip()]

    assumptions = _get_formulas('ASSUMPTIONS')
    invariants = _get_formulas('INVARIANTS')
    guarantees = _get_formulas('GUARANTEES')

    return TlsfSpec(
        title=title,
        inputs=inputs,
        outputs=outputs,
        assumptions=assumptions,
        invariants=invariants,
        guarantees=guarantees,
        status=status,
    )


# ── LTL parser ────────────────────────────────────────────────────────────────

# LTL AST nodes are tuples: ('op', children...) or ('atom', name) or ('bool', True/False)

def _tokenize(s: str) -> list[str]:
    return re.findall(r'&&|[|][|]|->|<->|[!GFX](?=\s*\(|\s*[a-zA-Z_])|[()]|[a-zA-Z_]\w*|true|false', s)


def _parse_ltl(tokens: list[str], pos: int = 0):
    """Recursive descent LTL parser. Returns (ast, next_pos)."""
    return _parse_implication(tokens, pos)


def _parse_implication(tokens, pos):
    left, pos = _parse_or(tokens, pos)
    while pos < len(tokens) and tokens[pos] == '->':
        pos += 1
        right, pos = _parse_or(tokens, pos)
        left = ('implies', left, right)
    return left, pos


def _parse_or(tokens, pos):
    left, pos = _parse_and(tokens, pos)
    while pos < len(tokens) and tokens[pos] == '||':
        pos += 1
        right, pos = _parse_and(tokens, pos)
        left = ('or', left, right)
    return left, pos


def _parse_and(tokens, pos):
    left, pos = _parse_until(tokens, pos)
    while pos < len(tokens) and tokens[pos] == '&&':
        pos += 1
        right, pos = _parse_until(tokens, pos)
        left = ('and', left, right)
    return left, pos


def _parse_until(tokens, pos):
    # U/W/R are binary infix with lower precedence than unary !,G,F,X
    left, pos = _parse_unary(tokens, pos)
    while pos < len(tokens) and tokens[pos] in ('U', 'W', 'R'):
        op = tokens[pos]
        pos += 1
        right, pos = _parse_unary(tokens, pos)
        left = (op, left, right)
    return left, pos


def _parse_unary(tokens, pos):
    if pos >= len(tokens):
        raise ValueError(f"Unexpected end of tokens at position {pos}")
    tok = tokens[pos]
    if tok == '!':
        pos += 1
        child, pos = _parse_unary(tokens, pos)
        return ('not', child), pos
    if tok in ('G', 'F', 'X'):
        pos += 1
        child, pos = _parse_unary(tokens, pos)
        return (tok, child), pos
    # Fall through to primary — U/W/R are handled at the until level above
    return _parse_primary(tokens, pos)


def _parse_primary(tokens, pos):
    if pos >= len(tokens):
        raise ValueError("Unexpected end in primary")
    tok = tokens[pos]
    if tok == '(':
        pos += 1
        expr, pos = _parse_ltl(tokens, pos)
        if pos < len(tokens) and tokens[pos] == ')':
            pos += 1
        return expr, pos
    if tok == 'true':
        return ('bool', True), pos + 1
    if tok == 'false':
        return ('bool', False), pos + 1
    if re.match(r'^[a-zA-Z_]\w*$', tok):
        return ('atom', tok), pos + 1
    # Try unary temporal ops
    if tok in ('G', 'F', 'X', '!'):
        pos += 1
        child, pos = _parse_primary(tokens, pos)
        return (tok, child), pos
    raise ValueError(f"Unexpected token '{tok}' at pos {pos}")


def parse_ltl_formula(s: str):
    s = s.strip()
    if not s:
        return ('bool', True)
    tokens = _tokenize(s)
    if not tokens:
        return ('bool', True)
    ast, pos = _parse_ltl(tokens, 0)
    if pos != len(tokens):
        remaining = ' '.join(tokens[pos:])
        # Try to handle 'U' as infix continuation
        print(f"  WARNING: LTL parse stopped at token {pos} '{tokens[pos]}', "
              f"remaining: {remaining}", file=sys.stderr)
    return ast


# ── LTL → mu-calculus encoder ─────────────────────────────────────────────────

_FRESH_COUNTER = [0]

def _fresh(prefix: str) -> str:
    _FRESH_COUNTER[0] += 1
    return f"{prefix}{_FRESH_COUNTER[0]}"


def _reset_counter():
    _FRESH_COUNTER[0] = 0


def _atom_to_mucalc(name: str, signals: list[str], n_states: int,
                    state_names: list[str]) -> str:
    """A signal name 'sig' maps to the predicate: disjunction of states where sig=1."""
    if name not in signals:
        raise ValueError(f"Unknown signal '{name}' — not in {signals}")
    idx = signals.index(name)
    # states where signals[idx] = 1
    sat_states = [state_names[i] for i in range(n_states) if (i >> (len(signals) - 1 - idx)) & 1]
    if not sat_states:
        return 'false'
    return ' || '.join(sat_states)


def _ast_to_mucalc(ast, signals: list[str], n_states: int, state_names: list[str]) -> str:
    """Recursively translate LTL AST to Mununu mu-calculus string."""
    op = ast[0]

    def rec(a):
        return _ast_to_mucalc(a, signals, n_states, state_names)

    def paren(s: str) -> str:
        return f"({s})"

    if op == 'bool':
        return 'true' if ast[1] else 'false'

    if op == 'atom':
        name = ast[1]
        if name in ('true', 'false'):
            return name
        return paren(_atom_to_mucalc(name, signals, n_states, state_names))

    if op == 'not':
        return paren(f"! {rec(ast[1])}")

    if op == 'and':
        return paren(f"{rec(ast[1])} && {rec(ast[2])}")

    if op == 'or':
        return paren(f"{rec(ast[1])} || {rec(ast[2])}")

    if op == 'implies':
        return paren(f"(! {rec(ast[1])}) || {rec(ast[2])}")

    # G φ = ν X. φ ∧ [] X
    if op == 'G':
        v = _fresh('NuG')
        return paren(f"nu {v}. ({rec(ast[1])} && ([] {v}))")

    # F φ = μ X. φ ∨ ⟨⟩ X
    if op == 'F':
        v = _fresh('MuF')
        return paren(f"mu {v}. ({rec(ast[1])} || (<> {v}))")

    # X φ → [] φ  (box next — conservative over-approximation for safety)
    # For synthesis: X in guarantee means "after any next transition, φ holds"
    if op == 'X':
        return paren(f"([] {rec(ast[1])})")

    # φ U ψ = μ X. ψ ∨ (φ ∧ ⟨⟩ X)
    if op == 'U':
        v = _fresh('MuU')
        return paren(f"mu {v}. ({rec(ast[2])} || ({rec(ast[1])} && (<> {v})))")

    # φ W ψ = ν X. ψ ∨ (φ ∧ [] X)  (weak until = G(φ) ∨ (φ U ψ))
    if op == 'W':
        v = _fresh('NuW')
        return paren(f"nu {v}. ({rec(ast[2])} || ({rec(ast[1])} && ([] {v})))")

    # φ R ψ = ν X. ψ ∧ (φ ∨ [] X)  (release = weak until dual)
    if op == 'R':
        v = _fresh('NuR')
        return paren(f"nu {v}. ({rec(ast[2])} && ({rec(ast[1])} || ([] {v})))")

    raise ValueError(f"Unknown LTL op: {op}")


def translate_formula(assumptions: list[str], invariants: list[str],
                      guarantees: list[str], signals: list[str],
                      n_states: int, state_names: list[str]) -> str:
    """
    Build the full synthesis formula:
      (¬ ASS_1 ∨ ... ∨ ¬ ASS_k) ∨ (INV_1 ∧ ... ∧ GUAR_1 ∧ ...)

    Which is equivalent to: (∧ ASS_i) → (∧ INV_j ∧ ∧ GUAR_l)
    """
    _reset_counter()

    def ltl_to_mu(s: str) -> str:
        ast = parse_ltl_formula(s)
        return _ast_to_mucalc(ast, signals, n_states, state_names)

    # Guarantees + invariants (each invariant is wrapped in G)
    goal_parts = []
    for inv in invariants:
        ast = parse_ltl_formula(inv)
        goal_parts.append(_ast_to_mucalc(('G', ast), signals, n_states, state_names))
    for guar in guarantees:
        goal_parts.append(ltl_to_mu(guar))

    if not goal_parts:
        return 'true'

    goal = ' && '.join(goal_parts)
    if len(goal_parts) > 1:
        goal = f"({goal})"

    if not assumptions:
        return goal

    # Assumption: (¬ ass_1 ∨ ¬ ass_2 ∨ ...) ∨ goal
    # = ¬(ass_1 ∧ ass_2 ∧ ...) ∨ goal
    # Using standard GR(1) encoding: ¬GF(X) = ! (nu N. (mu M. X || <> M) && [] N)
    ass_parts = [ltl_to_mu(a) for a in assumptions]
    if len(ass_parts) == 1:
        ass_conj = ass_parts[0]
    else:
        ass_conj = '(' + ' && '.join(ass_parts) + ')'

    return f"((! {ass_conj})\n            || {goal})"


# ── Propositional invariant pruning ──────────────────────────────────────────

def _is_propositional(ast) -> bool:
    """Return True iff the AST contains no temporal operators (G, F, X, U, W, R)."""
    op = ast[0]
    if op in ('G', 'F', 'X', 'U', 'W', 'R', 'U_partial'):
        return False
    if op == 'atom':
        return True
    if op == 'bool':
        return True
    if op in ('not',):
        return _is_propositional(ast[1])
    if op in ('and', 'or', 'implies'):
        return _is_propositional(ast[1]) and _is_propositional(ast[2])
    return False


def _eval_prop(ast, valuation: int, signals: list[str]) -> bool:
    """Evaluate a propositional LTL formula on a concrete state (bit-vector valuation)."""
    op = ast[0]
    n = len(signals)
    if op == 'bool':
        return ast[1]
    if op == 'atom':
        name = ast[1]
        if name == 'true':
            return True
        if name == 'false':
            return False
        idx = signals.index(name)
        bit_pos = n - 1 - idx
        return bool((valuation >> bit_pos) & 1)
    if op == 'not':
        return not _eval_prop(ast[1], valuation, signals)
    if op == 'and':
        return _eval_prop(ast[1], valuation, signals) and _eval_prop(ast[2], valuation, signals)
    if op == 'or':
        return _eval_prop(ast[1], valuation, signals) or _eval_prop(ast[2], valuation, signals)
    if op == 'implies':
        return (not _eval_prop(ast[1], valuation, signals)) or _eval_prop(ast[2], valuation, signals)
    raise ValueError(f"Non-propositional op in propositional eval: {op}")


def split_invariants(invariants: list[str]) -> tuple[list[str], list[str]]:
    """Split TLSF invariants into propositional (safe to prune structurally)
    and temporal (must be encoded as G(formula) in mu-calculus).

    Returns (propositional_list, temporal_list).
    """
    prop, temp = [], []
    for inv in invariants:
        ast = parse_ltl_formula(inv)
        if _is_propositional(ast):
            prop.append(inv)
        else:
            temp.append(inv)
    return prop, temp


def compute_valid_states(n_states: int, signals: list[str],
                         prop_invariants: list[str]) -> set[int]:
    """Return the set of state indices satisfying all propositional invariants.

    Also removes states reachable from bad states via transitions that CANNOT
    be avoided — but for now we just filter states that directly violate
    at least one invariant. Transitions from valid states to pruned states
    are removed by generate_ctxdsl.
    """
    if not prop_invariants:
        return set(range(n_states))

    inv_asts = [parse_ltl_formula(inv) for inv in prop_invariants]
    valid = set()
    for i in range(n_states):
        if all(_eval_prop(ast, i, signals) for ast in inv_asts):
            valid.add(i)
    return valid


# ── State / transition generator ─────────────────────────────────────────────

def _state_name(valuation: int, signals: list[str]) -> str:
    """Generate a state name from a bit-vector valuation index."""
    n = len(signals)
    bits = ''.join(str((valuation >> (n - 1 - i)) & 1) for i in range(n))
    return f"v{bits}"


def _signal_index(signal: str, signals: list[str]) -> int:
    return signals.index(signal)


def generate_states(signals: list[str]) -> list[tuple[str, bool]]:
    """Returns list of (state_name, is_initial) for all 2^n states.
    Initial state: all signals = 0.
    """
    n = len(signals)
    return [
        (_state_name(i, signals), i == 0)
        for i in range(2 ** n)
    ]


def generate_transitions(signals: list[str]) -> list[tuple[str, str, str]]:
    """Returns list of (from_state, label, to_state) for all signal-flip transitions.
    Each state has one transition per signal: set_<sig> and clr_<sig>.
    set_<sig>: from state with sig=0 to state with sig=1 (all other bits unchanged)
    clr_<sig>: from state with sig=1 to state with sig=0
    Both directions exist for every state (so set_req from req=1 is a self-loop).
    Actually: set_req flips bit to 1 (from any state); clr_req flips bit to 0.
    """
    n = len(signals)
    transitions = []
    for i in range(2 ** n):
        from_name = _state_name(i, signals)
        for sig_idx, sig in enumerate(signals):
            bit_pos = n - 1 - sig_idx
            # set_sig: flip bit to 1
            to_set = i | (1 << bit_pos)
            transitions.append((from_name, f"set_{sig}", _state_name(to_set, signals)))
            # clr_sig: flip bit to 0
            to_clr = i & ~(1 << bit_pos)
            transitions.append((from_name, f"clr_{sig}", _state_name(to_clr, signals)))
    return transitions


# ── CTXDSL generator ──────────────────────────────────────────────────────────

def generate_ctxdsl(spec: TlsfSpec, context_name: str | None = None) -> str:
    all_signals = spec.inputs + spec.outputs
    n = len(all_signals)

    if n > 12:
        print(f"WARNING: {n} signals → {2**n} states. This may be very large.",
              file=sys.stderr)

    if context_name is None:
        safe = re.sub(r'\W+', '_', spec.title.lower()).strip('_') or 'syntcomp'
        context_name = f"syntcomp_{safe}"

    # Split invariants: purely propositional ones can be enforced structurally
    # (by pruning states/transitions) when the initial state (all-zero valuation)
    # satisfies them. If the initial state is a violator, fall back to
    # formula encoding so the controller can fix it in the first step.
    prop_invs_all, temp_invs = split_invariants(spec.invariants)

    all_states_raw = generate_states(all_signals)
    all_transitions_raw = generate_transitions(all_signals)
    n_all = len(all_states_raw)

    # Separate propositional invariants into:
    #   - structurally safe: initial state (valuation=0) satisfies them → prune
    #   - initial-violating: initial state fails them → encode in formula
    inv_asts_prop = [(inv, parse_ltl_formula(inv)) for inv in prop_invs_all]
    prop_invs_structural = [inv for inv, ast in inv_asts_prop
                            if _eval_prop(ast, 0, all_signals)]
    prop_invs_initial_fail = [inv for inv, ast in inv_asts_prop
                              if not _eval_prop(ast, 0, all_signals)]

    # Structural pruning: only for invariants the initial state already satisfies
    valid_indices = compute_valid_states(n_all, all_signals, prop_invs_structural)
    valid_names = {all_states_raw[i][0] for i in valid_indices}

    states = [(s, init) for s, init in all_states_raw if s in valid_names]
    transitions = [
        (frm, lbl, to)
        for frm, lbl, to in all_transitions_raw
        if frm in valid_names and to in valid_names
    ]

    n_states = len(states)
    state_names = [s for s, _ in states]

    # Formula encodes: temporal invariants + initial-state-violating prop invariants
    formula_invs = temp_invs + prop_invs_initial_fail
    formula_body = translate_formula(
        spec.assumptions, formula_invs, spec.guarantees,
        all_signals, n_states, state_names,
    )

    lines: list[str] = []

    # Header
    if spec.status:
        lines.append(f"// SYNTCOMP: {spec.title}")
        lines.append(f"// TLSF STATUS: {spec.status}")
        lines.append(f"// Inputs:  {', '.join(spec.inputs)}")
        lines.append(f"// Outputs: {', '.join(spec.outputs)}")
        pruned_note = f" ({n_all - n_states} pruned by prop. invariants)" if n_all != n_states else ""
        lines.append(f"// States:  {n_states}  (signal-state encoding, 2^{n}{pruned_note})")
        lines.append(f"// Generated by tools/tlsf_to_ctxdsl.py")
        lines.append("")

    lines.append(f"context {context_name} {{")

    # Alphabet
    lines.append("    alphabet {")
    for sig in spec.inputs:
        lines.append(f"        label set_{sig};")
        lines.append(f"        label clr_{sig};")
    for sig in spec.outputs:
        lines.append(f"        label set_{sig};")
        lines.append(f"        label clr_{sig};")
    lines.append("    }")
    lines.append("")

    # Automata
    lines.append("    automata {")
    lines.append("        automaton Signals {")
    # Controllable labels (outputs)
    lines.append("            controllable {")
    for sig in spec.outputs:
        lines.append(f"                label set_{sig};")
        lines.append(f"                label clr_{sig};")
    lines.append("            }")
    # States
    lines.append("            states {")
    for name, is_initial in states:
        init_marker = " initial" if is_initial else ""
        lines.append(f"                state {name}{init_marker};")
    lines.append("            }")
    # Transitions
    lines.append("            transitions {")
    for frm, lbl, to in transitions:
        lines.append(f"                transition {frm} -> {to} on label {lbl};")
    lines.append("            }")
    lines.append("        }")
    lines.append("    }")
    lines.append("")

    # Formula
    lines.append("    mu_formulas {")
    lines.append("        formula syntcomp_prop {")
    lines.append("            over Signals;")
    lines.append(f"            body = {formula_body};")
    lines.append("        }")
    lines.append("    }")
    lines.append("")

    # Controller
    lines.append("    controllers {")
    lines.append("        controller synth {")
    lines.append("            source Signals;")
    lines.append("            satisfying syntcomp_prop;")
    lines.append("        }")
    lines.append("    }")
    lines.append("}")

    return "\n".join(lines) + "\n"


# ── CLI ───────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description=textwrap.dedent(__doc__),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("input", help="TLSF input file")
    parser.add_argument("--output", "-o", help="Output CTXDSL file (default: stdout)")
    parser.add_argument("--name", help="Context name override")
    parser.add_argument("--dry-run", action="store_true",
                        help="Print to stdout even if --output is specified")
    args = parser.parse_args()

    with open(args.input) as f:
        text = f.read()

    spec = parse_tlsf(text)
    print(f"  Title:   {spec.title}", file=sys.stderr)
    print(f"  Inputs:  {spec.inputs}", file=sys.stderr)
    print(f"  Outputs: {spec.outputs}", file=sys.stderr)
    print(f"  Status:  {spec.status or '(not annotated)'}", file=sys.stderr)
    print(f"  States:  {2 ** len(spec.inputs + spec.outputs)}", file=sys.stderr)
    if spec.assumptions:
        print(f"  Assumptions: {len(spec.assumptions)}", file=sys.stderr)
    if spec.invariants:
        print(f"  Invariants:  {len(spec.invariants)}", file=sys.stderr)
    print(f"  Guarantees:  {len(spec.guarantees)}", file=sys.stderr)

    ctxdsl = generate_ctxdsl(spec, args.name)

    if args.output and not args.dry_run:
        with open(args.output, "w") as f:
            f.write(ctxdsl)
        print(f"  Wrote {args.output}", file=sys.stderr)
    else:
        sys.stdout.write(ctxdsl)


if __name__ == "__main__":
    main()
