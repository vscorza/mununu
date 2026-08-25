#!/usr/bin/env bash
set -o pipefail
export PATH=/usr/local/cargo/bin:/opt/oss-cad-suite/bin:$PATH
export CARGO_TARGET_DIR=/cargo-target
export RUST_LOG=error
export MUNUNU_KEEP_YOSYS_TMP=1
cd /work
BIN=/cargo-target/release/mununu
SRC=examples/verify/dc_opentitan_edn_main_sm/source
echo "=== free verdict state_q==44 (BootUniAckWait) ==="
"$BIN" sv verify-recoverability --design-dir "$SRC" --top edn_main_sm --frontend slang --target "state_q == 44" 2>&1 | sed -n '/^{/,$p' | head -6
echo "=== locate + copy the lifted btor2 ==="
F=$(ls -t /tmp/*/*.btor2 /tmp/mununu-*/*.btor 2>/dev/null | head -1)
echo "kept btor2: $F"
if [ -n "$F" ]; then cp "$F" /work/scratchpad/edn_main_sm.btor; echo "copied ($(wc -l < /work/scratchpad/edn_main_sm.btor) lines)"; echo "--- state cells ---"; grep -E "^[0-9]+ state " /work/scratchpad/edn_main_sm.btor | head; echo "--- 1-bit inputs (candidate strategy signals) ---"; grep -E "^[0-9]+ input " /work/scratchpad/edn_main_sm.btor | grep -iE "boot_req|auto_req|enable|ack|escal" | head; fi
