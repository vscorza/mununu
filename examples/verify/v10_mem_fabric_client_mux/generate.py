#!/usr/bin/env python3
"""V.10 memory-fabric client mux — registered-mux variant study.

    generate.py <v1|v2|v3|v4> <out_dir> [--strict-schedule]

Emits an explicit-state CTXDSL model of ONE client port of a fixed-slot
memory fabric plus the fabric's request-issue path, and a verify.toml, for
one of four issue-path designs.

--------------------------------------------------------------------------
The question
--------------------------------------------------------------------------
The fabric selects the granted client's address with a combinational 5:1 mux
that sits between the clients' flops and the SDRAM row/column decode. The mux
costs ~22% of the memory clock. Registering it is the obvious fix and it is
UNSOUND: one clock edge can carry both an `accept` (of the request issued last
cycle) and a fresh `grant` (because the slot arbiter backfills, so a client can
be granted in consecutive cycles). Both updates are non-blocking, so the grant
captures the client's pointer BEFORE the accept advanced it, and the same
address is issued twice.

The four variants:

  v1  combinational mux (today)             — sound, 22% too slow
  v2  registered mux, no other change       — the failed attempt; reproduce it
  v3  registered mux + client advances its
      pointer on GRANT (presents its next
      address before the current one is
      accepted)                             — ?
  v4  registered mux + fabric HOLDS the
      request until accepted, with
      backpressure to the client            — ?

The discriminating verdicts are `reachable(DUP)` and `reachable(SKIP)`:

  v1 ⇒ DUP false, SKIP false     (the contrast pair's sound half)
  v2 ⇒ DUP TRUE                  (reproduces mem_board 10/12: 5 extra returns)
  v3 ⇒ SKIP TRUE                 (a refused grant drops a word the client
                                  has already advanced past)
  v4 ⇒ DUP false, SKIP false     (the grant is absorbed by backpressure)

--------------------------------------------------------------------------
Timing semantics (the ones that produced wrong models before)
--------------------------------------------------------------------------
* One CTXDSL transition is ONE CLOCK EDGE, and several things happen in it.
  `grant`, `accept` and a pointer advance can coincide; they are emitted as
  MULTI-LABEL transitions (`on label accept, label grant`).
* Every effect reads the PRE-state. `succ()` below computes the whole next
  state from `s` and never from a partially-updated value — that is
  non-blocking assignment, and it is the entire bug.
* A grant need NOT be followed by an accept. `sdram_burst` may refuse (the
  bank is not timed in); card B-21 measured 0.949 accepted beats per granted
  read slot. `refuse` is a free environment choice on every presented request.
* The environment acts on its own schedule. `grant` is a free choice on EVERY
  edge, so consecutive grants are possible — that is backfill ("25% is a
  floor, not a cap"), and a scheduler that granted once per 64 cycles could
  not express the bug.
* Returns (`r_valid` / `rid`) are decoupled by tens of cycles and are NOT
  modelled: this question is about the issue path, and tying a return to its
  issue would model a different memory.

--------------------------------------------------------------------------
Why explicit states and not CTXDSL `variables` + `guards`
--------------------------------------------------------------------------
CTXDSL transition guards accept exactly ONE comparison. `&&` / `||` / `!` in
a guard are parsed lossily and SILENTLY DISABLE the transition (verified
2026-09-04; `crate::guard::GuardExpr` has no And/Or/Not variant, so
`split_comparison` turns `a == b && a == 2` into `a == "b && a == 2"`).
Every enabling condition here is conjunctive — "in BUSY, and reg == del, and
the pointer is still in range" — so a hand-written variable+guard model would
have silently lost transitions and under-approximated: the model would have
been TOO KIND and would have missed the bug it exists to find.

Conjunction IS available in mu-calculus FORMULAS. So the discipline is:
enumerate the operational semantics here in Python, encode the configuration
in the state name, and keep the properties simple. This is the same idiom as
`examples/verify/v2_tso_storebuffer` and `v1_noc_mesh_4router`.

--------------------------------------------------------------------------
Operational state
--------------------------------------------------------------------------
    (loc, ptr, reg, dlv, err)

    loc ∈ {IDLE, BUSY}  BUSY = the issue register holds a request that is
                        being presented to sdram_burst THIS cycle.
                        v1 has no register and is always IDLE.
    ptr ∈ 0..N+1        the client's address pointer (what it drives on
                        c_addr, stable until its request is accepted)
    reg ∈ 0..N | None   the address in the fabric's issue register
                        (None while IDLE — the register content is dead)
    dlv ∈ 0..N+1        the address sdram_burst expects next == the number of
                        words correctly delivered so far
    err ∈ {None,DUP,SKIP}

`dlv` is the oracle, and it is the DEFINITION of correct delivery, not a
hypothesis about the mechanism: the memory must be handed address 0, then 1,
then 2, ... exactly once each. On an accept of address `a`:

    a == dlv  → good, dlv advances
    a <  dlv  → DUP   an address already delivered is delivered again
    a >  dlv  → SKIP  a word was never issued

DUP and SKIP are absorbing (first error wins), which keeps the state space
small and the property crisp. Reaching ptr/dlv == N+1 with no error is DONE.
"""
import os
import sys
from collections import deque

# A 4-word stream (addresses 0..3) — enough for a duplicate (needs two words)
# and for a skip (needs a dropped word plus its successor).
N = 3

IDLE, BUSY = "IDLE", "BUSY"

# state = (loc, ptr, reg, dlv, err)
INIT = (IDLE, 0, None, 0, None)


def classify(a, dlv):
    """The delivery oracle. Returns 'good' | 'DUP' | 'SKIP'."""
    if a == dlv:
        return "good"
    if a < dlv:
        return "DUP"
    return "SKIP"


def succ(s, variant, backfill=True):
    """All successors of `s` as (labels, next_state) pairs.

    Every field of the next state is computed from `s` alone — never from a
    field already updated in this call. That is the non-blocking-assignment
    semantics of the hardware, and the reason v2 is unsound.

    `backfill=False` is the CONTROL: it forbids a grant while a request is
    already in flight, i.e. a strict slot schedule with no back-to-back
    grants. v2's duplicate must DISAPPEAR under it — that is what proves the
    duplicate is caused by the coincident grant+accept edge and not by a
    modelling artifact. See validate.sh.
    """
    loc, ptr, reg, dlv, err = s

    if err is not None:              # DUP / SKIP are absorbing
        return [(["sink"], s)]
    if dlv > N:                      # stream complete
        return [(["sink"], s)]

    out = []
    # `grant` is a free environment choice on EVERY edge — this is what makes
    # consecutive grants (backfill) possible, and it is what v2 needs.
    for grant in (False, True):
        if grant and not backfill and loc == BUSY:
            continue
        if variant == "v1":
            # Combinational mux: the arbiter's grant, the address select and
            # sdram_burst's accept all happen in the SAME cycle. There is no
            # register, so the address presented is always the live pointer.
            if not grant:
                out.append((["nogrant"], s))
                continue
            for acc in (False, True):
                if not acc:
                    out.append((["grant", "refuse"], s))
                    continue
                verdict = classify(ptr, dlv)
                if verdict == "good":
                    out.append((["grant", "accept"],
                                (IDLE, ptr + 1, None, dlv + 1, None)))
                else:
                    out.append((["grant", "accept"],
                                (IDLE, ptr, None, dlv, verdict)))
            continue

        # ---- v2 / v3 / v4 : the mux output is registered -----------------
        # Retire first (what the memory does with the request presented this
        # cycle), then capture (what the arbiter's grant latches). BOTH read
        # the pre-state.
        retires = [None] if loc == IDLE else [False, True]   # False = refuse
        for acc in retires:
            nptr, ndlv, nerr = ptr, dlv, None
            retired = False
            lbl = []

            if acc is True:
                a = reg
                verdict = classify(a, dlv)
                if verdict == "good":
                    ndlv = dlv + 1
                    # v3's client already advanced at the grant; v1/v2/v4
                    # advance on the accept, per the c_acc contract.
                    if variant in ("v2", "v4"):
                        nptr = ptr + 1
                else:
                    nerr = verdict
                retired = True
                lbl.append("accept")
            elif acc is False:
                # sdram_burst refused. v2/v3 drop the request (nothing holds
                # it); v4 keeps driving it.
                retired = variant in ("v2", "v3")
                lbl.append("refuse")

            if nerr is not None:
                out.append((lbl + (["grant"] if grant else ["nogrant"]),
                            (IDLE, ptr, None, dlv, nerr)))
                continue

            # ---- capture ------------------------------------------------
            if variant == "v4" and loc == BUSY and acc is not True:
                # A request is held and was not retired: the fabric asserts
                # backpressure, so the arbiter's grant cannot latch.
                nloc, nreg = BUSY, reg
                lbl.append("grant_absorbed" if grant else "nogrant")
            elif variant == "v4" and loc == BUSY and acc is True:
                # The held request retired THIS edge. In the pre-state the
                # register was still valid, so backpressure was asserted for
                # this cycle and the coincident grant is absorbed too. This
                # is exactly the edge that breaks v2, and it is why v4 is
                # sound — at the cost of the back-to-back slot.
                nloc, nreg = IDLE, None
                lbl.append("grant_absorbed" if grant else "nogrant")
            elif grant:
                # v2 / v3 (and v4 from IDLE): the grant latches the address
                # the client is driving RIGHT NOW — the pre-state pointer.
                nloc, nreg = BUSY, ptr
                if variant == "v3" and ptr <= N:
                    # v3's client presents its next address immediately, i.e.
                    # it advances on c_gnt rather than on c_acc.
                    nptr = ptr + 1
                lbl.append("grant")
            else:
                nloc = BUSY if (loc == BUSY and not retired) else IDLE
                nreg = reg if nloc == BUSY else None
                lbl.append("nogrant")

            if nptr > N + 1 or ndlv > N + 1:
                continue
            out.append((lbl, (nloc, nptr, nreg, ndlv, nerr)))
    return out


def state_name(s):
    loc, ptr, reg, dlv, err = s
    if err is not None:
        return err
    if dlv > N:
        return "DONE"
    r = "x" if reg is None else str(reg)
    return "S_%s_p%d_r%s_d%d" % (loc, ptr, r, dlv)


def explore(variant, backfill=True):
    """BFS the reachable state space. Returns (names, edges, initial)."""
    seen, order, edges = {INIT}, [INIT], []
    q = deque([INIT])
    while q:
        s = q.popleft()
        for labels, t in succ(s, variant, backfill):
            edges.append((state_name(s), labels, state_name(t)))
            if t not in seen:
                seen.add(t)
                order.append(t)
                q.append(t)
    names = []
    for s in order:
        n = state_name(s)
        if n not in names:
            names.append(n)
    # The three outcome states are declared in EVERY variant, reachable or not.
    # A `reachable(DUP)` property whose target state does not exist is a hard
    # realization error ("unknown state 'DUP'"), not a `false` verdict — so a
    # sound variant would refuse to load instead of answering the question,
    # and the v1-vs-v2 contrast pair would collapse. Declaring them keeps the
    # property text identical across all four variants, which is the whole
    # point of a contrast pair.
    for outcome in ("DUP", "SKIP", "DONE"):
        if outcome not in names:
            names.append(outcome)
            edges.append((outcome, ["sink"], outcome))
    # Collapse duplicate edges introduced by the name-level quotient.
    uniq, out = set(), []
    for src, labels, dst in edges:
        key = (src, tuple(labels), dst)
        if key not in uniq:
            uniq.add(key)
            out.append((src, labels, dst))
    return names, out, state_name(INIT)


BLURB = {
    "v1": "combinational 5:1 mux (today's RTL) — sound, and 22% too slow.",
    "v2": "registered mux, no other change — the attempt that failed mem_board 10/12.",
    "v3": "registered mux + the client advances its pointer on c_gnt "
          "(presents its next address before the current one is accepted).",
    "v4": "registered mux + the fabric holds the request until accepted, "
          "with backpressure absorbing a coincident grant.",
}


def emit_ctxdsl(variant, names, edges, initial):
    L = []
    L.append("// mem_fabric client-mux variant %s — GENERATED by ../generate.py."
             % variant.upper())
    L.append("// Do not hand-edit; re-run the generator.")
    L.append("//")
    L.append("// %s" % BLURB[variant])
    L.append("//")
    L.append("// One transition = one clock edge. Multi-label transitions carry the")
    L.append("// events that coincide in that edge; `accept, grant` is the edge that")
    L.append("// breaks the registered mux. State name S_<loc>_p<ptr>_r<reg>_d<dlv>:")
    L.append("//   loc  IDLE / BUSY  — BUSY = the issue register is presenting to sdram_burst")
    L.append("//   ptr             — the client's address pointer (c_addr)")
    L.append("//   reg             — the address in the fabric's issue register (x = none)")
    L.append("//   dlv             — the address sdram_burst expects next (the oracle)")
    L.append("// DUP / SKIP are absorbing error states; DONE is clean completion.")
    L.append("")
    L.append("context MemFabric%s {" % variant.upper())
    L.append("    automata {")
    L.append("        automaton Fabric {")
    L.append("            // Every label is environment-driven: the slot arbiter chooses")
    L.append("            // grant/nogrant and sdram_burst chooses accept/refuse. An empty")
    L.append("            // `controllable` block makes the declaration explicit, so every")
    L.append("            // label classifies Uncontrollable instead of defaulting to")
    L.append("            // Controllable.")
    L.append("            controllable { }")
    L.append("            states {")
    for n in names:
        L.append("                state %s%s;" % (n, " initial" if n == initial else ""))
    L.append("            }")
    L.append("            transitions {")
    for src, labels, dst in edges:
        lbl = ", ".join("label %s" % x for x in labels)
        L.append("                transition %s -> %s on %s;" % (src, dst, lbl))
    L.append("            }")
    L.append("            predicates {")
    for outcome in ("DUP", "SKIP", "DONE"):
        L.append("                predicate %s_reached = state %s;"
                 % (outcome.lower(), outcome))
    L.append("            }")
    L.append("        }")
    L.append("    }")
    L.append("}")
    return "\n".join(L) + "\n"


def emit_verify_toml(variant, names):
    L = []
    L.append("# mem_fabric client-mux variant %s. GENERATED by ../generate.py —"
             % variant.upper())
    L.append("# do not hand-edit.")
    L.append("#")
    L.append("# %s" % BLURB[variant])
    L.append("")
    L.append("[project]")
    L.append('name = "MemFabric%s"' % variant.upper())
    L.append('description = "monono mem_fabric client-mux variant %s: is a duplicate '
             'or skipped issue reachable?"' % variant.upper())
    L.append("")
    L.append("[[sources]]")
    L.append('id = "Fabric"')
    L.append('adapter = "ctxdsl"')
    L.append('files = ["fabric_%s.ctxdsl"]' % variant)
    L.append("")
    L.append("[alphabet]")
    L.append('strategy = "direct"')
    L.append("")
    L.append("[composition]")
    L.append('semantics = "asynchronous"')
    L.append('members = ["Fabric"]')
    L.append('name = "FabricSys"')
    L.append("")
    L.append("[[properties]]")
    L.append("# THE question: is the same address ever handed to sdram_burst twice?")
    L.append('name = "duplicate_issue_reachable"')
    L.append('template = "reachable"')
    L.append('args = { TARGET = "DUP" }')
    L.append('over = "FabricSys"')
    L.append("")
    L.append("[[properties]]")
    L.append("# The dual failure: is a word ever skipped (never issued at all)?")
    L.append('name = "skipped_word_reachable"')
    L.append('template = "reachable"')
    L.append('args = { TARGET = "SKIP" }')
    L.append('over = "FabricSys"')
    L.append("")
    L.append("[[properties]]")
    L.append("# NON-VACUITY GATE (mandatory). If the clean completion is not reachable")
    L.append("# the model is stuck and the two verdicts above mean nothing.")
    L.append('name = "clean_completion_reachable"')
    L.append('template = "reachable"')
    L.append('args = { TARGET = "DONE" }')
    L.append('over = "FabricSys"')
    L.append("")
    L.append("[[properties]]")
    L.append("# Starvation / deadlock-freedom: from every reachable state, can the")
    L.append("# stream still complete? A refusing memory must only DELAY the client,")
    L.append("# never wedge it. AG EF DONE — branching-time recoverability, which is")
    L.append("# the sound liveness question to ask of an over-approximating model with")
    L.append("# no fairness constraint on sdram_burst.")
    L.append('name = "never_wedged"')
    L.append('formula = "nu Z . ((mu Y . (DONE || (<> Y))) && ([] Z))"')
    L.append('over = "FabricSys"')
    return "\n".join(L) + "\n"


def main():
    args = [a for a in sys.argv[1:] if a != "--strict-schedule"]
    backfill = "--strict-schedule" not in sys.argv[1:]
    if len(args) != 2 or args[0] not in ("v1", "v2", "v3", "v4"):
        sys.stderr.write(
            "usage: generate.py <v1|v2|v3|v4> <out_dir> [--strict-schedule]\n")
        return 2
    variant, out_dir = args[0], args[1]
    names, edges, initial = explore(variant, backfill)
    d = os.path.join(out_dir, variant)
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, "fabric_%s.ctxdsl" % variant), "w") as f:
        f.write(emit_ctxdsl(variant, names, edges, initial))
    with open(os.path.join(d, "verify.toml"), "w") as f:
        f.write(emit_verify_toml(variant, names))
    print("%s: %d states, %d transitions -> %s" % (variant, len(names), len(edges), d))
    return 0


if __name__ == "__main__":
    sys.exit(main())
