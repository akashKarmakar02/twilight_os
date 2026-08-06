# Timekeeping

Twilight's kernel timekeeping owns `CLOCK_MONOTONIC` and `CLOCK_REALTIME`. The
monotonic timeline is read from a **continuously advancing hardware clocksource**,
never from a count of delivered timer interrupts.

## Why this matters

A periodic LAPIC interrupt is a *clock event*, not an elapsed-time source. Under
QEMU TCG (and under KVM when the vCPU is descheduled), periodic interrupts can be
delayed or coalesced while `IF` is clear. Deriving `CLOCK_MONOTONIC` from
`delivered_timer_events * tick_period` permanently loses elapsed time whenever an
interrupt is late or merged. This was the root cause of issues #62 and #65.

The fix separates the two responsibilities, as Linux and FreeBSD do:

- **Clocksource** — answers "what time is it?" by reading a free-running hardware
  counter (TSC or HPET). Never depends on interrupt delivery.
- **Clock event** — a delivered timer IRQ drives scheduler ticks and deadline
  expiry via `handle_timer_event()`. It does **not** advance the timeline.

## Clocksource selection

Selection runs once at boot (`driver::time::init()`), after memory/MMIO support
is ready and ACPI tables are cached, and before interrupts are enabled:

1. **Validated invariant/paravirtual TSC** — selected when CPUID advertises an
   invariant TSC (`CPUID.80000007:EDX[8]`) **or** a paravirtual KVM TSC frequency
   (leaf `0x40000010`) is present. KVM guarantees the TSC advances with real
   elapsed time even when the vCPU is descheduled, so a paravirtual frequency is
   both a frequency source and a stability signal.
2. **HPET** — selected when the TSC is not validated as stable (e.g. under QEMU
   TCG). The HPET main counter is discovered through ACPI, mapped device/uncached,
   enabled, and proven to advance before selection.
3. **Explicit failure** — if neither backend validates, time-service
   initialization panics with an explicit message. There is **no silent
   interrupt-count fallback**.

Calibration alone (PIT counter-latch measurement, or `CPUID.0x16` base
frequency) is **not** a stability guarantee: under QEMU TCG the TSC advances at
host real time while the LAPIC/PIT advance at QEMU virtual time, so a
calibrated-only TSC diverges during `hlt`. The hypervisor vendor is explicitly
not used as a clock-quality test.

## Invariants

- `monotonic_ns()` advances without timer IRQ delivery and never moves backward
  (enforced by a `fetch_max` on the last returned nanosecond value).
- Clocksource reads never depend on delivered interrupt count.
- `CLOCK_REALTIME` is the boot-time RTC offset plus corrected monotonic time.
  Twilight does not yet implement `clock_settime`.
- Late clock-event IRQs do not lose elapsed time.
- The hot read path is lock-free and allocation-free; no MMIO mapping occurs
  after initialization.

## HPET backend

The HPET MMIO block is discovered via the cached ACPI tables
(`acpi::hpet::HpetInfo`), not a hardcoded address. It is mapped through
`sys::memory::map_mmio()`, which forces device/uncached page flags (updating any
pre-existing HHDM write-back mapping). The implementation:

- Reads capabilities at `+0x000`, configuration at `+0x010`, main counter at
  `+0x0F0`.
- Extracts the counter period in femtoseconds from capability bits 63:32 and
  converts it to nanoseconds via a wide multiply-before-divide rational
  converter (`compute_mult_shift_period`), preserving full femtosecond precision
  rather than rounding to a lossy integer Hz first.
- Requires a 64-bit counter (32-bit-only counters are rejected explicitly).
- Preserves unrelated general-configuration bits when setting `ENABLE_CNF`.
- Captures the counter epoch without resetting an already-active consumer.
- Proves the counter advances before selecting it.

## Testing

The regression harness under `tools/time-regression/` boots the live ISO across
a QEMU matrix (TCG/KVM, several CPU models, SMP counts) and verifies that
monotonic time passes during idle, CPU load, temporarily masked timer IRQs, and
delayed vCPU execution — and that the delivered-event count can diverge from
monotonic time without losing elapsed time.

## Out of scope

NTP, `clock_settime`, suspend/resume, SMP TSC synchronization, HPET comparator
interrupts, and scheduler sleeping.
