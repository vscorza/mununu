#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
RUN_ID="$(date +%Y%m%d-%H%M%S)-$(cd "$ROOT_DIR" && git rev-parse --short HEAD)"
RESULTS_ROOT="$ROOT_DIR/bench_results"
RESULTS_DIR="$RESULTS_ROOT/$RUN_ID"
LATEST_LINK="$RESULTS_ROOT/latest"
OS_TYPE=$(uname -s)
MEASUREMENT_TIME=${MEASUREMENT_TIME:-60}

mkdir -p "$RESULTS_DIR"
rm -rf "$LATEST_LINK"
ln -s "$RESULTS_DIR" "$LATEST_LINK" 2>/dev/null || true

if [[ "${FRAME_POINTERS:-1}" == "1" ]]; then
    export RUSTFLAGS="${RUSTFLAGS:-} -C force-frame-pointers=yes"
    echo "🧵 Frame pointers enabled (RUSTFLAGS=$RUSTFLAGS)"
fi

echo "🔧 Building release artifacts..."
cargo bench --no-run --bench clts_composition >/dev/null

BENCH_BINARY=$(ls -t "$ROOT_DIR/target/release/deps"/clts_composition-* 2>/dev/null | grep -v '\.dSYM' | head -n 1)
if [[ -z "$BENCH_BINARY" ]]; then
    echo "❌ Unable to locate clts_composition bench binary. Aborting." >&2
    exit 1
fi

function run_bench() {
    local bench_name=$1
    local output_prefix=$2
    local bench_log="$RESULTS_DIR/${output_prefix}.bench.txt"

    echo "📈 Profiling $bench_name"
    if [[ "$OS_TYPE" == "Linux" ]] && command -v perf >/dev/null 2>&1; then
        perf record --call-graph=dwarf --output="$RESULTS_DIR/${output_prefix}.perf" \
            env CRITERION_MEASUREMENT_TIME="$MEASUREMENT_TIME" \
            "$BENCH_BINARY" --bench "$bench_name" >"$bench_log" 2>&1
        perf report --input="$RESULTS_DIR/${output_prefix}.perf" --stdio > "$RESULTS_DIR/${output_prefix}.perf.txt"
        gprofpp "$RESULTS_DIR/${output_prefix}.perf" > "$RESULTS_DIR/${output_prefix}.perf.filtered" 2>/dev/null || true
    elif [[ "$OS_TYPE" == "Darwin" ]] && command -v sample >/dev/null 2>&1; then
        (env CRITERION_MEASUREMENT_TIME="$MEASUREMENT_TIME" "$BENCH_BINARY" --bench "$bench_name" >"$bench_log" 2>&1) &
        local bench_pid=$!
        sleep 1
        if ps -p "$bench_pid" >/dev/null 2>&1; then
            sample "$bench_pid" 10 -file "$RESULTS_DIR/${output_prefix}.sample.txt" >/dev/null 2>&1
            local sample_status=$?
            if [[ $sample_status -ne 0 ]]; then
                echo "⚠️  'sample' requires elevated privileges (try running 'sudo scripts/profile.sh')." >&2
                rm -f "$RESULTS_DIR/${output_prefix}.sample.txt"
            else
                python3 "$ROOT_DIR/scripts/summarize_sample.py" "$RESULTS_DIR/${output_prefix}.sample.txt" "$RESULTS_DIR/${output_prefix}.summary.txt" 2>/dev/null || true
            fi
        else
            echo "ℹ️  Benchmark finished before 'sample' could attach; rerun with a longer benchmark or invoke the script with 'sudo'." >&2
        fi
        wait "$bench_pid" || true
    else
        if [[ "$OS_TYPE" == "Linux" ]]; then
            echo "⚠️  'perf' not found; falling back to bench run without profiling." >&2
        elif [[ "$OS_TYPE" == "Darwin" ]]; then
            echo "⚠️  'sample' not found; falling back to bench run without profiling." >&2
        else
            echo "⚠️  Unsupported OS ($OS_TYPE); running bench without profiling." >&2
        fi
        env CRITERION_MEASUREMENT_TIME="$MEASUREMENT_TIME" "$BENCH_BINARY" --bench "$bench_name" | tee "$bench_log"
    fi
}

run_bench clts_build_large clts_build_large
run_bench clts_build_heavy clts_build_heavy
run_bench compose clts_composition
run_bench clts_heavy_light_composition clts_heavy_light_composition

RUN_README="$RESULTS_DIR/README.md"
COMMIT_HASH="$(cd "$ROOT_DIR" && git rev-parse --short HEAD)"
RUN_DATE="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

{
    cat <<EOF
# Profiling Run $RUN_ID
- Date (UTC): $RUN_DATE
- Commit: $COMMIT_HASH
- Measurement time (seconds): $MEASUREMENT_TIME

## Benchmarks Captured
EOF

    for bench in clts_build_large clts_build_heavy compose clts_heavy_light_composition; do
        log="$RESULTS_DIR/${bench}.bench.txt"
        summary="$RESULTS_DIR/${bench}.summary.txt"
        if [[ -f "$log" ]]; then
            echo "### $bench"
            echo
            echo "Log excerpt:"
            head -n 10 "$log"
            echo
            if [[ -f "$summary" ]]; then
                echo "Top frames:"
                head -n 10 "$summary"
            else
                echo "Top frames: (summary not available)"
            fi
            echo
        fi
    done

    cat <<'EOF'
## Proposed Optimisations
- Capture frame-pointer profiles for `composition::ensure_product_state` to confirm allocator/hash pressure.
- Prototype arena-backed product state caches (struct-of-arrays) so synchronous joins avoid repeated `HashMap::insert` and `BTreeSet` cloning.
- Continue trimming `CltsBuilder::build` hotspots: reuse staging buffers for `Iterator::collect` and variable merges to keep the ~2k-sample cost from climbing.

EOF
} >"$RUN_README"

echo "✅ Profiling complete. Results stored in $RESULTS_DIR"
