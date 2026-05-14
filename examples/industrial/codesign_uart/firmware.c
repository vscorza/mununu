/*
 * firmware.c — the C-side of the Document C §C.4 codesign example.
 *
 * The hand-authored CTXDSL automaton at `firmware.ctxdsl` was written
 * from this source. As of M4 / Doc C task C5 slice 2.b, `mununu
 * codesign extract-c` can walk this file and emit a linear CTXDSL
 * automaton on the same rendezvous-label alphabet — see
 * `extract-c-demo.sh` next to this file.
 *
 * The `UART_TypeDef` shape mirrors the canonical Cortex-M HAL
 * convention (`UART->CTRL.bit.tx_start`, `UART->STATUS.bit.tx_busy`,
 * `UART->DATA.byte`) so the reconstructed accessor strings line up
 * with the `c_accessor` field on each register-map entry. The values
 * 0x40010000 / 0x4 / 0x8 match the offsets in `register_map.json`.
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

/* In a real linker script `UART` would resolve to the peripheral's
 * base address (here 0x40010000). For the model-extraction demo we
 * keep it as an extern declaration so clang's AST emits a
 * DeclRefExpr the body walker can match against the register-map's
 * `c_accessor` strings (`UART->CTRL.bit.tx_start`, …). Slice 2.b
 * pre-expands #define-based base-pointers away; the extern form is
 * the canonical recipe Doc C §C.5 names as the work-around. */
extern volatile UART_TypeDef *const UART;

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
