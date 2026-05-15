/*
 * example_9_state_name_annotations.c — Plan phase L9 (gap 1).
 *
 * Closes verification-gap (1) from the C-extraction audit: the
 * synthesised CTXDSL automaton names its states `S0`, `S1`, `S2`, …
 * by default, which makes it hard for the user to write properties
 * against meaningful state names (e.g.
 * `mu X. (state = Sending) -> [c] X`).
 *
 * The fix is in-source state-name annotations via the
 * `MUNUNU_STATE("Name")` macro declared in
 * `crates/mununu-core/cmsis-stubs/mununu_annotations.h`. Clang lowers
 * each invocation to a `call void @__mununu_state(ptr noundef @.str)`
 * which mununu's IR walker recognises, records, and propagates as a
 * `source_state_hint` on the very next emitted register access.
 *
 * The synthesised CTXDSL substitutes the hint for the default state
 * name when emitting the source state of each transition. The
 * terminal state (after the last access) keeps its default `S<N>`
 * name unless a closing marker appears after the last access.
 *
 * SOUNDNESS — the marker is a pure metadata side-channel:
 *   - clang treats `__mununu_state` as an extern function with no
 *     side effects on register state (it never resolves to a real
 *     symbol; mununu's extractor consumes the call and emits no
 *     register access for it);
 *   - state names are syntactic identifiers in the synthesised
 *     CTXDSL, not semantic predicates — incorrect naming is a
 *     readability issue, not a soundness issue;
 *   - if the user's MUNUNU_STATE calls don't actually appear at
 *     basic-block boundaries the IR walker sees, mununu falls back
 *     to `S<N>` defaults silently.
 *
 * Expected lift:
 *   transition Polling -> Loop0 on label rd_status_tx_busy;
 *   transition Loop0   -> Loop0 on label rd_status_tx_busy;
 *   transition Loop0   -> Ready on label rd_status_tx_busy;
 *   transition Ready   -> Sending on label wr_data_byte;
 *   transition Sending -> S4    on label wr_ctrl_tx_start;
 */

#include <stdint.h>
#include "mununu_annotations.h"

#define UART ((volatile UART_TypeDef *)0x40010000u)

typedef struct {
    union { volatile uint32_t reg; struct { uint32_t tx_start:1; uint32_t enable:1; uint32_t reserved:30; } bit; } CTRL;
    union { volatile uint32_t reg; struct { uint32_t tx_busy:1; uint32_t rx_ready:1; uint32_t reserved:30; } bit; } STATUS;
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
