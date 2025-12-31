use core::sync::atomic::{AtomicU64, Ordering};

pub mod cmos;
pub mod pit;

static TSC_FREQUENCY: AtomicU64 = AtomicU64::new(0);

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
    let calibration_time = 250_000; // 0.25 seconds
    let a = tsc();
    crate::task::executor::sleep(calibration_time as f64 / 1e6);
    let b = tsc();
    TSC_FREQUENCY.store((b - a) / calibration_time, Ordering::Relaxed);
}

pub fn wait(nanoseconds: u64) {
    let delta = nanoseconds * tsc_frequency();
    let start = tsc();
    while tsc() - start < delta {
        core::hint::spin_loop();
    }
}
