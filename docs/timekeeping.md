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

## Architecture

```text
validated TSC or HPET clocksource
        |
        +--> CLOCK_MONOTONIC / CLOCK_REALTIME
        |
        +--> absolute deadline queue
                  |
                  +--> Sleeping / AwaitingIo -> Runnable
                  |
                  +--> earliest software deadline
                                |
scheduler quantum --------------+--> one-shot LAPIC clockevent
kernel HLT wake -----------------+          |
                                           +--> bounded expiry + rearm

readiness producer ---------------------> event wake
interrupting signal --------------------> signal wake
```

The clocksource, deadline queue, and clockevent deliberately have separate
ownership:

- `driver::time` owns the timelines and reads the selected clocksource.
- `sys::timer` owns process wait tokens and the absolute-deadline queue.
- `driver::time::clockevent` selects the next hardware event from the earliest
  software deadline, scheduler quantum, or kernel-context HLT wake.
- `sys::syscall::time` and `sys::syscall::poll` own their respective POSIX ABI
  validation, user-memory copies, and wake-reason policy.

This separation is the completed architecture tracked by #73. A timer interrupt
is never used as evidence that a fixed amount of time elapsed.

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
- An uninterrupted sleep cannot report success before its deadline.
- A blocked process is absent from runnable selection until an explicit wake.
- Timer IRQ processing never allocates, blocks, or retains a scheduler/timer
  lock across a context switch.

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

## Deadline queue and blocking

Process sleeps and finite I/O waits use one queue of absolute monotonic
deadlines (`sys::timer`). A task that blocks transitions from `Running` to
`Sleeping` or `AwaitingIo`, so it is absent from runnable selection until a
deadline, readiness event, interrupting signal, exit, or cancellation wakes it.
Callers do not loop on `CLOCK_MONOTONIC` and do not use a userspace TSC
spin-wait.

Every queue entry contains a PID, a non-repeating `WaitToken`, a deadline, and a
kind (`Sleep` or `IoTimeout`). The process publishes the same token with its
blocked state. Expiry validates both PID and token, so cancellation, a later
wait by the same process, and PID reuse cannot make a stale entry wake the wrong
wait. Cancelled entries are reclaimed lazily.

The publication transaction is protected by the timer queue's IRQ-safe lock:

1. Validate the request and calculate one absolute deadline.
2. Acquire scheduler ownership.
3. Under `lock_irq`, construct and reserve/push the queue entry in task context.
4. Publish its token, deadline metadata, and blocked state under that same guard.
5. Perform a final earlier-deadline recheck and reprogram the LAPIC if needed.
6. Release all guards before switching tasks.

Timer expiry removes at most 32 entries per hard-IRQ batch into stack storage,
drops the queue lock, and then wakes matching processes. Additional due entries
are drained after `irq_exit`, outside hard-IRQ context. The IRQ path therefore
does not allocate, block, or carry a queue guard across scheduler work.

Current task-side insertion calls `try_reserve_push()` while the timer queue's
IRQ-disabled guard is held; heap growth can allocate inside that critical
section. This differs from comments claiming reservation happens before IRQ
exclusion and is tracked in [#88](https://github.com/akashKarmakar02/twilight_os/issues/88).
Hard-IRQ expiry remains allocation-free. Deadline wakeups make tasks runnable;
current `master` does not implement the development branch's preferred-wakeup
scheduler hint, so normal runnable selection can add scheduling latency.

The synchronization contract remains BSP-only. The queue lock disables local
interrupts and protects IRQ/task sharing, but it is not a claim of SMP scheduler
support.

## Sleep and multiplexed-I/O semantics

`nanosleep` and `clock_nanosleep` copy and validate userspace timespecs before
blocking. Relative sleeps are converted once to an absolute monotonic deadline;
absolute `CLOCK_MONOTONIC` and `CLOCK_REALTIME` sleeps are translated onto the
same queue. An interrupting signal returns `EINTR`; relative sleeps report the
remaining duration while absolute sleeps do not.

`poll`, `ppoll`, and `select` use `IoTimeout` entries on that queue. Their wait
loop follows a lost-wakeup-safe order: inspect readiness, publish the waiter,
recheck readiness and the deadline, atomically block, then unregister and
recheck after every wake. Readiness producers cause event wakes; a finite
timeout wakes only its owning process. Scheduler timer events no longer perform
the former global per-tick poll wake-all.

## Network protocol deadlines

The PID 0 network service owns smoltcp protocol deadlines independently of
poll/select user timeouts (#83). smoltcp reads monotonic microseconds; its relative
poll delay is added to a monotonic nanosecond sample before await_io_until.
Immediate protocol work is paced at a one-millisecond future boundary. NIC
interrupts wake the service, and socket state changes wake poll/socket waiters;
there is no per-tick network broadcast. See [networking.md](networking.md).

## One-shot clockevents

The LAPIC timer is a clockevent device, not a clocksource. After calibration it
is normally programmed in one-shot mode for:

```text
min(earliest software deadline,
    current scheduler quantum deadline,
    kernel-context HLT wake deadline)
```

The timer ISR reads `CLOCK_MONOTONIC`, accounts the scheduler quantum, consumes
the kernel HLT wake if due, expires a bounded deadline batch, and rearms the next
event before EOI. An insertion of an earlier deadline uses an atomic
compare/replace of the armed absolute deadline while interrupts are disabled,
so it cannot be hidden behind an older, later event.

Conversion from nanoseconds to LAPIC counts uses calibrated ticks per second,
wide multiply-before-divide, and ceiling. Already-due deadlines are clamped to
the minimum safe nonzero count; deadlines beyond the 32-bit LAPIC range are
clamped and reconsidered when that event fires. Invalid calibration selects the
explicit periodic-delivery fallback. Even in fallback mode, elapsed time still
comes exclusively from the continuous clocksource.

When there is no deadline and no runnable peer requiring a scheduler quantum,
the LAPIC timer is disarmed rather than delivering an unconditional 1 kHz tick.

## QEMU clock-domain semantics

For ordinary execution under TCG or KVM, vCPU descheduling and guest `IF`
masking must not erase elapsed time: both TSC and HPET are read directly instead
of reconstructing time from delivered IRQs.

An explicit QMP `stop` is different. QEMU may freeze its virtual clock while the
VM is paused, and Twilight treats that interval like a guest suspend rather than
ordinary vCPU descheduling. Host comparisons must therefore exclude deliberate
QMP-stop time from a running-VM rate assertion, or report the pause/resume case
separately. Twilight does not currently promise that `CLOCK_MONOTONIC` includes
time spent in an explicit VM pause or guest suspend.

## Testing

The regression harness under `tools/time-regression/` boots the live ISO across
a QEMU matrix that includes `-accel tcg -cpu core2duo`, `qemu64`, `max`, and
`-enable-kvm -cpu host` when KVM is available. It exercises relative and
absolute sleeps, repeated monotonic reads under CPU work, sleeps under CPU load,
and deadline-aware `poll` timeouts from 10 microseconds through 1 second.

The hard correctness gates are:

- no successful uninterrupted sleep returns early;
- no monotonic read moves backward;
- guest distributions and short-duration batch envelopes are recorded without
  per-iteration serial output contaminating the sample.

The harness records QEMU version, commit, dirty-tree state, ISO hash,
accelerator, CPU model, and SMP count with each run. KVM cells are skipped with
an explicit reason when `/dev/kvm` is unavailable. QMP pause/resume testing is
recorded as `not_run` because the harness disables the monitor; it does not
cover deliberate VM pauses.

Pure unit tests cover clocksource conversion and wraparound, HPET precision,
deadline ordering/cancellation/token reuse, bounded overflow expiry, sleep
validation and absolute-deadline translation, one-shot selection, LAPIC count
conversion and clamping, and poll/select deadline construction.

For the application-level validation tracked separately by #72, Smolnes uses an
accumulated absolute monotonic deadline. If rendering misses a frame, it advances
the schedule by complete frame periods instead of converting lateness into
permanent drift. That pacing validates this kernel architecture but does not
replace the kernel timing tests.

## Out of scope

NTP, `clock_settime`, defined suspend/resume time accounting, SMP scheduling and
TSC synchronization, HPET comparator clockevents, hard real-time guarantees,
and application-specific frame-pacing policy.
