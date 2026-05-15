/*
 * example_6_multi_entry_driver.c — Plan §"Motivating examples" #6.
 *
 * A driver file with multiple public entry points the application
 * calls in arbitrary order. Phase L7's `--driver-mode` emits a
 * top-level `Driver` automaton that non-deterministically
 * dispatches to each entry via `call_<fn>` / `return_<fn>`
 * rendezvous labels.
 *
 * This file is compiled end-to-end via real clang. The
 * extractor must produce three per-function automata + one
 * Driver automaton + the appropriate dispatch transitions.
 *
 * ILLUSTRATIVE — not derived from any vendor SDK.
 */

#include <stdint.h>

typedef struct {
    union {
        uint32_t reg;
        struct {
            uint32_t tx_start : 1;
            uint32_t enable   : 1;
            uint32_t reserved : 30;
        } bit;
    } CTRL;
    union {
        uint32_t reg;
        struct {
            uint32_t tx_busy  : 1;
            uint32_t rx_ready : 1;
            uint32_t reserved : 30;
        } bit;
    } STATUS;
    union {
        uint32_t reg;
        uint8_t  byte;
    } DATA;
} UART_TypeDef;

#define UART ((volatile UART_TypeDef *)0x40010000u)

void uart_init(void) {
    /* Enable the peripheral. */
    UART->CTRL.bit.enable = 1;
}

void uart_send(uint8_t byte) {
    while (UART->STATUS.bit.tx_busy)
        ;
    UART->DATA.byte = byte;
    UART->CTRL.bit.tx_start = 1;
}

void uart_recv(uint8_t *out) {
    while (!UART->STATUS.bit.rx_ready)
        ;
    *out = UART->DATA.byte;
}
