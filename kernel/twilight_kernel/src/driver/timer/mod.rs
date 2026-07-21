use core::sync::atomic::{AtomicU64, Ordering};

pub mod cmos;
pub mod pit;

static TSC_FREQUENCY: AtomicU64 = AtomicU64::new(0);

/// Calibrated TSC ticks per microsecond.
pub fn tsc_frequency() -> u64 {
    TSC_FREQUENCY.load(Ordering::Relaxed)
}

pub fn tsc() -> u64 {
    unsafe {
        core::arch::x86_64::_mm_lfence();
        core::arch::x86_64::_rdtsc()
    }
}

pub fn init() {
    pit::init(); // Set PIT to 1000Hz (1ms) before calibration
    let calibration_time = 250_000; // 0.25 seconds
    let a = tsc();
    crate::task::executor::sleep(calibration_time as f64 / 1e6);
    let b = tsc();
    TSC_FREQUENCY.store((b - a) / calibration_time, Ordering::Relaxed);
}

pub fn wait(nanoseconds: u64) {
    let ticks_per_microsecond = tsc_frequency();
    if nanoseconds == 0 || ticks_per_microsecond == 0 {
        return;
    }

    // The calibration stores cycles/us. Convert ns to cycles with ceiling
    // division so sub-microsecond ATA timing waits never collapse to zero.
    let delta = ((nanoseconds as u128 * ticks_per_microsecond as u128) + 999) / 1_000;
    let delta = core::cmp::min(delta, u64::MAX as u128) as u64;
    let start = tsc();
    while tsc().wrapping_sub(start) < delta {
        core::hint::spin_loop();
    }
}
