//! Kernel timekeeping: the system timeline, decoupled from interrupt delivery.
//!
//! This module owns `CLOCK_MONOTONIC` and `CLOCK_REALTIME`. The monotonic
//! timeline is read from a continuously advancing hardware clocksource (the
//! invariant TSC on x86_64), **not** from a count of delivered timer interrupts.
//!
//! This is the root-cause fix for #62: under KVM, periodic LAPIC interrupts can
//! be delayed or coalesced while the vCPU is descheduled, so an interrupt-count
//! clock permanently loses elapsed time. Reading the TSC instead means host
//! descheduling no longer erases guest time.
//!
//! Timer interrupts are now pure *clock events*: they drive scheduler ticks and
//! deadline expiry via [`handle_timer_event`], but they do not advance the
//! timeline. The two responsibilities are separated, as Linux and FreeBSD do.

pub mod clocksource;
pub mod tsc;

use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;

use conquer_once::spin::OnceCell;

use twilight_common::syscall::types::{EFAULT, EINVAL, Timespec};

use crate::driver::timer::cmos::CMOS;
use crate::task::executor::halt;

const NSEC_PER_SEC: u64 = 1_000_000_000;

/// The TSC clocksource, when available. `None` (and `TSC_AVAILABLE = false`)
/// on QEMU TCG, where the TSC is not a valid clocksource.
static CLOCKSOURCE: OnceCell<clocksource::ClockSource> = OnceCell::uninit();

/// Whether the TSC clocksource was successfully initialized. When false,
/// `monotonic_ns()` falls back to the interrupt-count clocksource.
static TSC_AVAILABLE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Boot-time offset such that `CLOCK_REALTIME = offset + CLOCK_MONOTONIC`.
/// Established once from the CMOS RTC; the RTC is used only for the epoch, never
/// for elapsed-time progression.
static REALTIME_OFFSET_NS: AtomicU64 = AtomicU64::new(0);
static OFFSET_INITED: AtomicU64 = AtomicU64::new(0); // 0 = no, 1 = yes

/// Counter of delivered timer events. Under KVM/bare metal this is diagnostic
/// only; the TSC is the clocksource. Under QEMU TCG this counter **is** the
/// clocksource, because TCG delivers every scheduled interrupt (no coalescing)
/// so one event == one tick period.
static TIMER_EVENTS: AtomicU64 = AtomicU64::new(0);

/// Nanoseconds per timer event (tick period). The LAPIC timer is calibrated to
/// 1 kHz, so each event represents 1 ms. Used by the TCG fallback clocksource.
const NS_PER_TIMER_EVENT: u64 = 1_000_000;

/// Initialize the clocksource. Must run once during boot, before any time read.
///
/// On x86_64 this tries the invariant TSC first. Under QEMU TCG (no KVM) the
/// TSC is not a valid clocksource (it advances at host real time while the
/// LAPIC/PIT advance at QEMU virtual time, so the two diverge during idle); in
/// that case we fall back to the interrupt-count clocksource, which is correct
/// under TCG because TCG delivers every interrupt.
pub fn init() {
    match tsc::detect() {
        Some(tsc) => {
            let invariant = tsc.is_invariant();
            let freq = tsc.frequency_hz();
            let freq_source = tsc.frequency_source();
            let usable = tsc.usable_as_clocksource();

            // Always publish the TSC frequency for timer::wait and LAPIC
            // calibration, even when the TSC is not usable as the clocksource.
            tsc::publish_frequency(freq);

            if usable {
                if invariant {
                    crate::serial_println!(
                        "\x1b[93m[time]\x1b[0m clocksource=tsc invariant=true freq={} Hz source={}",
                        freq,
                        freq_source,
                    );
                } else {
                    crate::serial_println!(
                        "\x1b[93m[time]\x1b[0m clocksource=tsc invariant=false freq={} Hz source={} \
                         (warning: TSC not advertised invariant; time may drift in deep sleep on bare metal)",
                        freq,
                        freq_source,
                    );
                }
                let _ = CLOCKSOURCE.try_init_once(|| tsc.into_source());
                TSC_AVAILABLE.store(true, Ordering::Release);
            } else {
                // TCG: TSC frequency is calibrated for timer::wait/LAPIC, but
                // the TSC is not the clocksource. Fall back to interrupt count.
                crate::serial_println!(
                    "\x1b[93m[time]\x1b[0m clocksource=tick freq={} Hz source={} \
                     (TCG: TSC calibrated for delays but not usable as clocksource; using interrupt-count)",
                    freq,
                    freq_source,
                );
            }
        }
        None => {
            // No TSC frequency at all. Fall back to the interrupt-count clocksource.
            crate::serial_println!(
                "\x1b[93m[time]\x1b[0m clocksource=tick (TSC unavailable; using interrupt-count fallback)"
            );
        }
    }
}

/// Continuously advancing monotonic nanoseconds since boot.
///
/// Under KVM/bare metal this reads the TSC clocksource and does not depend on
/// interrupt delivery. Under QEMU TCG it falls back to counting delivered timer
/// events (correct there because TCG does not coalesce interrupts).
#[inline]
pub fn monotonic_ns() -> u64 {
    if TSC_AVAILABLE.load(Ordering::Acquire) {
        if let Some(cs) = CLOCKSOURCE.get() {
            return cs.elapsed_ns(tsc::read_cycles());
        }
    }
    // TCG fallback: each delivered timer event is one tick period.
    TIMER_EVENTS.load(Ordering::Relaxed) * NS_PER_TIMER_EVENT
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

/// Clock-event handler invoked by the timer ISR.
///
/// This advances scheduler state and expires deadlines; it does **not** advance
/// the timeline. The timeline is read from the clocksource on demand. Renamed
/// from the old `pit_tick_isr` to make clear that a delivered interrupt is an
/// event, not a unit of time.
pub fn handle_timer_event() {
    TIMER_EVENTS.fetch_add(1, Ordering::Relaxed);
    crate::sys::proc::on_timer_tick();
}

/// Diagnostic: number of timer events delivered since boot.
pub fn timer_event_count() -> u64 {
    TIMER_EVENTS.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Sleeping
// ---------------------------------------------------------------------------

/// Block the current task until `nanoseconds` of monotonic time have elapsed.
///
/// Two strategies are used, selected by duration:
///
/// * **Short sleeps** (below [`SHORT_SLEEP_THRESHOLD_NS`]) busy-wait on the TSC
///   via [`crate::driver::timer::wait`]. This is necessary because under QEMU TCG
///   the fallback clocksource ([`monotonic_ns`]) has 1 ms granularity (one tick
///   per delivered LAPIC interrupt), so an `hlt`-based sleep of less than 1 ms
///   rounds up to the next tick — up to a ~100× overshoot for a 10 µs sleep. The
///   TSC advances per executed cycle even under TCG, so a TSC busy-wait is precise
///   for short intervals and never stalls (it spins rather than halting, so it is
///   immune to TCG virtual-clock-during-hlt behavior). The CPU cost is bounded by
///   the short duration. This mirrors how Linux (`udelay`/`ndelay`) and FreeBSD
///   (`DELAY`) realize sub-tick delays.
///
/// * **Long sleeps** (at/above the threshold) use a `sti; hlt` loop keyed on
///   [`monotonic_ns`], which is efficient (the CPU halts while waiting) and for
///   which 1 ms tick quantization is negligible.
///
/// Under KVM/bare metal the TSC is the clocksource, so both paths are precise;
/// the threshold only selects between busy-waiting and halting. A deadline-
/// ordered timer wheel that blocks sleeping tasks across tick boundaries (so a
/// long sleep is not woken once per tick) remains a tracked follow-up in #62.
pub fn sleep_ns(nanoseconds: u64) {
    if nanoseconds == 0 {
        return;
    }

    // Below the tick quantization, busy-wait on the TSC for precision. The
    // threshold is a few tick periods: any sleep that would suffer large
    // relative error from 1 ms rounding takes the precise busy-wait path.
    if nanoseconds < SHORT_SLEEP_THRESHOLD_NS {
        crate::driver::timer::wait(nanoseconds);
        return;
    }

    let start = monotonic_ns();
    let deadline = start.saturating_add(nanoseconds);

    while monotonic_ns() < deadline {
        halt();
        crate::sys::preempt::cond_resched();
    }
}

/// Durations below this are realized as a TSC busy-wait rather than an `hlt`
/// loop. Chosen as a small multiple of [`NS_PER_TIMER_EVENT`] (the 1 ms tick
/// period under the TCG fallback clocksource) so that any sleep whose accuracy
/// would be dominated by tick quantization takes the precise busy-wait path.
const SHORT_SLEEP_THRESHOLD_NS: u64 = NS_PER_TIMER_EVENT * 2;

/// Validate and execute a relative `nanosleep`/`clock_nanosleep` request.
pub fn sleep_timespec(req: &Timespec) -> Result<(), i64> {
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= (NSEC_PER_SEC as i64) {
        return Err(-(EINVAL as i64));
    }

    let secs = req.tv_sec as u128;
    let nsec = req.tv_nsec as u128;
    let total = secs
        .saturating_mul(NSEC_PER_SEC as u128)
        .saturating_add(nsec);

    sleep_ns(core::cmp::min(total, u64::MAX as u128) as u64);
    Ok(())
}

// ---------------------------------------------------------------------------
// CLOCK_* policy and clock_gettime
// ---------------------------------------------------------------------------

pub const CLOCK_REALTIME: i32 = 0;
pub const CLOCK_MONOTONIC: i32 = 1;

pub fn sys_clock_gettime(clockid: i32, tp: *mut Timespec) -> i64 {
    if tp.is_null() {
        return -(EFAULT as i64);
    }

    if OFFSET_INITED.load(Ordering::Relaxed) == 0 {
        init_realtime_offset_from_cmos();
    }

    let ns: u64 = match clockid {
        CLOCK_MONOTONIC => monotonic_ns(),
        CLOCK_REALTIME => realtime_ns(),
        _ => return -(EINVAL as i64),
    };

    unsafe {
        (*tp).tv_sec = (ns / NSEC_PER_SEC) as i64;
        (*tp).tv_nsec = (ns % NSEC_PER_SEC) as i64;
    }
    0
}

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
