#!/usr/bin/env bash
# The flow-control loop: synthesize the kernel, assemble it with the datapath
# sub-modules, and verify the whole system. Needs `mununu` on PATH + the
# mununu-sva docker image + the `mununu-target` volume (warm cargo cache).
set -euo pipefail
cd "$(dirname "$0")"

echo "[1/3] synthesize the flow-control kernel (sound GR(1))"
mununu --quiet context synth flow_ctrl.tlsf --adapter tlsf \
  --controller-mode gr1 --automaton _ --emit-sv flow_ctrl.sv

echo "[2/3] assemble producer -> FIFO -> consumer + the synthesized kernel"
cat producer.sv consumer.sv fifo.sv flow_ctrl.sv system.sv > _build.sv

echo "[3/3] verify the whole system"
docker run --rm -v "$PWD":/work mununu-sva bash /work/verify.sh
rm -f _build.sv
