#!/usr/bin/env bash
# Agentic-RTL loop demo: synthesize a control kernel + verify the whole system.
set -euo pipefail
cd "$(dirname "$0")"
MUNUNU="${MUNUNU:-mununu}"
echo "== 1. Synthesize the arbiter kernel from arbiter.tlsf (sound GR(1)) =="
"$MUNUNU" --quiet context synth arbiter.tlsf --adapter tlsf \
  --controller-mode gr1 --automaton _ --emit-sv gr1_controller.sv
echo "   wrote gr1_controller.sv"
echo "== 2. Assemble the system (masters + synthesized arbiter + monitors) =="
cat master.sv system.sv gr1_controller.sv > _build.sv
echo "== 3. Verify the WHOLE system (yosys + btormc, mununu-sva image) =="
docker run --rm -v "$PWD":/work mununu-sva bash /work/verify.sh
rm -f _build.sv
echo "== all 'unsat' => the assembled system is verified by construction =="
