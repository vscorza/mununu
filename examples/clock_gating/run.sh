#!/usr/bin/env bash
# The clock-gating loop: synthesize the kernel, assemble it with the gated
# datapath, and verify the whole system. Needs `mununu` on PATH + the mununu-sva
# docker image.
set -euo pipefail
cd "$(dirname "$0")"
echo "[1/3] synthesize the clock-gating kernel (sound GR(1))"
mununu --quiet context synth clock_gate.tlsf --adapter tlsf \
  --controller-mode gr1 --automaton _ --emit-sv clock_gate.sv
echo "[2/3] assemble the gated domain + the synthesized kernel"
cat gated_domain.sv clock_gate.sv system.sv > _build.sv
echo "[3/3] verify the whole system"
docker run --rm -v "$PWD":/work mununu-sva bash /work/verify.sh
rm -f _build.sv
