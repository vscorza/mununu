/*
 * example_2_typecast_register_access.c — Plan §"Motivating examples" #2.
 *
 * The Zephyr / FreeRTOS BSP convention: a peripheral register
 * accessed via an inline `*(volatile uint32_t *)0xADDRESS` cast.
 * No struct, no global, no macro — just a literal address.
 *
 * This file is compiled end-to-end via real clang in
 * `validate-motivating-examples.sh` and consumed by
 * `mununu codesign extract-c`. The extractor must recognise the
 * inline `inttoptr` in the resulting IR and match the literal
 * address against the register map's
 * `[base + offset, base + offset + width]` window.
 *
 * ILLUSTRATIVE — not derived from any vendor SDK; the address
 * 0x40010004 corresponds to UART_LITE STATUS in the example
 * register-map.
 */

#include <stdint.h>

void uart_status_clear(void) {
    /* Read STATUS, clear it. The IR will emit:
     *   store volatile i32 1, ptr inttoptr (i64 1073807364 to ptr)
     * The matcher resolves the inline inttoptr to the STATUS
     * register's address-range window. */
    *(volatile uint32_t *)0x40010004 = 1;
}
