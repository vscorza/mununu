/*
 * example_5_isr_with_main_thread.c — Plan §"Motivating examples" #5.
 *
 * An Interrupt Service Routine that runs asynchronously relative
 * to the main-thread firmware. The `@mununu_isr` annotation opts
 * the handler in; the extractor must:
 *   1. Detect the annotation by scanning C source.
 *   2. Tag `UART_IRQHandler` as an ISR in the function summary.
 *   3. Emit a top-level CTXDSL `compositions { ... }` block
 *      composing the main thread asynchronously with each ISR.
 *
 * This file is compiled end-to-end via real clang. ISR detection
 * is annotation-only — there are no naming-convention defaults
 * (`*_IRQHandler` is NOT treated specially without the annotation).
 *
 * ILLUSTRATIVE — not derived from any vendor SDK.
 */

#include <stdint.h>

typedef struct {
    union {
        uint32_t reg;
        struct {
            uint32_t tx_start : 1;
            uint32_t reserved : 31;
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

/* Main-thread driver — touches DATA. */
void uart_send(uint8_t byte) {
    UART->DATA.byte = byte;
    UART->CTRL.bit.tx_start = 1;
}

/**
 * @brief UART receive ISR.
 *
 * Reads STATUS for the rx_ready flag, then DATA. Runs
 * asynchronously to `uart_send` — the verifier must see them
 * composed via `compositions { Codesign = asynchronous {...} }`.
 *
 * @mununu_isr
 */
void UART_IRQHandler(void) {
    if (UART->STATUS.bit.rx_ready) {
        /* Touch DATA so the extracted automaton has at least
         * one access. */
        (void)UART->DATA.byte;
    }
}
