/*
 * firmware_buggy.c — Nordic nRF52840 TWIM0 (I2C) driver,
 *                    planted-bug variant.
 *
 * Same general shape as `firmware.c` but with the
 * register-write-ordering bug from `upstream/nrfx_twim_buggy.c`
 * planted in `twim_txrx`: TASKS_RESUME fires BEFORE the
 * buffer-set registers are written, so the in-progress TWIM
 * transaction reads stale buffer pointers from a previous
 * transaction.
 *
 * The pattern is anchored to public Nordic errata pedigree
 * (Errata 211 family — TWIM register-ordering anomalies on early
 * silicon). The upstream `nrfx_twim.c` does NOT have this bug;
 * see `upstream/nrfx_twim.c:534-541` for the canonical
 * (correct) sequence. **This is a pattern study, not a finding
 * about Nordic silicon** (Claims Integrity Rule 2). See
 * README.md "Planted-bug disclosure" for the full statement.
 *
 * mununu's protocol-conformance property
 * `safety_buffer_before_task` checks that every TASKS_RESUME /
 * TASKS_STARTTX is preceded by the relevant buffer-set writes
 * in the same transaction. The buggy variant violates that
 * property; the verifier produces a counterexample trace.
 */

#include <stdint.h>

#define TWIM0_BASE 0x40003000u

#define TWIM0_TASKS_STARTRX   (*(volatile uint32_t *)(TWIM0_BASE + 0x000))
#define TWIM0_TASKS_STARTTX   (*(volatile uint32_t *)(TWIM0_BASE + 0x008))
#define TWIM0_TASKS_STOP      (*(volatile uint32_t *)(TWIM0_BASE + 0x014))
#define TWIM0_TASKS_RESUME    (*(volatile uint32_t *)(TWIM0_BASE + 0x020))
#define TWIM0_EVENTS_STOPPED  (*(volatile uint32_t *)(TWIM0_BASE + 0x104))
#define TWIM0_EVENTS_ERROR    (*(volatile uint32_t *)(TWIM0_BASE + 0x124))
#define TWIM0_EVENTS_TXSTARTED (*(volatile uint32_t *)(TWIM0_BASE + 0x150))
#define TWIM0_EVENTS_LASTTX   (*(volatile uint32_t *)(TWIM0_BASE + 0x160))
#define TWIM0_SHORTS          (*(volatile uint32_t *)(TWIM0_BASE + 0x200))
#define TWIM0_INTENSET        (*(volatile uint32_t *)(TWIM0_BASE + 0x304))
#define TWIM0_ENABLE          (*(volatile uint32_t *)(TWIM0_BASE + 0x500))
#define TWIM0_FREQUENCY       (*(volatile uint32_t *)(TWIM0_BASE + 0x524))
#define TWIM0_RXD_PTR         (*(volatile uint32_t *)(TWIM0_BASE + 0x534))
#define TWIM0_RXD_MAXCNT      (*(volatile uint32_t *)(TWIM0_BASE + 0x538))
#define TWIM0_TXD_PTR         (*(volatile uint32_t *)(TWIM0_BASE + 0x544))
#define TWIM0_TXD_MAXCNT      (*(volatile uint32_t *)(TWIM0_BASE + 0x548))
#define TWIM0_ADDRESS         (*(volatile uint32_t *)(TWIM0_BASE + 0x588))

void twim_init(void) {
    TWIM0_FREQUENCY = 0x04000000u;
    TWIM0_ENABLE    = 6u;
    TWIM0_INTENSET  = 0u;
}

void twim_tx(uint8_t slave_addr, const uint8_t *buf, uint8_t len) {
    TWIM0_ADDRESS    = (uint32_t)slave_addr;
    TWIM0_TXD_PTR    = (uint32_t)(uintptr_t)buf;
    TWIM0_TXD_MAXCNT = (uint32_t)len;
    TWIM0_SHORTS     = 0u;
    TWIM0_TASKS_STARTTX = 1u;

    while (!TWIM0_EVENTS_TXSTARTED)
        ;
    TWIM0_EVENTS_TXSTARTED = 0u;

    while (!TWIM0_EVENTS_LASTTX)
        ;
    TWIM0_EVENTS_LASTTX = 0u;

    TWIM0_TASKS_STOP = 1u;

    while (!TWIM0_EVENTS_STOPPED)
        ;
    TWIM0_EVENTS_STOPPED = 0u;
}

void twim_rx(uint8_t slave_addr, uint8_t *buf, uint8_t len) {
    TWIM0_ADDRESS    = (uint32_t)slave_addr;
    TWIM0_RXD_PTR    = (uint32_t)(uintptr_t)buf;
    TWIM0_RXD_MAXCNT = (uint32_t)len;
    TWIM0_SHORTS     = 0u;
    TWIM0_TASKS_STARTRX = 1u;

    while (!TWIM0_EVENTS_STOPPED)
        ;
    TWIM0_EVENTS_STOPPED = 0u;
}

/* PLANTED BUG: TASKS_RESUME fires BEFORE the buffer-set writes.
 * The upstream `nrfx_twim.c` writes buffers first, then triggers
 * RESUME (see lines 534-541 of the upstream HAL). Inverting the
 * order means the in-progress TWIM transaction may read stale
 * buffer pointers — the verifier catches this. */
void twim_txrx(uint8_t slave_addr,
               const uint8_t *tx_buf, uint8_t tx_len,
               uint8_t *rx_buf, uint8_t rx_len) {
    TWIM0_ADDRESS = (uint32_t)slave_addr;
    /* INTENTIONAL BUG: TASKS_RESUME fires before the new buffer
     * registers are set. The verifier's
     * `safety_buffer_before_task` property is violated here. */
    TWIM0_TASKS_RESUME = 1u;
    TWIM0_TXD_PTR    = (uint32_t)(uintptr_t)tx_buf;
    TWIM0_TXD_MAXCNT = (uint32_t)tx_len;
    TWIM0_RXD_PTR    = (uint32_t)(uintptr_t)rx_buf;
    TWIM0_RXD_MAXCNT = (uint32_t)rx_len;
    TWIM0_SHORTS     = 0u;
}
