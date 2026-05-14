/*
 * firmware.c — the C-side of the Document C §C.4 codesign example.
 *
 * The hand-authored CTXDSL automaton at `firmware.ctxdsl` was written
 * from this source. As of M4 / Doc C phase L3, `mununu codesign
 * extract-c` walks this file via the LLVM-IR backend and emits a
 * matching CTXDSL automaton with polling-loop detection and
 * bit-field RMW collapsing — see `extract-c-demo.sh` next to this
 * file.
 *
 * The `UART_TypeDef` shape mirrors the canonical Cortex-M HAL
 * convention (`UART->CTRL.bit.tx_start`, `UART->STATUS.bit.tx_busy`,
 * `UART->DATA.byte`). The base address 0x40010000 + offsets 0/4/8
 * match `register_map.json`.
 *
 * ILLUSTRATIVE — this is not derived from any specific vendor SDK;
 * the shape is industry-standard but the addresses + naming are
 * mununu-internal.
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

/* Canonical STM32/NXP/Microchip HAL convention: the peripheral
 * base pointer is a `#define` macro that resolves to a literal
 * address at compile time. The LLVM-IR backend doesn't care
 * which spelling the C source uses — clang's IR has the address
 * either way. See `firmware_macro.c` for the macro-form variant
 * tested side-by-side for parity. */
#define UART ((volatile UART_TypeDef *)0x40010000u)

/**
 * @brief Send one byte through the UART peripheral.
 *
 * Polls STATUS.tx_busy until the peripheral is idle, writes the byte
 * to DATA, then raises CTRL.tx_start so the peripheral picks up the
 * transaction.
 *
 * @mununu_guarantee G(wr_data_byte -> eventually wr_ctrl_tx_start)
 * @mununu_assume G(reset -> eventually rd_status_tx_busy)
 */
void uart_send(uint8_t byte) {
    while (UART->STATUS.bit.tx_busy)
        ; /* poll until peripheral is idle */
    UART->DATA.byte = byte;
    UART->CTRL.bit.tx_start = 1;
}
