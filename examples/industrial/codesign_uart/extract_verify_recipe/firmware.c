/*
 * firmware.c — end-to-end extract-and-verify recipe.
 *
 * Same `uart_send_byte` shape as the parent `firmware.c`, instrumented
 * with `MUNUNU_STATE("…")` markers (phase L9 / gap 1, PR #51) so the
 * synthesised CTXDSL automaton's states have meaningful names instead
 * of `S0`/`S1`/`S2`/`S3`.
 *
 * This file is consumed end-to-end by `run.sh` in this directory:
 *   1. `mununu codesign extract-c` lifts the function via clang -O0 -emit-llvm.
 *   2. `jq` pulls the synthesised CTXDSL `automaton { ... }` fragment out of
 *      the JSON output.
 *   3. The fragment is spliced into `verify.template.ctxdsl` (which carries
 *      a hand-authored protocol-spec peripheral + composition + formulas).
 *   4. `mununu context eval` evaluates the formulas over the composition.
 *
 * The state names (`Polling`, `Ready`, `Sending`) become first-class CTXDSL
 * identifiers — the formulas can be written against them by name (e.g.
 * `mu X. (Sending || <> X)` instead of guessing which numeric state is which).
 */

#include <stdint.h>
#include "mununu_annotations.h"

#define UART ((volatile UART_TypeDef *)0x40010000u)

typedef struct {
    union { volatile uint32_t reg; struct { uint32_t tx_start:1; uint32_t enable:1; uint32_t reserved:30; } bit; } CTRL;
    union { volatile uint32_t reg; struct { uint32_t tx_busy:1;  uint32_t rx_ready:1; uint32_t reserved:30; } bit; } STATUS;
    union { volatile uint32_t reg; uint8_t byte; } DATA;
} UART_TypeDef;

void uart_send_byte(uint8_t byte) {
    MUNUNU_STATE("Polling");
    while (UART->STATUS.bit.tx_busy)
        ;
    MUNUNU_STATE("Ready");
    UART->DATA.byte = byte;
    MUNUNU_STATE("Sending");
    UART->CTRL.bit.tx_start = 1;
}
