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

/// The selected hardware clocksource. `OnceCell` because it is initialized once
/// at boot and read on every time read; reads are lock-free after init.
static CLOCKSOURCE: OnceCell<clocksource::ClockSource> = OnceCell::uninit();

/// Boot-time offset such that `CLOCK_REALTIME = offset + CLOCK_MONOTONIC`.
/// Established once from the CMOS RTC; the RTC is used only for the epoch, never
/// for elapsed-time progression.
static REALTIME_OFFSET_NS: AtomicU64 = AtomicU64::new(0);
static OFFSET_INITED: AtomicU64 = AtomicU64::new(0); // 0 = no, 1 = yes

/// Diagnostic counter of delivered timer events. **Not** the timeline. Kept
/// only so boot diagnostics can compare IRQ count against clocksource elapsed
/// time; never used to derive `CLOCK_MONOTONIC`.
static TIMER_EVENTS: AtomicU64 = AtomicU64::new(0);

/// Initialize the clocksource. Must run once during boot, before any time read.
///
/// On x86_64 this detects the invariant TSC and calibrates it. Failing to
/// initialize is fatal because every consumer (logs, sleeps, scheduler,
/// syscalls) depends on a working timeline.
pub fn init() {
    let tsc = tsc::detect().expect("no usable clocksource: TSC frequency unavailable");

    let invariant = tsc.is_invariant();
    let freq = tsc.frequency_hz();
    let freq_source = tsc.frequency_source();

    if invariant {
        crate::serial_println!(
            "\x1b[93m[time]\x1b[0m clocksource=tsc invariant=true freq={} Hz source={}",
            freq,
            freq_source,
        );
    } else {
        // The TSC is usable but CPUID did not advertise it as invariant. On
        // real hardware this means the TSC may stop in deep C-states; under
        // software emulation (TCG) there are no real C-states so it is correct.
        crate::serial_println!(
            "\x1b[93m[time]\x1b[0m clocksource=tsc invariant=false freq={} Hz source={} \
             (warning: TSC not advertised invariant; time may drift in deep sleep on bare metal)",
            freq,
            freq_source,
        );
    }

    tsc::publish_frequency(freq);

    // `detect()` already captured the epoch at calibration time; store that
    // ClockSource as-is so elapsed_ns is near-zero at boot.
    let _ = CLOCKSOURCE.try_init_once(|| tsc.into_source());
}

/// Continuously advancing monotonic nanoseconds since boot.
///
/// Lock-free, allocation-free, and safe to call from any context including the
/// timer ISR. Does not depend on interrupt delivery.
#[inline]
pub fn monotonic_ns() -> u64 {
    let Some(cs) = CLOCKSOURCE.get() else {
        // Before init: no time has elapsed. Returning 0 keeps early-boot
        // consumers (e.g. the log macro) functional.
        return 0;
    };
    cs.elapsed_ns(tsc::read_cycles())
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
/// This still uses a `sti; hlt` loop keyed on [`monotonic_ns`], which is now
/// correct because the deadline is measured against the TSC. Replacing this with
/// a deadline-ordered timer wait queue (so a sleeping task is not woken once per
/// intermediate tick) is tracked as a separate follow-up in #62 (Phase 3).
pub fn sleep_ns(nanoseconds: u64) {
    if nanoseconds == 0 {
        return;
    }

    let start = monotonic_ns();
    let deadline = start.saturating_add(nanoseconds);

    while monotonic_ns() < deadline {
        halt();
        crate::sys::preempt::cond_resched();
    }
}

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
