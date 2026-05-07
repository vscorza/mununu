#!/usr/bin/env bash
# bench_diff.sh — compare current Criterion run against a saved baseline.
#
# Usage:
#   scripts/bench_diff.sh <baseline-name> [--threshold-percent N] [--robust]
#
# Reads target/criterion/**/<baseline-name>/estimates.json (baseline) and
# target/criterion/**/new/estimates.json (current run), computes per-bench
# median ratio, and exits non-zero if any benchmark regressed by more than
# the threshold (default 10%).
#
# --robust uses sample.json (per-iteration measurements) and reports
# additional distribution-level signals: median + IQR ratio + Mann-Whitney
# U test p-value. Robust mode is more tolerant of bimodal distributions
# caused by cache-state shifts; it flags only regressions that are both
# statistically significant (p<0.01) AND beyond the threshold.
#
# See notebook/BENCH_POLICY.md "Regression mitigation" for the full protocol.
#
# Designed to run in CI after `cargo bench --baseline <baseline-name>`.

set -euo pipefail

BASELINE="${1:-main}"
THRESHOLD=10
ROBUST=0

shift || true
while [ $# -gt 0 ]; do
    case "$1" in
        --threshold-percent) THRESHOLD="$2"; shift 2 ;;
        --robust) ROBUST=1; shift ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

CRIT_DIR="${CARGO_TARGET_DIR:-target}/criterion"
if [ ! -d "$CRIT_DIR" ]; then
    echo "error: $CRIT_DIR does not exist; run cargo bench first" >&2
    exit 2
fi

python3 - "$CRIT_DIR" "$BASELINE" "$THRESHOLD" "$ROBUST" <<'PY'
import json, os, sys
from pathlib import Path
from statistics import median

crit_dir = Path(sys.argv[1])
baseline = sys.argv[2]
threshold = float(sys.argv[3])
robust = sys.argv[4] == '1'

regressions = []
improvements = []
neutral = []
missing = []
significance_skipped = []

def mann_whitney_p(a, b):
    """Two-sided Mann-Whitney U test p-value via normal approximation.
    Avoids scipy dependency; accurate enough for regression triage on
    100+ samples (Criterion default). Returns None if samples too small."""
    n1, n2 = len(a), len(b)
    if n1 < 8 or n2 < 8:
        return None
    combined = sorted([(v, 0) for v in a] + [(v, 1) for v in b])
    # Average ranks for ties.
    ranks = [0.0] * len(combined)
    i = 0
    while i < len(combined):
        j = i
        while j + 1 < len(combined) and combined[j+1][0] == combined[i][0]:
            j += 1
        avg = (i + j + 2) / 2.0
        for k in range(i, j+1):
            ranks[k] = avg
        i = j + 1
    r1 = sum(ranks[k] for k in range(len(combined)) if combined[k][1] == 0)
    u1 = r1 - n1 * (n1 + 1) / 2
    u2 = n1 * n2 - u1
    u = min(u1, u2)
    mu = n1 * n2 / 2
    sigma = (n1 * n2 * (n1 + n2 + 1) / 12) ** 0.5
    if sigma == 0:
        return 1.0
    z = (u - mu) / sigma
    # Two-sided p from normal approximation.
    from math import erf, sqrt
    p = 2 * (1 - 0.5 * (1 + erf(abs(z) / sqrt(2))))
    return p

for est_path in crit_dir.rglob('estimates.json'):
    if est_path.parent.name != 'new':
        continue
    bench_dir = est_path.parent.parent
    base_path = bench_dir / baseline / 'estimates.json'
    if not base_path.exists():
        missing.append(str(bench_dir.relative_to(crit_dir)))
        continue
    with open(est_path) as f:
        new_est = json.load(f)
    with open(base_path) as f:
        base_est = json.load(f)
    new_med = new_est['median']['point_estimate']
    base_med = base_est['median']['point_estimate']
    name = str(bench_dir.relative_to(crit_dir))

    # Prefer Criterion's own change/estimates.json if present — it carries
    # the bootstrap-on-means delta + 95% CI (Criterion's headline number)
    # AND the bootstrap median delta. Fall back to a recomputed median
    # ratio when change/ is absent (e.g., no baseline saved).
    change_path = bench_dir / 'change' / 'estimates.json'
    crit_mean_pct = None
    crit_mean_ci = None
    crit_median_pct = None
    if change_path.exists():
        try:
            with open(change_path) as f:
                change_est = json.load(f)
            crit_mean_pct = change_est['mean']['point_estimate'] * 100.0
            ci = change_est['mean']['confidence_interval']
            crit_mean_ci = (ci['lower_bound'] * 100.0, ci['upper_bound'] * 100.0)
            crit_median_pct = change_est['median']['point_estimate'] * 100.0
        except Exception:
            crit_mean_pct = None
            crit_mean_ci = None
            crit_median_pct = None

    # `pct` is the headline number used for threshold gating. Prefer
    # Criterion's bootstrap-on-means; fall back to median ratio.
    if crit_mean_pct is not None:
        pct = crit_mean_pct
    else:
        ratio = new_med / base_med if base_med > 0 else float('inf')
        pct = (ratio - 1.0) * 100.0

    # Robust gate: only flag if Mann-Whitney p<0.01.
    # Criterion's sample.json stores `times` = total wall time for `iters`
    # iterations of the bench. iters varies across samples (Criterion
    # ramps it up during measurement). Compare per-iteration times, not
    # raw times, otherwise MW sees the iters ramp instead of the bench.
    p_value = None
    if robust:
        new_samples_path = bench_dir / 'new' / 'sample.json'
        base_samples_path = bench_dir / baseline / 'sample.json'
        if new_samples_path.exists() and base_samples_path.exists():
            try:
                with open(new_samples_path) as f:
                    new_data = json.load(f)
                with open(base_samples_path) as f:
                    base_data = json.load(f)
                new_per_iter = [
                    t / i for t, i in zip(new_data['times'], new_data['iters'])
                    if i > 0
                ]
                base_per_iter = [
                    t / i for t, i in zip(base_data['times'], base_data['iters'])
                    if i > 0
                ]
                p_value = mann_whitney_p(base_per_iter, new_per_iter)
            except Exception:
                p_value = None
        if p_value is None:
            significance_skipped.append(name)

    entry = (name, base_med, new_med, pct, p_value, crit_mean_ci, crit_median_pct)
    if pct > threshold:
        # In robust mode, demote to neutral if p>=0.01 (no significant difference).
        if robust and p_value is not None and p_value >= 0.01:
            neutral.append(entry)
        else:
            regressions.append(entry)
    elif pct < -threshold:
        improvements.append(entry)
    else:
        neutral.append(entry)

def fmt(entries):
    for entry in sorted(entries, key=lambda e: -abs(e[3])):
        name, b, n, pct, p_value, ci, median_pct = entry
        suffix_parts = []
        if ci is not None:
            suffix_parts.append(f"CI[{ci[0]:+.1f}%, {ci[1]:+.1f}%]")
        if median_pct is not None:
            suffix_parts.append(f"med {median_pct:+.1f}%")
        if p_value is not None:
            suffix_parts.append(f"p={p_value:.3f}")
        suffix = "  " + "  ".join(suffix_parts) if suffix_parts else ""
        print(f"  {pct:+7.1f}%   {name}  (baseline {b:.3e} → current {n:.3e}){suffix}")

mode_label = "robust (Mann-Whitney p<0.01)" if robust else "median ratio"
print(f"baseline: {baseline}    threshold: ±{threshold}%    mode: {mode_label}\n")
print(f"REGRESSIONS ({len(regressions)}):")
fmt(regressions)
print(f"\nIMPROVEMENTS ({len(improvements)}):")
fmt(improvements)
print(f"\nNEUTRAL ({len(neutral)}):")
fmt(neutral)
if missing:
    print(f"\nMISSING BASELINE ({len(missing)}):")
    for m in missing:
        print(f"  {m}")
if significance_skipped:
    print(f"\nSIGNIFICANCE TESTING SKIPPED (sample.json absent or unparseable, {len(significance_skipped)}):")
    for m in significance_skipped[:10]:
        print(f"  {m}")

sys.exit(1 if regressions else 0)
PY
