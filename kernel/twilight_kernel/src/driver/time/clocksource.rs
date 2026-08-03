//! Generic fixed-point cycle-to-nanosecond clocksource converter.
//!
//! A [`ClockSource`] holds the epoch counter value and the multiplier/shift pair
//! used to convert elapsed hardware cycles into nanoseconds:
//!
//! ```text
//! elapsed_cycles = (now_cycles - epoch_cycles) mod 2^64
//! elapsed_ns     = (elapsed_cycles * mult) >> shift
//! ```
//!
//! This is deliberately free of any x86 specifics so that it can be unit-tested
//! in isolation and reused by a future HPET or paravirtual (kvm-clock) backend.
//!
//! The conversion mirrors the scheme used by Linux's `struct clocksource`:
//! `mult`/`shift` are precomputed at calibration time so the hot read path is a
//! single multiply + shift, with a `u128` intermediate to avoid overflow.

use core::sync::atomic::{AtomicU64, Ordering};

/// Continuously advancing counter that answers "what time is it?".
///
/// A `ClockSource` does **not** depend on interrupt delivery. Reading the
/// underlying hardware counter (TSC, HPET, ...) and calling [`elapsed_ns`]
/// yields elapsed nanoseconds regardless of how many timer IRQs were delivered.
#[derive(Debug)]
pub struct ClockSource {
    /// Counter value captured at boot; elapsed time is measured relative to it.
    epoch_cycles: u64,
    /// `elapsed_ns = (elapsed_cycles * mult) >> shift`.
    mult: u64,
    shift: u32,
    /// Nominal frequency in Hz, kept for diagnostics only.
    freq_hz: u64,
    /// Last value returned by [`elapsed_ns`], used to enforce monotonic
    /// non-decrease across readers on CPUs whose counters are not perfectly
    /// synchronized. Cheap and correct; irrelevant on a single CPU today.
    last_ns: AtomicU64,
}

impl ClockSource {
    /// Build a clocksource from calibrated conversion parameters.
    pub const fn new(epoch_cycles: u64, mult: u64, shift: u32, freq_hz: u64) -> Self {
        Self {
            epoch_cycles,
            mult,
            shift,
            freq_hz,
            last_ns: AtomicU64::new(0),
        }
    }

    /// Nominal counter frequency in Hz (diagnostics).
    pub fn frequency_hz(&self) -> u64 {
        self.freq_hz
    }

    /// Convert a raw counter reading into elapsed nanoseconds since the epoch.
    ///
    /// The subtraction is wrapping so a counter that wraps at `u64::MAX` (TSC at
    /// ~years for GHz rates) is handled correctly. The `u128` multiply cannot
    /// overflow for any realistic elapsed window. Round-to-nearest is applied
    /// (the standard Linux `clocksource_cyc2ns` scheme) so truncation does not
    /// accumulate a rate error.
    pub fn elapsed_ns(&self, now_cycles: u64) -> u64 {
        let delta = now_cycles.wrapping_sub(self.epoch_cycles);
        let raw = (delta as u128) * (self.mult as u128);
        let ns = if self.shift == 0 {
            raw
        } else {
            (raw + (1u128 << (self.shift - 1))) >> self.shift
        };
        let ns = core::cmp::min(ns, u64::MAX as u128) as u64;

        // Guarantee non-decreasing reads. `fetch_max` is a single locked RMW on
        // x86; on a single-CPU system this is effectively free.
        let prev = self.last_ns.fetch_max(ns, Ordering::AcqRel);
        core::cmp::max(ns, prev)
    }
}

/// Compute a `mult`/`shift` pair so that `(cycles * mult) >> shift == ns`
/// for a counter running at `freq_hz`.
///
/// We want `ns = cycles * 1_000_000_000 / freq_hz`. Choosing `shift` as large
/// as possible while keeping `mult` within `u64` maximizes precision. This is
/// the standard clocksource calibration helper.
pub fn compute_mult_shift(freq_hz: u64) -> (u64, u32) {
    const NSEC_PER_SEC: u128 = 1_000_000_000;

    if freq_hz == 0 {
        return (0, 0);
    }

    // Find the largest shift such that mult = (NSEC_PER_SEC << shift) / freq_hz
    // still fits in u64. Start from a generous shift and back off.
    let mut shift: u32 = 32;
    while shift > 0 {
        let mult = ((NSEC_PER_SEC << shift) + (freq_hz as u128) / 2) / (freq_hz as u128);
        if mult <= u64::MAX as u128 {
            return (mult as u64, shift);
        }
        shift -= 1;
    }
    // shift == 0: ns = cycles * 1e9 / freq_hz, truncated. Only reached for
    // absurdly high frequencies; still correct, just lower precision.
    let mult = (NSEC_PER_SEC + (freq_hz as u128) / 2) / (freq_hz as u128);
    (mult as u64, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs_for_freq(freq_hz: u64) -> ClockSource {
        let (mult, shift) = compute_mult_shift(freq_hz);
        ClockSource::new(0, mult, shift, freq_hz)
    }

    #[test]
    fn exact_frequency_conversion() {
        // 1 GHz: 1 cycle == 1 ns.
        let cs = cs_for_freq(1_000_000_000);
        assert_eq!(cs.elapsed_ns(1_000_000_000), 1_000_000_000);
        assert_eq!(cs.elapsed_ns(1_000), 1_000);
    }

    #[test]
    fn submicrosecond_rounding() {
        // 3 GHz: 3 cycles == 1 ns.
        let cs = cs_for_freq(3_000_000_000);
        // 3 cycles -> 1 ns
        assert_eq!(cs.elapsed_ns(3), 1);
        // 3000 cycles -> 1000 ns
        assert_eq!(cs.elapsed_ns(3000), 1000);
    }

    #[test]
    fn epoch_offset() {
        let (mult, shift) = compute_mult_shift(1_000_000_000);
        let cs = ClockSource::new(1_000_000, mult, shift, 1_000_000_000);
        // 1e6 cycles after epoch == 1e6 ns.
        assert_eq!(cs.elapsed_ns(2_000_000), 1_000_000);
    }

    #[test]
    fn counter_wrap() {
        let (mult, shift) = compute_mult_shift(1_000_000_000);
        let cs = ClockSource::new(u64::MAX - 100, mult, shift, 1_000_000_000);
        // 200 cycles elapsed across the wrap == 200 ns.
        assert_eq!(cs.elapsed_ns(100), 200);
    }

    #[test]
    fn monotonic_non_decrease() {
        let cs = cs_for_freq(1_000_000_000);
        let a = cs.elapsed_ns(500);
        let b = cs.elapsed_ns(400); // hypothetical backward TSC read
        assert!(b >= a);
    }

    #[test]
    fn long_duration_no_overflow() {
        let cs = cs_for_freq(3_000_000_000);
        // ~1 year of cycles at 3 GHz.
        let one_year_cycles = 3_000_000_000u64 * 60 * 60 * 24 * 365;
        let ns = cs.elapsed_ns(one_year_cycles);
        let secs = ns / 1_000_000_000;
        assert!((secs as u128) > 31_000_000 && (secs as u128) < 32_000_000);
    }
}
