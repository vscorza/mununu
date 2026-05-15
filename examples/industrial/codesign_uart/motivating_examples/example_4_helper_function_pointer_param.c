/*
 * example_4_helper_function_pointer_param.c — Plan §"Motivating examples" #4.
 *
 * Idiomatic firmware factoring: a helper function that takes the
 * peripheral pointer as a parameter and does some register work.
 * The caller passes the peripheral base; the callee dereferences
 * the parameter.
 *
 * This file is compiled end-to-end via real clang. Phase L5.5
 * (pointer-parameter alias tracking) lets the extractor follow the
 * caller's `UART` argument through the callee's alloca-store-load
 * round-trip:
 *
 *   call void @uart_wait_idle(ptr noundef <UART resolved>)
 *   define void @uart_wait_idle(ptr %0) {
 *     %2 = alloca ptr               <- callee's stack slot for %0
 *     store ptr %0, ptr %2          <- parameter spilled to alloca
 *     ...
 *     %4 = load ptr, ptr %2         <- parameter reloaded
 *     %5 = gep ..., ptr %4, ...     <- field access on parameter
 *     %6 = load volatile i32, ptr %5
 *   }
 *
 * The resolver chases `%4` back to the store-to-alloca, then back
 * to parameter `%0`, then looks up `%0` in the L5.5 parameter
 * bindings, and finds the caller's resolved `UART` address.
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
            uint32_t reserved : 31;
        } bit;
    } STATUS;
    union {
        uint32_t reg;
        uint8_t  byte;
    } DATA;
} UART_TypeDef;

#define UART ((volatile UART_TypeDef *)0x40010000u)

/* Helper: read STATUS through a parameter-passed peripheral pointer. */
static void uart_read_status(volatile UART_TypeDef *u) {
    (void)u->STATUS.bit.tx_busy;
}

void uart_send(uint8_t byte) {
    uart_read_status(UART);
    UART->DATA.byte = byte;
}
