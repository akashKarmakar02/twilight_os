//! HPET (High Precision Event Timer) clocksource backend.
//!
//! Used as the continuously readable monotonic clocksource when the TSC is not
//! validated as a stable clocksource (e.g. under QEMU TCG, where the TSC advances
//! at host real time while the LAPIC/PIT advance at QEMU virtual time). The HPET
//! main counter is a free-running 64-bit counter that advances at a fixed
//! hardware rate independent of interrupt delivery, so it does not lose elapsed
//! time when timer IRQs are coalesced or delayed.
//!
//! This is the root-cause fix for #65: the previous TCG fallback derived
//! `CLOCK_MONOTONIC` from `delivered_timer_events * 1ms`, which permanently lost
//! time whenever a periodic LAPIC interrupt was late or coalesced. The HPET
//! counter is read directly and never depends on interrupt count.
//!
//! ## Register layout (offsets from the MMIO base)
//!  - `0x000` General Capabilities / ID
//!  - `0x010` General Configuration
//!  - `0x0f0` Main Counter (64-bit)
//!
//! Capabilities bits: counter period in femtoseconds = bits 63:32; `CNT_64BIT`
//! = bit 13. Configuration bit 0 = `ENABLE_CNF` (start the main counter).

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

/// HPET register offsets (bytes from the MMIO base).
const REG_CAPABILITIES: usize = 0x000;
const REG_CONFIG: usize = 0x010;
const REG_MAIN_COUNTER: usize = 0x0f0;

/// General Configuration bit: enable the main counter.
const ENABLE_CNF: u64 = 1 << 0;
/// Capabilities bit: 64-bit main counter.
const CNT_64BIT: u64 = 1 << 13;

/// The HPET register block fits within one 4 KiB page.
const HPET_MMIO_SIZE: usize = 0x400;

/// A calibrated HPET clocksource backend.
///
/// `base` is the kernel-virtual address of the HPET MMIO block (mapped
/// device/uncached by [`map_mmio`](crate::sys::memory::map_mmio)). All reads in
/// the hot path are a single aligned volatile u64 load of the main counter,
/// followed by a wrapping subtract and a fixed-point multiply+shift — lock-free
/// and allocation-free.
#[derive(Debug)]
pub struct HpetClock {
    /// Kernel-virtual address of the HPET MMIO base register block.
    base: *mut u64,
    /// Counter value captured at boot; elapsed time is measured relative to it.
    epoch: u64,
    /// `elapsed_ns = (elapsed_cycles * mult) >> shift`, computed from the
    /// femtosecond counter period without first rounding to an integer Hz.
    mult: u64,
    shift: u32,
    /// Last value returned by [`read_ns`], enforcing monotonic non-decrease.
    last_ns: AtomicU64,
}

// SAFETY: `HpetClock` holds a raw pointer to MMIO that is shared across CPUs but
// accessed only through volatile reads/writes. The pointer refers to fixed
// device memory that outlives the kernel. Reads are lock-free.
unsafe impl Send for HpetClock {}
unsafe impl Sync for HpetClock {}

impl HpetClock {
    /// Read the current monotonic nanosecond count.
    ///
    /// Lock-free and allocation-free: one volatile u64 read, a wrapping
    /// subtract, a u128 multiply+shift, and a `fetch_max` for monotonicity.
    #[inline]
    pub fn read_ns(&self) -> u64 {
        let now = self.read_counter();
        let delta = now.wrapping_sub(self.epoch);
        let raw = (delta as u128) * (self.mult as u128);
        let ns = if self.shift == 0 {
            raw
        } else {
            (raw + (1u128 << (self.shift - 1))) >> self.shift
        };
        let ns = core::cmp::min(ns, u64::MAX as u128) as u64;
        let prev = self.last_ns.fetch_max(ns, Ordering::AcqRel);
        core::cmp::max(ns, prev)
    }

    /// Raw read of the 64-bit main counter.
    #[inline]
    fn read_counter(&self) -> u64 {
        // SAFETY: `base` points to the HPET MMIO block mapped as device memory.
        // The main counter is at a fixed byte offset and is always safe to read.
        unsafe { read_volatile(self.base.byte_add(REG_MAIN_COUNTER) as *const u64) }
    }
}

/// Discover, map, enable, and validate an HPET clocksource.
///
/// Returns `None` if no HPET table is advertised, the counter is not 64-bit, the
/// period is implausible, the counter does not advance, or the MMIO mapping
/// fails. The caller then falls back to the next backend or fails explicitly.
pub fn discover() -> Option<HpetClock> {
    use acpi::hpet::HpetInfo;

    let hpet = crate::sys::acpi::with_tables(|tables| HpetInfo::new(tables))
        .and_then(|r| r.ok())?;

    let phys_base = hpet.base_address as u64;
    crate::sys::memory::map_mmio(phys_base, HPET_MMIO_SIZE).ok()?;

    // The HHDM gives us a kernel-virtual address for this physical frame.
    let base = crate::sys::memory::phys_to_virt(x86_64::PhysAddr::new(phys_base)).as_mut_ptr::<u64>();

    // Build a temporary view to read capabilities and configure the counter.
    let probe = HpetProbe { base };

    // SAFETY: `base` points to the HPET MMIO block, mapped device/uncached by
    // `map_mmio` above. All register accesses are aligned volatile u64 reads/writes
    // at fixed offsets defined by the HPET specification.
    let (period_fs, epoch) = unsafe {
        let caps = probe.read_reg(REG_CAPABILITIES);
        let period_fs = caps >> 32;

        // Require a 64-bit counter. A 32-bit counter wraps too quickly (~4 s at
        // 1 GHz) for a system monotonic clock without extension logic; reject it
        // explicitly per #65 rather than silently producing a wrapping clock.
        if caps & CNT_64BIT == 0 {
            crate::serial_println!(
                "\x1b[93m[time]\x1b[0m hpet: rejecting 32-bit-only counter (period={} fs)",
                period_fs
            );
            return None;
        }

        // Validate the period. A period of 0 is malformed; a period above 1 second
        // (10^15 fs) is implausible — the HPET specification mandates a minimum rate
        // of ~10 Hz (max period 100 ms), so anything above 1 s is treated as broken.
        if period_fs == 0 || period_fs > 1_000_000_000_000_000 {
            crate::serial_println!(
                "\x1b[93m[time]\x1b[0m hpet: rejecting implausible period={} fs",
                period_fs
            );
            return None;
        }

        // Enable the main counter, preserving unrelated general-configuration bits.
        let cfg = probe.read_reg(REG_CONFIG);
        probe.write_reg(REG_CONFIG, cfg | ENABLE_CNF);

        // Capture the epoch without resetting an already-active consumer. If the
        // counter was already running (e.g. firmware left it enabled), we measure
        // elapsed time relative to its current value rather than resetting it.
        let epoch = probe.read_counter();

        (period_fs, epoch)
    };

    // Prove the counter advances before selecting it. Spin briefly via the TSC
    // (which advances per-cycle even under TCG) and re-read; a non-advancing
    // counter would freeze guest time.
    // SAFETY: same MMIO block as above.
    let tsc0 = super::tsc::read_cycles();
    let mut advanced = false;
    // Wait up to ~1 ms of TSC cycles for the counter to tick.
    let deadline = tsc0.saturating_add(super::tsc::frequency_hz().max(1_000) / 1_000);
    while super::tsc::read_cycles() < deadline {
        // SAFETY: reading the main counter is a fixed volatile MMIO load.
        let now = unsafe { probe.read_counter() };
        if now != epoch {
            advanced = true;
            break;
        }
    }
    if !advanced {
        crate::serial_println!("\x1b[93m[time]\x1b[0m hpet: counter does not advance, rejecting");
        return None;
    }

    let (mult, shift) = compute_mult_shift_period(period_fs);

    Some(HpetClock {
        base,
        epoch,
        mult,
        shift,
        last_ns: AtomicU64::new(0),
    })
}

/// Compute a `mult`/`shift` pair converting HPET counter ticks into nanoseconds,
/// given the counter period in femtoseconds.
///
/// One counter tick equals `period_fs` femtoseconds; one nanosecond equals
/// 10^6 femtoseconds, so `ns = cycles * period_fs / 10^6`. We choose the largest
/// `shift` such that `mult = (period_fs << shift) / 10^6` fits in `u64`, then
/// `elapsed_ns = (cycles * mult) >> shift`. This keeps the full femtosecond
/// precision — it never rounds the period to a lossy integer Hz first.
fn compute_mult_shift_period(period_fs: u64) -> (u64, u32) {
    const FS_PER_NS: u128 = 1_000_000;

    if period_fs == 0 {
        return (0, 0);
    }

    let period = period_fs as u128;
    let mut shift: u32 = 32;
    while shift > 0 {
        let mult = (period * (1u128 << shift) + FS_PER_NS / 2) / FS_PER_NS;
        if mult <= u64::MAX as u128 {
            return (mult as u64, shift);
        }
        shift -= 1;
    }
    let mult = (period + FS_PER_NS / 2) / FS_PER_NS;
    (mult as u64, 0)
}

/// Minimal borrow used during discovery, before the full [`HpetClock`] is built.
struct HpetProbe {
    base: *mut u64,
}

impl HpetProbe {
    unsafe fn read_reg(&self, offset: usize) -> u64 {
        unsafe { read_volatile(self.base.byte_add(offset) as *const u64) }
    }
    unsafe fn write_reg(&self, offset: usize, value: u64) {
        unsafe { write_volatile(self.base.byte_add(offset) as *mut u64, value) };
    }
    unsafe fn read_counter(&self) -> u64 {
        unsafe { self.read_reg(REG_MAIN_COUNTER) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs_for_period(period_fs: u64) -> (u64, u32, u64) {
        let (mult, shift) = compute_mult_shift_period(period_fs);
        (mult, shift, period_fs)
    }

    /// Convert `cycles` ticks of a counter with the given period to ns, using the
    /// same arithmetic as `HpetClock::read_ns`.
    fn cycles_to_ns(cycles: u64, mult: u64, shift: u32) -> u64 {
        let raw = (cycles as u128) * (mult as u128);
        let ns = if shift == 0 {
            raw
        } else {
            (raw + (1u128 << (shift - 1))) >> shift
        };
        core::cmp::min(ns, u64::MAX as u128) as u64
    }

    #[test]
    fn typical_hpet_period_14mhz() {
        // QEMU's default HPET runs at ~14.318 MHz (the ATAC clock): period ≈
        // 69.84 ns ≈ 69_841 fs. 1 tick ≈ 70 ns.
        let (mult, shift, period_fs) = cs_for_period(69_841);
        assert!(mult > 0);
        // 1 tick should be ~70 ns (within rounding).
        let one_tick = cycles_to_ns(1, mult, shift);
        assert!((68..=72).contains(&one_tick), "one_tick={}", one_tick);
        // 1_000_000 ticks ≈ 69.84 ms.
        let million = cycles_to_ns(1_000_000, mult, shift);
        let expected = 1_000_000u128 * period_fs / 1_000_000;
        assert!((million as i128 - expected as i128).abs() <= 2);
    }

    #[test]
    fn period_zero_is_rejected() {
        let (mult, shift) = compute_mult_shift_period(0);
        assert_eq!(mult, 0);
        assert_eq!(shift, 0);
    }

    #[test]
    fn high_resolution_period_10mhz() {
        // A 100 ns period (10 MHz) HPET.
        let (mult, shift) = compute_mult_shift_period(100_000);
        assert!(mult > 0);
        // 10 ticks == 1 us == 1000 ns.
        let ns = cycles_to_ns(10, mult, shift);
        assert_eq!(ns, 1000);
    }

    #[test]
    fn round_trip_rate_matches_period() {
        // For several realistic periods, the per-tick ns should equal
        // round(period_fs / 1e6).
        for &period_fs in &[69_841u64, 100_000, 41_666, 83_333] {
            let (mult, shift) = compute_mult_shift_period(period_fs);
            let ns_per_tick = cycles_to_ns(1, mult, shift);
            let expected = ((period_fs as u128) + 500_000) / 1_000_000;
            assert!(
                (ns_per_tick as i128 - expected as i128).abs() <= 1,
                "period={} ns_per_tick={} expected={}",
                period_fs,
                ns_per_tick,
                expected
            );
        }
    }

    #[test]
    fn long_duration_no_overflow() {
        let (mult, shift, _) = cs_for_period(69_841);
        // ~1 year of ticks at 14.3 MHz.
        let one_year_ticks = 14_318_000u64 * 60 * 60 * 24 * 365;
        let ns = cycles_to_ns(one_year_ticks, mult, shift);
        let secs = ns / 1_000_000_000;
        // ~31.5 million seconds in a year.
        assert!((secs as u128) > 30_000_000 && (secs as u128) < 33_000_000);
    }

    #[test]
    fn counter_wrap_handled() {
        // A wrapping subtraction from an epoch near u64::MAX must produce a small
        // elapsed ns, not overflow.
        let (mult, shift, _) = cs_for_period(69_841);
        let epoch = u64::MAX - 10;
        let now: u64 = 10; // wrapped past u64::MAX
        let delta = now.wrapping_sub(epoch);
        assert_eq!(delta, 21);
        let ns = cycles_to_ns(delta, mult, shift);
        let one_tick = cycles_to_ns(1, mult, shift);
        assert!(ns >= 20 * one_tick && ns <= 21 * one_tick);
    }
}
