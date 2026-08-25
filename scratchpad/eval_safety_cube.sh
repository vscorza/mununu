#!/usr/bin/env bash
# Evaluate verify-safety (KMTS 3-valued cube + inductive-invariant discovery) on an HWMCC bv subset.
# Reports decided/unknown + a SOUNDNESS GATE: any definite verdict contradicting HWMCC ground truth.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
BIN=target/debug/mununu
export LIBRARY_PATH="/usr/local/opt/z3/lib"
BENCH="$HOME/hwmcc20-bench/flat"
TIMEOUT="${TIMEOUT:-25}"

# Ground truth per benchmark: consensus of non-timeout solver statuses in the two competition CSVs.
# uns => safe (bad unreachable => our Holds); sat => unsafe (bad reachable => our Violated).
python3 - "$@" <<'PY'
import subprocess, sys, os, glob
home=os.path.expanduser("~/hwmcc20-bench")
bench=os.path.join(home,"flat")
BIN="target/debug/mununu"
TIMEOUT=int(os.environ.get("TIMEOUT","25"))

def truth_map():
    m={}
    for f in ("hwmcc20-bv-sat.csv","hwmcc20-bv-uns.csv"):
        p=os.path.join(home,f)
        if not os.path.exists(p): continue
        for line in open(p):
            t=line.strip().split(";")
            if not t or t[0]=="benchmark": continue
            name=t[0]; votes={}
            # rows are benchmark; then repeated (solver,status,bound,real,time,mem)
            i=1
            while i+1 < len(t):
                st=t[i+1]
                if st in ("uns","sat"): votes[st]=votes.get(st,0)+1
                i+=6
            if votes:
                # any 'sat' with no 'uns' => sat; any 'uns' with no 'sat' => uns; both => conflict(skip)
                if "sat" in votes and "uns" not in votes: m[name]="unsafe"
                elif "uns" in votes and "sat" not in votes: m[name]="safe"
                # if both appear, leave unknown (shouldn't happen for sound solvers)
    return m

TRUTH=truth_map()

# Curated tractable subset (smaller / structured — the cube's best chance), by basename.
subset=[
 "vcegar_QF_BV_ar","paper_v3","mul1","mul2","mul3","mul7","mul9",
 "vis_arrays_am2910_p2","vcegar_QF_BV_itc99_b13_p10","gen44","gen43",
 "circular_pointer_top_w64_d8_e0","circular_pointer_top_w128_d8_e0",
 "shift_register_top_w16_d8_e0","shift_register_top_w32_d8_e0","shift_register_top_w64_d8_e0",
 "miim","krebs.3.prop1-func-interl","anderson.3.prop1-back-serstep","cal2",
]
def norm(n): return n[:-len(".btor2")] if n.endswith(".btor2") else n

rows=[]; sound_violations=[]
dec=0; unk=0; err=0
for name in subset:
    path=os.path.join(bench,name+".btor2")
    if not os.path.exists(path):
        rows.append((name,"MISSING","",""))
        continue
    try:
        out=subprocess.run([BIN,"btor2","verify-safety",path],capture_output=True,text=True,timeout=TIMEOUT)
        v="parse-err"
        for ln in out.stdout.splitlines():
            if '"verdict"' in ln:
                v=ln.split('"')[3]
        if out.returncode!=0 and v=="parse-err":
            v="err"
    except subprocess.TimeoutExpired:
        v="timeout"
    # truth lookup — try exact + some name normalizations
    tr=TRUTH.get(name) or TRUTH.get(name.replace(".","_")) or "?"
    verdict_class = {"holds":"safe","violated":"unsafe"}.get(v)
    agree=""
    if verdict_class and tr in ("safe","unsafe"):
        if verdict_class==tr: agree="AGREE"; dec+=1
        else: agree="**CONTRADICT**"; sound_violations.append((name,v,tr)); dec+=1
    elif v in ("holds","violated"):
        agree="decided(no-truth)"; dec+=1
    elif v in ("unknown",): unk+=1; agree="abstain"
    else: err+=1; agree=v
    rows.append((name,v,tr,agree))

print(f"{'benchmark':40s} {'verdict':10s} {'truth':8s} {'check'}")
print("-"*80)
for name,v,tr,a in rows:
    print(f"{name:40s} {str(v):10s} {str(tr):8s} {a}")
print("-"*80)
print(f"decided={dec}  unknown/abstain={unk}  err/timeout={err}  of {len(subset)}")
print(f"SOUNDNESS VIOLATIONS (definite verdict contradicting HWMCC truth): {len(sound_violations)}")
for s in sound_violations: print("  ⚠⚠", s)
PY
