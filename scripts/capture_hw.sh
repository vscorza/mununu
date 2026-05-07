#!/usr/bin/env bash
# capture_hw.sh — write a hardware/software fingerprint for an experiment.
#
# Usage: scripts/capture_hw.sh > experiments/EXP-NNNN/hw-fingerprint.txt
#
# Captures CPU model, cache topology, RAM, kernel, container/host, governor,
# turbo state, hyperthreading, NUMA, plus toolchain version. Cross-platform
# (Linux/macOS); falls back to "unavailable" rather than failing.

set -euo pipefail

emit() {
    printf '%-28s %s\n' "$1" "$2"
}

section() {
    printf '\n--- %s ---\n' "$1"
}

emit "# captured" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
emit "# host" "$(hostname 2>/dev/null || echo unknown)"
emit "# uname" "$(uname -a)"

section "OS"
if [ -r /etc/os-release ]; then
    grep -E '^(NAME|VERSION|ID|VERSION_ID)=' /etc/os-release || true
elif [ "$(uname -s)" = "Darwin" ]; then
    sw_vers 2>/dev/null || true
fi

section "CPU"
case "$(uname -s)" in
    Linux)
        if command -v lscpu >/dev/null 2>&1; then
            lscpu | grep -E '^(Architecture|Model name|CPU\(s\)|Thread\(s\)|Core\(s\)|Socket\(s\)|CPU max MHz|CPU min MHz|L1d|L1i|L2|L3|Flags|Vendor ID|CPU family|Model:|Stepping)'
        fi
        if [ -r /proc/cpuinfo ]; then
            grep -m1 -E '^(model name|cpu MHz|cpu cores|microcode)' /proc/cpuinfo || true
        fi
        ;;
    Darwin)
        sysctl -n machdep.cpu.brand_string 2>/dev/null && emit "model.brand" "$(sysctl -n machdep.cpu.brand_string)"
        for k in hw.ncpu hw.physicalcpu hw.logicalcpu hw.cpufamily hw.cputype hw.cpusubtype \
                 hw.l1icachesize hw.l1dcachesize hw.l2cachesize hw.l3cachesize \
                 hw.cachelinesize hw.pagesize hw.optional.arm.FEAT_FP16 hw.optional.arm.FEAT_DotProd; do
            v=$(sysctl -n "$k" 2>/dev/null || echo unavailable)
            emit "$k" "$v"
        done
        ;;
esac

section "Memory"
case "$(uname -s)" in
    Linux)
        if command -v free >/dev/null 2>&1; then free -h; fi
        if [ -r /proc/meminfo ]; then grep -E '^(MemTotal|MemAvailable|HugePages_Total)' /proc/meminfo; fi
        ;;
    Darwin)
        emit "hw.memsize" "$(sysctl -n hw.memsize 2>/dev/null || echo unavailable)"
        ;;
esac

section "Frequency / governor"
case "$(uname -s)" in
    Linux)
        if [ -r /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor ]; then
            cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null | sort -u | sed 's/^/governor: /'
        fi
        if [ -r /sys/devices/system/cpu/intel_pstate/no_turbo ]; then
            emit "intel_pstate.no_turbo" "$(cat /sys/devices/system/cpu/intel_pstate/no_turbo)"
        fi
        ;;
    Darwin)
        # Apple Silicon doesn't expose governor; record p-state cap if available.
        emit "powermetrics" "$(command -v powermetrics >/dev/null 2>&1 && echo available || echo unavailable)"
        ;;
esac

section "NUMA"
if command -v numactl >/dev/null 2>&1; then
    numactl --hardware 2>/dev/null || emit "numactl" "unavailable"
else
    emit "numactl" "unavailable"
fi

section "Container"
emit "container" "$(if [ -f /.dockerenv ]; then echo yes; else echo no; fi)"
if [ -f /.dockerenv ] && [ -r /etc/hostname ]; then
    emit "container.hostname" "$(cat /etc/hostname)"
fi

section "Toolchain"
emit "rustc" "$(rustc --version 2>/dev/null || echo unavailable)"
emit "cargo" "$(cargo --version 2>/dev/null || echo unavailable)"
emit "git.commit" "$(git rev-parse HEAD 2>/dev/null || echo unavailable)"
emit "git.branch" "$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo unavailable)"
emit "git.dirty" "$(if [ -n "$(git status --porcelain 2>/dev/null)" ]; then echo yes; else echo no; fi)"

if [ -f rust-toolchain.toml ]; then
    section "rust-toolchain.toml"
    cat rust-toolchain.toml
fi

section "Environment subset"
for v in CARGO_TARGET_DIR RUSTFLAGS RUST_LOG RAYON_NUM_THREADS; do
    emit "$v" "${!v:-<unset>}"
done
