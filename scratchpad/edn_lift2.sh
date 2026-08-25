#!/usr/bin/env bash
set -o pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=error
export MUNUNU_KEEP_YOSYS_TMP=1
cd /work
BIN=/cargo-target/release/mununu
echo "=== free verdict state_q==44 (BootUniAckWait) ==="
"$BIN" sv verify-recoverability examples/recoverability/edn_boot_sm.sv --top edn_boot_sm --frontend slang --target "state_q == 44" 2>&1 | sed -n '/^{/,$p' | head -6
echo "=== grab btor2 ==="
F=$(ls -t /tmp/*/*.btor2 /tmp/mununu-*/*.btor 2>/dev/null | head -1); echo "btor: $F"
if [ -n "$F" ]; then cp "$F" /work/scratchpad/edn_boot_sm.btor; echo "state cells:"; grep -E "^[0-9]+ state " scratchpad/edn_boot_sm.btor | head; echo "inputs:"; grep -E "^[0-9]+ input " scratchpad/edn_boot_sm.btor; fi
