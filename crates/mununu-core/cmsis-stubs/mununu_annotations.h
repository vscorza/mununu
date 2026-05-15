/*
 * mununu_annotations.h — in-source annotation markers consumed by
 *                        `mununu codesign extract-c`.
 *
 * The extractor recognises calls to specific marker functions and
 * lifts them out of the IR as semantic annotations on the
 * surrounding code. The marker functions are declared `extern`
 * here; they have NO definition (the firmware never links — we
 * only need clang to parse + emit IR). Trying to link a firmware
 * that uses these markers without a no-op definition is the
 * caller's responsibility; suggested:
 *
 *     #ifdef MUNUNU_MARKERS_NOOP
 *     void __mununu_state(const char *n) { (void)n; }
 *     #endif
 *
 * SOUNDNESS: the marker calls are at the IR level just call
 * instructions to `@__mununu_state(...)`. The extractor consumes
 * them and they contribute NO register accesses. A real firmware
 * compile (for the device, not for verification) would link them
 * to the no-op definition or strip them via the optimizer.
 */
#ifndef MUNUNU_ANNOTATIONS_H
#define MUNUNU_ANNOTATIONS_H

/* Phase-L9 (gap 1 from the verification-stack audit): hint to the
 * extractor that the basic block beginning here represents a state
 * the user named `name`. The synthesised CTXDSL automaton then uses
 * `name` instead of the default `S<N>` for the SOURCE state of
 * every transition emitted between this marker and the next.
 *
 * Example:
 *
 *     void uart_send(uint8_t byte) {
 *         MUNUNU_STATE("Polling");
 *         while (UART->STATUS.bit.tx_busy);
 *         MUNUNU_STATE("Ready");
 *         UART->DATA.byte = byte;
 *         MUNUNU_STATE("Sending");
 *         UART->CTRL.bit.tx_start = 1;
 *     }
 *
 * synthesises to (approximately):
 *
 *     transitions {
 *         transition Polling -> Loop0 on label rd_status_tx_busy;
 *         transition Loop0 -> Ready on label rd_status_tx_busy;
 *         transition Ready -> Sending on label wr_data_byte;
 *         transition Sending -> S4 on label wr_ctrl_tx_start;
 *     }
 *
 * — state names the user can write properties against.
 */
extern void __mununu_state(const char *name);
#define MUNUNU_STATE(name) __mununu_state(name)

#endif /* MUNUNU_ANNOTATIONS_H */
