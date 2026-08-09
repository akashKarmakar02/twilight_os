//! Kernel timekeeping: the system timeline, decoupled from interrupt delivery.
//!
//! This module owns `CLOCK_MONOTONIC` and `CLOCK_REALTIME`. The monotonic
//! timeline is read from a continuously advancing hardware clocksource — a
//! validated invariant/paravirtual TSC, or an HPET main counter — **never** from
//! a count of delivered timer interrupts.
//!
//! This is the root-cause fix for #65 (and its predecessor #62). Under QEMU TCG,
//! periodic LAPIC interrupts can be delayed or coalesced while the vCPU is
//! descheduled, so an interrupt-count clock permanently loses elapsed time. The
//! previous TCG fallback (`delivered_timer_events * 1ms`) is removed; under TCG
//! we now read the HPET main counter, which advances at a fixed hardware rate
//! independent of interrupt delivery.
//!
//! Timer interrupts are pure *clock events*: they drive scheduler ticks and
//! deadline expiry via [`handle_timer_event`], but they do not advance the
//! timeline. The two responsibilities are separated, as Linux and FreeBSD do.

pub mod clockevent;
pub mod clocksource;
pub mod hpet;
pub mod source;
pub mod tsc;

use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;

use conquer_once::spin::OnceCell;

use crate::driver::timer::cmos::CMOS;

const NSEC_PER_SEC: u64 = 1_000_000_000;

/// The selected continuously readable clocksource backend. `None` only before
/// [`init`] has run; if [`init`] cannot select any backend it panics rather than
/// leaving this unset.
static CLOCKSOURCE: OnceCell<source::SelectedClocksource> = OnceCell::uninit();

/// Boot-time offset such that `CLOCK_REALTIME = offset + CLOCK_MONOTONIC`.
/// Established once from the CMOS RTC; the RTC is used only for the epoch, never
/// for elapsed-time progression.
static REALTIME_OFFSET_NS: AtomicU64 = AtomicU64::new(0);
static OFFSET_INITED: AtomicU64 = AtomicU64::new(0); // 0 = no, 1 = yes

/// Counter of delivered timer events, diagnostic only. The timeline is read from
/// [`CLOCKSOURCE`]; this counter exists so [`timer_event_count`] can report how
/// many clock-event IRQs were delivered (used by the regression harness to prove
/// that delivered-event count can diverge without affecting monotonic time).
static TIMER_EVENTS: AtomicU64 = AtomicU64::new(0);

/// Initialize the clocksource. Must run once during boot, before any time read
/// and before interrupts are enabled.
///
/// Selection policy (see #65):
///  1. Validated invariant/paravirtual TSC — correct under KVM `-cpu host` on an
///     invariant host, or when a paravirtual KVM TSC frequency is advertised.
///  2. HPET — discovered via ACPI, mapped device/uncached, validated to advance.
///  3. If neither validates: panic with an explicit message. There is no silent
///     interrupt-count fallback.
pub fn init() {
    // Always try the TSC first. `detect()` calibrates the TSC frequency even when
    // the TSC is not usable as the clocksource, because `timer::wait` and LAPIC
    // calibration need a TSC frequency for short busy-waits.
    let selected = match tsc::detect() {
        Some(tsc) => {
            let freq = tsc.frequency_hz();
            let freq_source = tsc.frequency_source();
            tsc::publish_frequency(freq);

            if tsc.usable_as_clocksource() {
                let invariant = tsc.is_invariant();
                if invariant {
                    crate::serial_println!(
                        "\x1b[93m[time]\x1b[0m clocksource=tsc invariant=true freq={} Hz source={}",
                        freq,
                        freq_source,
                    );
                } else {
                    crate::serial_println!(
                        "\x1b[93m[time]\x1b[0m clocksource=tsc invariant=false freq={} Hz source={} \
                         (paravirtual TSC frequency; valid clocksource)",
                        freq,
                        freq_source,
                    );
                }
                source::SelectedClocksource::Tsc(tsc.into_source())
            } else {
                // TSC calibrated but not usable as the clocksource (e.g. QEMU
                // TCG: no invariant flag, no paravirtual frequency). Try HPET.
                select_hpet(freq, freq_source)
            }
        }
        None => {
            crate::serial_println!(
                "\x1b[93m[time]\x1b[0m TSC unavailable; selecting HPET clocksource"
            );
            select_hpet(0, "none")
        }
    };

    let _ = CLOCKSOURCE.try_init_once(|| selected);
}

/// Try to select the HPET backend; panic if it does not validate.
///
/// `tsc_freq_hz` / `tsc_freq_source` are passed only for diagnostics.
fn select_hpet(tsc_freq_hz: u64, tsc_freq_source: &str) -> source::SelectedClocksource {
    match hpet::discover() {
        Some(hpet) => {
            crate::serial_println!(
                "\x1b[93m[time]\x1b[0m clocksource=hpet (tsc_freq={} Hz source={} not usable; \
                 HPET selected)",
                tsc_freq_hz,
                tsc_freq_source,
            );
            source::SelectedClocksource::Hpet(hpet)
        }
        None => {
            // No continuous backend validated. Per #65, fail explicitly rather
            // than silently falling back to an interrupt-count clocksource.
            crate::serial_println!(
                "\x1b[91m[time]\x1b[0m FATAL: no continuous clocksource validated \
                 (TSC unusable, HPET unavailable); refusing to boot with interrupt-count fallback"
            );
            panic!("no continuous clocksource validated (TSC unusable, HPET unavailable)");
        }
    }
}

/// Continuously advancing monotonic nanoseconds since boot.
///
/// Reads the selected hardware backend (TSC or HPET) directly. Does not depend on
/// interrupt delivery: late or coalesced timer IRQs do not affect this value.
#[inline]
pub fn monotonic_ns() -> u64 {
    match CLOCKSOURCE.get() {
        Some(cs) => cs.read_ns(),
        None => 0,
    }
}

/// Wall-clock nanoseconds since the Unix epoch (`CLOCK_REALTIME`).
#[inline]
pub fn realtime_ns() -> u64 {
    REALTIME_OFFSET_NS
        .load(Ordering::Relaxed)
        .saturating_add(monotonic_ns())
}

/// Establish the realtime epoch from the CMOS RTC.
///
/// Called once at boot. The RTC defines the wall-clock origin; monotonic elapsed
/// time thereafter comes from the clocksource, so realtime no longer inherits
/// interrupt-count drift.
pub fn init_realtime_offset_from_cmos() {
    let cmos_secs: u64 = CMOS::new().unix_time();
    let realtime_ns = cmos_secs.saturating_mul(NSEC_PER_SEC);
    let mono_ns = monotonic_ns() as u128;

    // REALTIME = OFFSET + MONOTONIC  =>  OFFSET = REALTIME - MONOTONIC
    let offset = (realtime_ns as u128).saturating_sub(mono_ns);
    REALTIME_OFFSET_NS.store(offset as u64, Ordering::Relaxed);
    OFFSET_INITED.store(1, Ordering::Relaxed);
}

/// The boot-time realtime epoch offset (`CLOCK_REALTIME = offset + monotonic`).
///
/// Exposed so the time-syscall policy module can translate absolute `CLOCK_REALTIME`
/// deadlines into monotonic deadlines for the deadline queue without reaching into
/// this module's privates.
pub fn realtime_offset_ns() -> u64 {
    REALTIME_OFFSET_NS.load(Ordering::Relaxed)
}

/// Lazily establish the realtime epoch on first `CLOCK_REALTIME` access if boot
/// did not already do so. Idempotent.
///
/// The flag is claimed atomically so that, even though only the BSP currently
/// runs syscall code (APs halt in `ap_main`), a future SMP scheduler cannot
/// race two CPUs through the check+init window and shift the epoch with a
/// second CMOS read. Losers observe the already-claimed/completed state and
/// return.
pub fn ensure_realtime_offset_inited() {
    if OFFSET_INITED
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        init_realtime_offset_from_cmos();
    }
}

/// Clock-event handler invoked by the timer ISR.
///
/// This accounts the scheduler quantum, expires deadlines, and rearms the
/// one-shot clockevent for the next event — all before EOI. It does **not**
/// advance the timeline: the timeline is read from the clocksource on demand.
/// Renamed from the old `pit_tick_isr` to make clear that a delivered interrupt
/// is an event, not a unit of time.
///
/// IRQ path (#68):
/// ```text
/// read now -> account quantum -> expire_due -> rearm -> (caller EOIs)
/// ```
pub fn handle_timer_event() {
    TIMER_EVENTS.fetch_add(1, Ordering::Relaxed);
    let now = monotonic_ns();
    // Account the scheduler quantum: set need_resched only if the running
    // task's slice has actually elapsed, replacing the old per-tick reschedule.
    crate::sys::preempt::account_quantum(now);
    // A kernel-context sleep uses HLT without a schedulable Process deadline.
    // Consume its one-shot deadline here; delivery of this interrupt wakes the
    // HLT loop, while clearing it prevents an immediate interrupt storm.
    crate::driver::time::clockevent::account_kernel_hlt_wake(now);
    // Expire deadline-blocked sleepers. Bounded hard-IRQ batch; overflow is
    // drained post-irq_exit by sys::timer::process_deferred_expiry().
    crate::sys::timer::expire_due(now);
    // Rearm the one-shot timer for the next earliest deadline or quantum, before
    // EOI, so a deadline expiring during this IRQ is observed and not lost.
    crate::driver::time::clockevent::rearm(now);
}

/// Diagnostic: number of timer events delivered since boot. Used by the
/// regression harness to prove delivered-event count can diverge from monotonic
/// time without losing elapsed time.
pub fn timer_event_count() -> u64 {
    TIMER_EVENTS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// CLOCK_* ids
// ---------------------------------------------------------------------------
//
// Time-syscall *policy* (validation, user copies, signal interruption, remainder
// computation) lives in `crate::sys::syscall::time`. This module only reads the
// clock and reports clock events; it no longer touches userspace pointers or
// implements sleep semantics.

pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;

// ---------------------------------------------------------------------------
// Uptime helpers (used by logs, procfs, drivers)
// ---------------------------------------------------------------------------

/// Monotonic uptime as a `Duration`.
pub fn uptime_duration() -> Duration {
    let ns = monotonic_ns();
    Duration::new(ns / NSEC_PER_SEC, (ns % NSEC_PER_SEC) as u32)
}

/// Monotonic uptime in seconds as `f64`, for log timestamps and legacy callers.
pub fn uptime_secs_f64() -> f64 {
    let ns = monotonic_ns();
    (ns / NSEC_PER_SEC) as f64 + ((ns % NSEC_PER_SEC) as f64) / 1e9
}
