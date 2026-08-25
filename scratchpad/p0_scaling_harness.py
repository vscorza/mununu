#!/usr/bin/env python3
# P0 — pin the eager KMTS-cube wall. Generate a design with N in-cone guard atoms (→ N auto-seeded cube
# predicates) over a width-W datapath, run verify-recoverability (which forces the eager SmtAllPairs may +
# SmtHyperMust must), and record wall-clock vs (N, W). If time grows ~exponentially in N, the all-pairs
# O(2^{2|P|}) is the wall; if it grows with W at fixed N, per-query datapath cost compounds it.
import subprocess, time, os, sys
BIN = "target/debug/mununu"
os.environ["LIBRARY_PATH"] = os.environ.get("LIBRARY_PATH", "/usr/local/opt/z3/lib")

def design(n, w):
    # ctrl returns to idle when `data` (width w, increments) hits ANY of n distinct constants c_k = 3k+1.
    # All n `eq(data,c_k)` are in ctrl's next-state cone → all auto-seeded → |P| ≈ n (+ good + ctrl).
    L = [f"1 sort bitvec 1", f"2 sort bitvec {w}",
         "3 state 1 ctrl", "4 state 2 data",
         "5 one 1", "6 zero 2", "7 one 2",
         "8 init 1 3 5",   # ctrl = 1 (busy)
         "9 init 2 4 6",   # data = 0
         "10 add 2 4 7", "11 next 2 4 10"]  # data' = data + 1
    nid = 20
    eq_nids = []
    for k in range(n):
        c = 3 * k + 1
        L.append(f"{nid} constd 2 {c}"); cn = nid; nid += 1
        L.append(f"{nid} eq 1 4 {cn}");  eq_nids.append(nid); nid += 1
    # any_hit = OR of the eq's
    acc = eq_nids[0]
    for e in eq_nids[1:]:
        L.append(f"{nid} or 1 {acc} {e}"); acc = nid; nid += 1
    L.append(f"{nid} not 1 {acc}"); nothit = nid; nid += 1      # not any hit
    L.append(f"{nid} and 1 3 {nothit}"); nxt = nid; nid += 1    # ctrl' = ctrl AND not any hit
    L.append(f"{nid} next 1 3 {nxt}")
    return "\n".join(L) + "\n"

def run(n, w, timeout=90):
    src = f"/tmp/p0_n{n}_w{w}.btor2"
    open(src, "w").write(design(n, w))
    t0 = time.time()
    try:
        out = subprocess.run([BIN, "btor2", "verify-recoverability", src, "--target", "ctrl == 0"],
                             capture_output=True, text=True, timeout=timeout)
        dt = time.time() - t0
        v = next((ln.split('"')[3] for ln in out.stdout.splitlines() if '"verdict"' in ln), "?")
        return dt, v
    except subprocess.TimeoutExpired:
        return timeout, "TIMEOUT"

print(f"{'N(preds)':>8} {'W(width)':>8} {'verdict':>9} {'wall_s':>9}")
print("-" * 40)
# Sweep |P| at fixed width, then width at fixed |P|.
for n in [1, 2, 3, 4, 5, 6, 7, 8]:
    dt, v = run(n, 16)
    print(f"{n:>8} {16:>8} {v:>9} {dt:>9.2f}")
print("-" * 40)
for w in [8, 16, 32, 48, 64]:
    dt, v = run(6, w)
    print(f"{6:>8} {w:>8} {v:>9} {dt:>9.2f}")
