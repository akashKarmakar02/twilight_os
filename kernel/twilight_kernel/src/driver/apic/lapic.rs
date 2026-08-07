//! Local APIC driver.
//!
//! The timer is programmed in **one-shot mode** as a clockevent: each event is
//! armed for a specific nanosecond delta and fires once. This replaces the old
//! fixed 1 kHz periodic tick (#68). A periodic fallback is retained behind
//! [`FORCE_PERIODIC_FALLBACK`] (and auto-selected when calibration rejects) so
//! the kernel can still boot on a CPU model that miscalibrates; the fallback is a
//! delivery mechanism only and never serves as a clocksource.

use crate::sys::memory::phys_mem_offset;
use conquer_once::spin::OnceCell;
use core::ptr::{read_volatile, write_volatile};

// Standard Local APIC base address
const LAPIC_BASE: u64 = 0xFEE00_000;

// Register Offsets
const ID_REG: u64 = 0x020;
#[allow(dead_code)]
const VERSION_REG: u64 = 0x030;
#[allow(dead_code)]
const TPR_REG: u64 = 0x080; // Task Priority
const EOI_REG: u64 = 0x0B0; // End of Interrupt
const SVR_REG: u64 = 0x0F0; // Spurious Interrupt Vector
const TICR_REG: u64 = 0x380; // Timer Initial Count
const TCCR_REG: u64 = 0x390; // Timer Current Count
const TDCR_REG: u64 = 0x3E0; // Timer Divide Config
const LVT_TIMER_REG: u64 = 0x320; // LVT Timer

// Timer Modes (LVT timer bits).
const TIMER_ONE_SHOT: u32 = 0x00;
const TIMER_PERIODIC: u32 = 0x20000;

/// Timer interrupt vector used by both one-shot and periodic modes.
const TIMER_VECTOR: u32 = 0xFD;

/// Divide-by-16 configuration value for TDCR.
const DIVIDE_BY_16: u32 = 0x3;

/// Force the periodic 1 kHz fallback regardless of calibration. Debug switch:
/// leave false unless diagnosing a one-shot regression.
const FORCE_PERIODIC_FALLBACK: bool = false;

/// Duration of the calibration busy-wait, in nanoseconds (10 ms). Measured
/// against the selected continuous clocksource, not an assumed interval.
const CALIB_NS: u64 = 10_000_000;

/// Plausible LAPIC input-frequency bounds. The bus/crystal frequency that feeds
/// the LAPIC timer is at most a few GHz; reject anything outside this range as
/// a calibration failure and fall back to periodic.
const FREQ_MIN_HZ: u64 = 1_000; // 1 kHz
const FREQ_MAX_HZ: u64 = 10_000_000_000; // 10 GHz

/// Minimum safe initial count. Programming a count of 0 or 1 can race the LAPIC
/// and fail to fire; use a small floor.
const MIN_COUNT_TICKS: u32 = 8;

pub unsafe fn write_reg(offset: u64, value: u32) {
    let base = phys_mem_offset() + LAPIC_BASE;
    let ptr = (base + offset) as *mut u32;
    unsafe {
        write_volatile(ptr, value);
    }
}

pub unsafe fn read_reg(offset: u64) -> u32 {
    let base = phys_mem_offset() + LAPIC_BASE;
    let ptr = (base + offset) as *const u32;
    unsafe { read_volatile(ptr) }
}

pub fn end_of_interrupt() {
    unsafe {
        write_reg(EOI_REG, 0);
    }
}

pub fn id() -> u32 {
    unsafe { read_reg(ID_REG) >> 24 }
}

pub fn init() {
    unsafe {
        // Map APIC MMIO page to prevent page fault
        crate::sys::memory::map_mmio(LAPIC_BASE, 4096).expect("Failed to map LAPIC");

        // Enable LAPIC (set bit 8 of SVR, and map vector 0xFF to spurious interrupts)
        write_reg(SVR_REG, 0x100 | 0xFF);

        // Mask timer interrupt until calibration/arming decides the mode.
        write_reg(LVT_TIMER_REG, MASKED);

        // Divide by 16 for both calibration and runtime.
        write_reg(TDCR_REG, DIVIDE_BY_16);

        calibrate_and_program();
    }
}

const MASKED: u32 = 0x10000;

/// Calibrated one-shot clockevent parameters. `None` only before [`init`].
static LAPIC_CE: OnceCell<LapicClockevent> = OnceCell::uninit();

#[derive(Clone, Copy)]
struct LapicClockevent {
    /// LAPIC timer ticks per second, derived from the continuous clocksource.
    ticks_per_sec: u64,
    /// Minimum delta expressible as a safe initial count, in nanoseconds.
    min_delta_ns: u64,
    /// Mode selected at init: one-shot, or periodic fallback.
    mode: ClockeventMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClockeventMode {
    OneShot,
    PeriodicFallback,
}

impl LapicClockevent {
    fn max_delta_ns(&self) -> u64 {
        // Nanoseconds corresponding to the full 32-bit count range.
        ticks_to_ns(u32::MAX, self.ticks_per_sec)
    }
}

/// Calibrate the LAPIC timer against the selected continuous clocksource and
/// select one-shot or periodic-fallback mode.
///
/// Root-cause fix (#68): the previous code calibrated against a TSC busy-wait
/// and *assumed* the wait lasted exactly 10 ms. Under TCG the runtime clocksource
/// may be HPET (a different timebase than the TSC), so calibrating LAPIC ticks
/// against the TSC while deadlines are measured in HPET time is a domain
/// mismatch. We instead measure the actual elapsed clocksource interval and
/// compute `ticks_per_sec = elapsed_ticks * 1e9 / (clock_end - clock_start)`.
fn calibrate_and_program() {
    if FORCE_PERIODIC_FALLBACK {
        program_periodic_fallback("force_periodic_fallback");
        return;
    }

    let Some(ce) = calibrate_oneshot() else {
        program_periodic_fallback("calibration_rejected");
        return;
    };

    let _ = LAPIC_CE.try_init_once(|| ce);
    crate::serial_println!(
        "\x1b[93m[time]\x1b[0m clockevent=oneshot ticks/sec={} min_delta_ns={} max_delta_ns={}",
        ce.ticks_per_sec,
        ce.min_delta_ns,
        ce.max_delta_ns(),
    );
}

/// Run the one-shot calibration and return the parameters if plausible.
fn calibrate_oneshot() -> Option<LapicClockevent> {
    unsafe {
        // One-shot mode, large initial count. The counter counts down from
        // TICR; elapsed ticks = (TICR - TCCR) over the measurement window.
        write_reg(LVT_TIMER_REG, TIMER_ONE_SHOT | TIMER_VECTOR);
        write_reg(TDCR_REG, DIVIDE_BY_16);
        write_reg(TICR_REG, u32::MAX);
    }

    // Measure the actual elapsed clocksource interval. Read the selected
    // backend (TSC or HPET) — the same timebase the deadline queue uses — so a
    // domain mismatch cannot skew the conversion.
    let start = crate::driver::time::monotonic_ns();
    busy_wait_ns(CALIB_NS);
    let end = crate::driver::time::monotonic_ns();

    let elapsed_ticks = u32::MAX.wrapping_sub(unsafe { read_reg(TCCR_REG) });
    let elapsed_ns = end.checked_sub(start)?;

    // ticks/sec = elapsed_ticks * 1e9 / elapsed_ns  (wide multiply-before-divide)
    if elapsed_ns == 0 {
        return None;
    }
    let ticks_per_sec = (elapsed_ticks as u128)
        .checked_mul(1_000_000_000)
        .and_then(|p| p.checked_div(elapsed_ns as u128))
        .and_then(|v| u64::try_from(v).ok())?;
    let ticks_per_sec: u64 = ticks_per_sec;

    if !(FREQ_MIN_HZ..=FREQ_MAX_HZ).contains(&ticks_per_sec) || elapsed_ticks == 0 {
        return None;
    }

    let min_delta_ns = ticks_to_ns(MIN_COUNT_TICKS, ticks_per_sec).max(1);
    Some(LapicClockevent {
        ticks_per_sec,
        min_delta_ns,
        mode: ClockeventMode::OneShot,
    })
}

/// Program the periodic 1 kHz fallback. Used when calibration fails or the debug
/// switch forces it. The periodic tick drives clock events; one-shot APIs
/// become no-ops.
fn program_periodic_fallback(reason: &str) {
    // Derive a 1 ms interval from the clocksource-calibrated tick rate if we
    // can; otherwise fall back to a conservative PIT-derived estimate. Either
    // way this is a delivery mechanism only, never a clocksource.
    let ticks_per_ms = estimate_ticks_per_ms();
    unsafe {
        write_reg(LVT_TIMER_REG, TIMER_PERIODIC | TIMER_VECTOR);
        write_reg(TDCR_REG, DIVIDE_BY_16);
        write_reg(TICR_REG, ticks_per_ms.max(1));
    }
    let _ = LAPIC_CE.try_init_once(|| LapicClockevent {
        ticks_per_sec: (ticks_per_ms as u64).saturating_mul(1000),
        min_delta_ns: 1_000_000,
        mode: ClockeventMode::PeriodicFallback,
    });
    crate::serial_println!(
        "\x1b[93m[time]\x1b[0m clockevent=periodic-fallback reason={} ticks/ms={}",
        reason,
        ticks_per_ms,
    );
}

/// Best-effort 1 ms tick count for the periodic fallback. Reuses the one-shot
/// calibration machinery but does not depend on its plausibility checks.
fn estimate_ticks_per_ms() -> u32 {
    unsafe {
        write_reg(LVT_TIMER_REG, TIMER_ONE_SHOT | TIMER_VECTOR);
        write_reg(TDCR_REG, DIVIDE_BY_16);
        write_reg(TICR_REG, u32::MAX);
    }
    let start = crate::driver::time::monotonic_ns();
    busy_wait_ns(1_000_000);
    let end = crate::driver::time::monotonic_ns();
    let elapsed_ticks = u32::MAX.wrapping_sub(unsafe { read_reg(TCCR_REG) });
    let elapsed_ns = end.saturating_sub(start).max(1);
    // ticks per millisecond, ceiling.
    let per_sec = (elapsed_ticks as u128)
        .saturating_mul(1_000_000_000)
        .checked_div(elapsed_ns as u128)
        .unwrap_or(0);
    (per_sec / 1000).try_into().unwrap_or(1).max(1)
}

/// Busy-wait approximately `nanoseconds` using the selected clocksource.
///
/// Unlike the TSC-domain `timer::wait`, this reads `monotonic_ns()` so it stays
/// in the same timebase the deadline queue uses — correct under TCG where the
/// clocksource may be HPET.
fn busy_wait_ns(nanoseconds: u64) {
    if nanoseconds == 0 {
        return;
    }
    let start = crate::driver::time::monotonic_ns();
    while crate::driver::time::monotonic_ns().wrapping_sub(start) < nanoseconds {
        core::hint::spin_loop();
    }
}

// --- ns <-> ticks conversion ------------------------------------------------

/// Convert nanoseconds to LAPIC ticks with ceiling, clamped to `u32` range.
fn ns_to_ticks(delta_ns: u64, ticks_per_sec: u64) -> u32 {
    if ticks_per_sec == 0 || delta_ns == 0 {
        return MIN_COUNT_TICKS;
    }
    let ticks = ((delta_ns as u128)
        .saturating_mul(ticks_per_sec as u128)
        .saturating_add(999_999_999))
        / 1_000_000_000;
    if ticks > u32::MAX as u128 {
        u32::MAX
    } else if ticks < MIN_COUNT_TICKS as u128 {
        MIN_COUNT_TICKS
    } else {
        ticks as u32
    }
}

/// Convert LAPIC ticks to nanoseconds (floor).
fn ticks_to_ns(ticks: u32, ticks_per_sec: u64) -> u64 {
    if ticks_per_sec == 0 {
        return 0;
    }
    ((ticks as u128).saturating_mul(1_000_000_000) / ticks_per_sec as u128) as u64
}

// --- public clockevent API --------------------------------------------------

/// Program a one-shot timer event `delta_ns` nanoseconds in the future.
///
/// `delta_ns` is clamped to `[min_delta_ns, max_delta_ns]`; an already-due or
/// sub-minimum delta is clamped up to `min_delta_ns` so the LAPIC reliably fires.
/// No-op in periodic-fallback mode (the periodic tick drives events).
pub fn program_oneshot_ns(delta_ns: u64) {
    let Some(ce) = LAPIC_CE.get() else {
        return;
    };
    if ce.mode != ClockeventMode::OneShot {
        return;
    }
    let delta = delta_ns.clamp(ce.min_delta_ns, ce.max_delta_ns());
    let count = ns_to_ticks(delta, ce.ticks_per_sec);
    unsafe {
        write_reg(LVT_TIMER_REG, TIMER_ONE_SHOT | TIMER_VECTOR);
        write_reg(TDCR_REG, DIVIDE_BY_16);
        write_reg(TICR_REG, count);
    }
}

/// Disarm the timer. Masks the LVT timer entry. No-op in periodic-fallback mode.
pub fn cancel_timer() {
    let Some(ce) = LAPIC_CE.get() else {
        return;
    };
    if ce.mode != ClockeventMode::OneShot {
        return;
    }
    unsafe {
        write_reg(LVT_TIMER_REG, MASKED);
    }
}

/// Minimum safe delta in nanoseconds. Sub-minimum deltas are clamped up to this.
pub fn min_delta_ns() -> u64 {
    LAPIC_CE.get().map_or(1, |ce| ce.min_delta_ns)
}

/// Maximum expressible delta in nanoseconds (full 32-bit count range).
pub fn max_delta_ns() -> u64 {
    LAPIC_CE.get().map_or(u64::MAX, |ce| ce.max_delta_ns())
}

/// Selected clockevent mode, for diagnostics.
pub fn mode() -> &'static str {
    match LAPIC_CE.get() {
        Some(ce) if ce.mode == ClockeventMode::OneShot => "oneshot",
        Some(_) => "periodic-fallback",
        None => "uninitialized",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ns_to_ticks_ceils_sub_tick_to_minimum() {
        // 1 ns at 1 GHz is ~1 tick; below MIN_COUNT_TICKS floors to minimum.
        let tps = 1_000_000_000;
        assert_eq!(ns_to_ticks(0, tps), MIN_COUNT_TICKS);
        assert_eq!(ns_to_ticks(1, tps), MIN_COUNT_TICKS);
        // Exactly one tick's worth of ns still respects the minimum floor.
        let one_tick_ns = ticks_to_ns(1, tps);
        assert_eq!(ns_to_ticks(one_tick_ns, tps), MIN_COUNT_TICKS);
    }

    #[test]
    fn ns_to_ticks_ceils_positive_deltas() {
        let tps = 1_000_000_000; // 1 tick per ns
        // 999_999_999 ns -> just under 1e9 ticks -> ceil to 1e9 ticks.
        assert_eq!(ns_to_ticks(999_999_999, tps), 1_000_000_000);
        // 1_000_000_001 ns -> ceil to 1_000_000_001 ticks.
        assert_eq!(ns_to_ticks(1_000_000_001, tps), 1_000_000_001);
    }

    #[test]
    fn ns_to_ticks_clamps_to_u32_max() {
        let tps = 1_000_000_000;
        // A huge delta saturates to u32::MAX rather than wrapping.
        assert_eq!(ns_to_ticks(u64::MAX, tps), u32::MAX);
    }

    #[test]
    fn ns_to_ticks_zero_frequency_is_safe() {
        assert_eq!(ns_to_ticks(1_000_000, 0), MIN_COUNT_TICKS);
    }

    #[test]
    fn ticks_to_ns_round_trips_within_floor() {
        let tps = 2_000_000_000; // 2 ticks/ns
        // 1_000_000_000 ns -> 2_000_000_000 ticks -> back to 1_000_000_000 ns.
        let ticks = ns_to_ticks(1_000_000_000, tps);
        assert_eq!(ticks_to_ns(ticks, tps), 1_000_000_000);
    }

    #[test]
    fn ticks_to_ns_zero_frequency_is_safe() {
        assert_eq!(ticks_to_ns(123, 0), 0);
    }

    #[test]
    fn clamp_uses_min_delta_for_due_deadline() {
        // Simulates an already-due deadline: clamp(min, max) of 0 yields min.
        let min = 1_000u64;
        let max = 1_000_000_000u64;
        assert_eq!(0u64.clamp(min, max), min);
        assert_eq!(5u64.clamp(min, max), min);
        assert_eq!(max.clamp(min, max), max);
        assert_eq!((max + 1).clamp(min, max), max);
    }
}
