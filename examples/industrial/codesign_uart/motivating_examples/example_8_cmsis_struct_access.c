/*
 * example_8_cmsis_struct_access.c — Plan phase L8 demonstration.
 *
 * Firmware C using the canonical CMSIS-DEVICE vendor idiom:
 * `UART->FIELD` struct-member access via an SVD-derived header.
 * Compiled with `--cmsis-stubs` so clang resolves __IO, __NOP,
 * etc. against mununu's bundled minimal CMSIS shims.
 *
 * SOUNDNESS — the C source assumes the SVD-derived header is on
 * the include path (e.g. via `--include path/to/uart_header_dir`).
 * Generate the header by running:
 *   mununu codesign emit-cmsis-header --svd <file> --peripheral UART \
 *       > uart.h
 *
 * For mununu's own validation we already have the equivalent
 * register-map JSON sidecar at `register_map.json` — the
 * SVD-derived header would emit the same struct layout from it.
 *
 * The point of L8 isn't that this C source is *new* (we have
 * Example 1 / 3 covering similar idioms via firmware.c's
 * #define-based macro form). The point is that L8 lets a user
 * starting from a VENDOR SDK paste their familiar `NRF_TWIM0->X`
 * code in and have mununu lift it directly — no manual
 * `#define UART ((volatile T *)0xADDR)` rewrite required.
 */

#include <stdint.h>
#include "uart_cmsis.h"

void uart_send_byte(uint8_t byte) {
    while (UART->STATUS.bit.tx_busy)
        ;
    UART->DATA.byte = byte;
    UART->CTRL.bit.tx_start = 1;
}
