/*
 * firmware.c — Nordic nRF52840 TWIM0 (I2C) driver, self-contained.
 *
 * This C source models the register-write sequence performed by
 * `twim_xfer` in `upstream/nrfx_twim.c` (lines ~441-665 in the
 * pinned commit) in a form mununu's LLVM-IR-based C extractor can
 * lift end-to-end without the Nordic SDK include tree.
 *
 * Each register access uses an inline literal address
 * (`*(volatile uint32_t *)0xADDR`) so the extractor lifts it via
 * the L2 `inttoptr` matcher + address-range window matching
 * against `register_map.json`. Register offsets correspond
 * one-to-one to the SVD-derived sidecar at `register_map.json`:
 *
 *   FREQUENCY      offset 0x524 = 1316
 *   ENABLE         offset 0x500 = 1280
 *   ADDRESS        offset 0x588 = 1416
 *   SHORTS         offset 0x200 = 512
 *   INTENSET       offset 0x304 = 772
 *   TXD.PTR        offset 0x544 = 1348
 *   TXD.MAXCNT     offset 0x548 = 1352
 *   RXD.PTR        offset 0x534 = 1332
 *   RXD.MAXCNT     offset 0x538 = 1336
 *   TASKS_STARTTX  offset 0x008 = 8
 *   TASKS_STARTRX  offset 0x000 = 0
 *   TASKS_STOP     offset 0x014 = 20
 *   TASKS_RESUME   offset 0x020 = 32
 *   EVENTS_STOPPED offset 0x104 = 260
 *   EVENTS_ERROR   offset 0x124 = 292
 *   EVENTS_TXSTARTED offset 0x150 = 336
 *   EVENTS_LASTTX  offset 0x160 = 352
 *
 * SOUNDNESS — CORRECT-ORDER VARIANT.
 *
 * This file models the **correct** register-write ordering from the
 * upstream `nrfx_twim.c`: every buffer pointer / configuration
 * register is written BEFORE the task that consumes it is
 * triggered. The buggy variant at `firmware_buggy.c` inverts this
 * ordering for two specific transitions and produces a verifier
 * counterexample under the protocol-conformance property.
 *
 * The example is a *pattern study* anchored to public Nordic
 * errata pedigree (Errata 211 family — TWIM register-ordering
 * anomalies on early silicon). It is NOT a claim about any
 * specific commercial silicon. See README.md "Planted-bug
 * disclosure" for the full Claims Integrity Rule 2 statement.
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

/* Initialise the peripheral: frequency, then enable. */
void twim_init(void) {
    TWIM0_FREQUENCY = 0x04000000u;   /* 100 kHz */
    TWIM0_ENABLE    = 6u;            /* magic enable value for TWIM */
    TWIM0_INTENSET  = 0u;            /* polling-mode; no IRQs */
}

/* Issue a TX-only transaction: buffer + length + slave address, then trigger. */
void twim_tx(uint8_t slave_addr, const uint8_t *buf, uint8_t len) {
    TWIM0_ADDRESS    = (uint32_t)slave_addr;
    TWIM0_TXD_PTR    = (uint32_t)(uintptr_t)buf;
    TWIM0_TXD_MAXCNT = (uint32_t)len;
    TWIM0_SHORTS     = 0u;                       /* no shortcut */
    TWIM0_TASKS_STARTTX = 1u;                    /* fire */

    /* Poll for TXSTARTED then LASTTX then STOPPED. */
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

/* Issue an RX-only transaction: buffer + length + slave address. */
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

/* TXRX-with-resume: the canonical Nordic Errata-211 sequence.
 * The correct ordering is buffer-set THEN resume; the buggy
 * variant inverts these. */
void twim_txrx(uint8_t slave_addr,
               const uint8_t *tx_buf, uint8_t tx_len,
               uint8_t *rx_buf, uint8_t rx_len) {
    TWIM0_ADDRESS = (uint32_t)slave_addr;
    /* Correct order: write all buffer/length registers BEFORE
     * triggering TASKS_RESUME (which kicks the in-progress TWIM
     * transaction forward). */
    TWIM0_TXD_PTR    = (uint32_t)(uintptr_t)tx_buf;
    TWIM0_TXD_MAXCNT = (uint32_t)tx_len;
    TWIM0_RXD_PTR    = (uint32_t)(uintptr_t)rx_buf;
    TWIM0_RXD_MAXCNT = (uint32_t)rx_len;
    TWIM0_SHORTS     = 0u;
    TWIM0_TASKS_RESUME = 1u;
}
