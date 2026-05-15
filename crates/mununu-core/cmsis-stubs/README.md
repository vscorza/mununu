# cmsis-stubs — vendor-neutral CMSIS shims for `mununu codesign extract-c`

Part of the mununu codesign C extractor's phase L8 cmsis-stub bundle. These
headers exist to satisfy clang's parse stage so the extractor can lift
register accesses from C source that uses CMSIS-style idioms — they are
**not real CMSIS implementations**.

## What's here

| Header | What it stubs | Soundness posture |
|---|---|---|
| `cmsis_compiler.h` | `__IO` / `__I` / `__O` access qualifiers, inline-attribute macros (`__INLINE`, `__STATIC_INLINE`, `__WEAK`, …), compiler intrinsics (`__NOP`, `__DMB`, `__DSB`, `__ISB`), PRIMASK manipulation. | Each stub is a no-op or returns a constant. The extractor's register-access lifting only inspects volatile load/store to peripheral MMIO; intrinsics that don't touch MMIO are invisible to it. |
| `core_cm4.h` | CMSIS-CORE Cortex-M4 essentials: `IRQn_Type` typedef, `NVIC_EnableIRQ` / `NVIC_SetPriority` / `NVIC_ClearPendingIRQ` and friends as inline no-ops. | NVIC behaviour doesn't touch peripheral MMIO. Stubs are sound under the extractor's "single-threaded firmware view" abstraction (Doc C §C.5). |

## How `mununu codesign extract-c --cmsis-stubs` uses them

The CLI flag adds this directory to clang's include path. Combined with an
SVD-derived header (generate via `mununu codesign emit-cmsis-header --svd
file.svd --vendor-prefix NRF_`), they let clang parse firmware that writes
`NRF_TWIM0->TASKS_STARTTX = 1` directly.

## What this bundle does NOT cover

- Vendor-specific HAL headers (`nrfx_twim.h`, `stm32f4xx_hal_uart.h`,
  `mxc_uart.h`, …). These declare peripheral-specific enums (error codes,
  configuration constants, helper-function prototypes) that the SVD does
  not describe. Per-SDK stub packs would close this; they're outside L8's
  scope.
- libc symbols (`errno.h`, `string.h`'s `memcpy`). Where firmware C uses
  these, the system's libc headers should already be available — they're
  in clang's default include path on macOS/Linux.
- ARM-specific compiler intrinsics beyond the basic set (`__LDREXW`,
  `__STREXW`, bit-banding helpers). These are easy to add when a real
  use-case demands them.

## Soundness

The stubs only cover *vendor-neutral* CMSIS primitives whose semantics
have no effect on register-access extraction. The two pillars:

1. **No stub here touches peripheral MMIO.** Anything that did would
   appear as a phantom register access in the extractor's output.
2. **All stubs are inlinable.** clang at `-O0` keeps them as separate
   calls; the call-graph walker in phase L5 inlines them at extraction
   time and finds zero accesses, which is correct (a `__NOP` doesn't
   read or write any register).

If a future extension to this bundle needs to model a vendor primitive
whose semantics interact with the register layer (e.g. a vendor's atomic
register-modify intrinsic), the soundness note must accompany the
addition.
