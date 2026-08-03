//! Timer hardware subsystem.
//!
//! The system timeline lives in [`crate::driver::time`], read from the
//! invariant-TSC clocksource. This module retains only:
//!  - the PIT hardware setup ([`pit`]) used for early boot clock events and as a
//!    calibration reference, and
//!  - the CMOS RTC ([`cmos`]) used for the realtime epoch.
//!
//! Historically this module also calibrated the TSC by sleeping on the
//! interrupt-count clock. That calibration is removed: the TSC frequency is now
//! resolved from CPUID (or, as a fallback, a PIT-cycle measurement that does not
//! depend on interrupt delivery) in [`crate::driver::time::tsc`]. See #62.

pub mod cmos;
pub mod pit;

/// Calibrated TSC ticks per microsecond, for legacy callers (ATA/USB nanosecond
/// busy-waits and procfs MHz display). Delegates to the clocksource frequency.
pub fn tsc_frequency() -> u64 {
    // The clocksource stores Hz; cycles/us = Hz / 1_000_000.
    crate::driver::time::tsc::frequency_hz() / 1_000_000
}

/// Read the current TSC value (serialized). Delegates to the clocksource backend.
pub fn tsc() -> u64 {
    crate::driver::time::tsc::read_cycles()
}

/// Initialize the PIT hardware. The TSC clocksource is initialized separately
/// by [`crate::driver::time::init`]; this function no longer calibrates the TSC
/// by sleeping on the (now-removed) interrupt-count clock.
pub fn init() {
    pit::init();
}

/// Busy-wait approximately `nanoseconds` using the TSC.
///
/// This is a spin-wait used for short hardware delays (ATA PIO timing, USB
/// frame intervals). It counts TSC cycles, not interrupts, so it is unaffected
/// by the #62 coalescing defect. Ceiling division keeps sub-microsecond waits
/// from collapsing to zero.
pub fn wait(nanoseconds: u64) {
    let ticks_per_microsecond = tsc_frequency();
    if nanoseconds == 0 || ticks_per_microsecond == 0 {
        return;
    }

    let delta = ((nanoseconds as u128 * ticks_per_microsecond as u128) + 999) / 1_000;
    let delta = core::cmp::min(delta, u64::MAX as u128) as u64;
    let start = tsc();
    while tsc().wrapping_sub(start) < delta {
        core::hint::spin_loop();
    }
}
