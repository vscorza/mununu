/*
 * cmsis_compiler.h — vendor-neutral CMSIS compiler primitives stub.
 *
 * Part of mununu's phase L8 cmsis-stubs bundle. The stubs satisfy
 * clang's parse stage for firmware code that uses CMSIS-style
 * macros and intrinsics; they are NOT real implementations.
 *
 * Soundness posture: each stub here resolves to a no-op or a plain
 * read/write, which is sound for the LLVM-IR-based extractor
 * because the extractor only cares about volatile load/store
 * instructions targeting register addresses. Stubs that intercept
 * those would corrupt extraction; stubs that wrap unrelated
 * machinery (NOP, DMB, memory barriers, etc.) are safe.
 */
#ifndef MUNUNU_CMSIS_COMPILER_H
#define MUNUNU_CMSIS_COMPILER_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

/* Access qualifiers — already #defined by the SVD-derived header,
 * but in case the user's flow includes this directly. */
#ifndef __IO
#define __IO volatile
#endif
#ifndef __I
#define __I  volatile const
#endif
#ifndef __O
#define __O  volatile
#endif

/* Inline-attribute helpers. Vendor toolchains have variants
 * (`__INLINE`, `__STATIC_INLINE`, `__forceinline`, …); we map them
 * all to standard C99 `static inline`. */
#ifndef __INLINE
#define __INLINE static inline
#endif
#ifndef __STATIC_INLINE
#define __STATIC_INLINE static inline
#endif
#ifndef __WEAK
#define __WEAK __attribute__((weak))
#endif
#ifndef __ALIGNED
#define __ALIGNED(x) __attribute__((aligned(x)))
#endif
#ifndef __PACKED
#define __PACKED __attribute__((packed))
#endif

/* Compiler intrinsics — no-ops at the IR level. */
#ifndef __NOP
__STATIC_INLINE void __NOP(void) { __asm__ volatile ("" ::: "memory"); }
#endif
#ifndef __DMB
__STATIC_INLINE void __DMB(void) { __asm__ volatile ("" ::: "memory"); }
#endif
#ifndef __DSB
__STATIC_INLINE void __DSB(void) { __asm__ volatile ("" ::: "memory"); }
#endif
#ifndef __ISB
__STATIC_INLINE void __ISB(void) { __asm__ volatile ("" ::: "memory"); }
#endif

/* Critical-section primitives. The extractor models firmware as
 * single-threaded; PRIMASK manipulation is invisible to the
 * register-access analysis. */
#ifndef __disable_irq
__STATIC_INLINE void __disable_irq(void) { __asm__ volatile ("" ::: "memory"); }
#endif
#ifndef __enable_irq
__STATIC_INLINE void __enable_irq(void) { __asm__ volatile ("" ::: "memory"); }
#endif
#ifndef __get_PRIMASK
__STATIC_INLINE uint32_t __get_PRIMASK(void) { return 0u; }
#endif
#ifndef __set_PRIMASK
__STATIC_INLINE void __set_PRIMASK(uint32_t v) { (void)v; }
#endif

#endif /* MUNUNU_CMSIS_COMPILER_H */
