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

pub mod clocksource;
pub mod hpet;
pub mod source;
pub mod tsc;

use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;

use conquer_once::spin::OnceCell;

use twilight_common::syscall::types::{EFAULT, EINVAL, Timespec};

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

/// Clock-event handler invoked by the timer ISR.
///
/// This advances scheduler state and expires deadlines; it does **not** advance
/// the timeline. The timeline is read from the clocksource on demand. Renamed
/// from the old `pit_tick_isr` to make clear that a delivered interrupt is an
/// event, not a unit of time.
pub fn handle_timer_event() {
    TIMER_EVENTS.fetch_add(1, Ordering::Relaxed);
    crate::sys::proc::on_timer_tick();
    // Expire deadline-blocked sleepers. The timeline is read from the
    // clocksource on demand, so this uses monotonic_ns(), not the event count.
    // Bounded hard-IRQ batch; overflow is drained post-irq_exit by
    // sys::timer::process_deferred_expiry().
    crate::sys::timer::expire_due(monotonic_ns());
}

/// Diagnostic: number of timer events delivered since boot. Used by the
/// regression harness to prove delivered-event count can diverge from monotonic
/// time without losing elapsed time.
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
///   via [`crate::driver::timer::wait`]. This counts TSC cycles, which advance
///   per executed instruction even under QEMU TCG, so it is a precise *delay*
///   regardless of which clocksource backs `CLOCK_MONOTONIC`. (The TSC is not
///   used as the system clocksource under TCG because its wall rate diverges
///   from virtual time during `hlt`, but for a short busy-wait that rate error is
///   negligible.) This mirrors how Linux (`udelay`/`ndelay`) and FreeBSD
///   (`DELAY`) realize sub-tick delays.
///
/// * **Long sleeps** (at/above the threshold) block on the deadline-ordered
///   timer queue ([`crate::sys::timer::block_current_until`]). The caller is
///   moved to `Sleeping` and selected by the runnable scan only after its
///   deadline expires, so it is not woken once per tick and does not suffer a
///   runnable-queue delay after expiry. The CPU halts while waiting.
pub fn sleep_ns(nanoseconds: u64) {
    if nanoseconds == 0 {
        return;
    }

    // Below the tick quantization, busy-wait on the TSC for precision. The
    // threshold is a few tick periods: any sleep that would suffer large relative
    // error from 1 ms rounding takes the precise busy-wait path.
    if nanoseconds < SHORT_SLEEP_THRESHOLD_NS {
        crate::driver::timer::wait(nanoseconds);
        return;
    }

    let deadline = monotonic_ns().saturating_add(nanoseconds);
    // Block on the deadline queue. The wake reason is ignored here: plain
    // nanosleep is not interruptible by signals yet (that is the POSIX syscall
    // ticket's concern). A Deadline reason means the absolute time arrived.
    let _reason = crate::sys::timer::block_current_until(deadline);
}

/// Durations below this are realized as a TSC busy-wait rather than an `hlt`
/// loop. A small multiple of the 1 ms tick period so that any sleep whose
/// accuracy would be dominated by tick quantization takes the precise busy-wait
/// path. Under an HPET clocksource the long-sleep path is already precise (HPET
/// resolution is typically ~70 ns), so the threshold only selects between
/// busy-waiting and halting for short durations.
const SHORT_SLEEP_THRESHOLD_NS: u64 = 2_000_000;

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
