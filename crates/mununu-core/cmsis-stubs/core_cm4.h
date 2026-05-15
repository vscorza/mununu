/*
 * core_cm4.h — minimal Cortex-M4 NVIC/SCB stubs for mununu's L8.
 *
 * The real CMSIS-CORE header is ~6000 lines. We only need the
 * symbols upstream HAL code typically references: IRQn_Type enum
 * placeholder, NVIC_EnableIRQ / NVIC_SetPriority / NVIC_ClearPendingIRQ
 * as no-op inline stubs, and the standard SCB cycle-counter
 * accessors as constant-returning stubs.
 *
 * Soundness: an IRQ controller's behaviour is invisible to the
 * register-access extractor — the firmware's volatile loads/stores
 * to peripheral MMIO are what we care about. NVIC stubs that don't
 * touch peripheral memory are sound by construction.
 */
#ifndef MUNUNU_CORE_CM4_H
#define MUNUNU_CORE_CM4_H

#include "cmsis_compiler.h"

/* IRQn_Type — the SDK normally defines this as an enum with one
 * entry per interrupt. Vendors extend it; we ship a permissive
 * `typedef int IRQn_Type` so any integer literal works as an IRQ
 * number. The HAL's `NVIC_EnableIRQ(SOME_IRQn)` call goes through
 * because clang treats the enum-constant SOME_IRQn as `int`
 * once #define'd elsewhere (or accepts an unresolved identifier
 * at parse time when followed by `;`). */
#ifndef IRQn_Type_defined
typedef int IRQn_Type;
#define IRQn_Type_defined 1
#endif

/* NVIC stubs. */
__STATIC_INLINE void NVIC_EnableIRQ(IRQn_Type irq) { (void)irq; }
__STATIC_INLINE void NVIC_DisableIRQ(IRQn_Type irq) { (void)irq; }
__STATIC_INLINE void NVIC_SetPriority(IRQn_Type irq, uint32_t prio) { (void)irq; (void)prio; }
__STATIC_INLINE uint32_t NVIC_GetPriority(IRQn_Type irq) { (void)irq; return 0u; }
__STATIC_INLINE void NVIC_ClearPendingIRQ(IRQn_Type irq) { (void)irq; }
__STATIC_INLINE void NVIC_SetPendingIRQ(IRQn_Type irq) { (void)irq; }
__STATIC_INLINE uint32_t NVIC_GetPendingIRQ(IRQn_Type irq) { (void)irq; return 0u; }
__STATIC_INLINE uint32_t NVIC_GetActive(IRQn_Type irq) { (void)irq; return 0u; }
__STATIC_INLINE void NVIC_SystemReset(void) { while (1) {} }

#endif /* MUNUNU_CORE_CM4_H */
