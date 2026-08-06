//! x86_64 invariant-TSC clocksource backend.
//!
//! The invariant TSC is a continuously advancing counter that advances at a
//! fixed rate independent of CPU power state and (with `nonstop_tsc`) continues
//! to advance while the logical CPU is in C-states. Under KVM with `-cpu host`
//! on a host advertising `constant_tsc` + `nonstop_tsc` + `arat`, the TSC is
//! the correct virtualized clocksource: it advances with real elapsed time even
//! when the vCPU is descheduled and periodic LAPIC interrupts are coalesced.
//!
//! The TSC is selected as the clocksource only when validated as stable: either
//! CPUID advertises an invariant TSC, or a paravirtual KVM TSC frequency is
//! present. Calibration alone is not a stability guarantee. When the TSC is not
//! usable (e.g. under QEMU TCG), the HPET backend is used instead (#65).

use raw_cpuid::CpuId;

use super::clocksource::{compute_mult_shift, ClockSource};

/// Minimum TSC frequency (1 kHz) below which calibration is treated as failed.
const MIN_FREQ_HZ: u64 = 1_000;

/// A calibrated invariant-TSC clocksource.
#[derive(Debug)]
pub struct TscClock {
    source: ClockSource,
    /// How the frequency was obtained, for boot diagnostics.
    freq_source: &'static str,
    /// Whether CPUID advertised an invariant TSC.
    invariant: bool,
    /// Whether the TSC is usable as the system clocksource. True only when the
    /// TSC is validated as stable (invariant flag, or a paravirtual KVM TSC
    /// frequency). Calibration alone is not a stability guarantee. False under
    /// QEMU TCG, where the TSC advances at host real time while LAPIC/PIT advance
    /// at QEMU virtual time; the caller falls back to HPET.
    usable_as_clocksource: bool,
}

impl TscClock {
    pub fn source(&self) -> &ClockSource {
        &self.source
    }

    pub fn frequency_hz(&self) -> u64 {
        self.source.frequency_hz()
    }

    pub fn frequency_source(&self) -> &'static str {
        self.freq_source
    }

    pub fn is_invariant(&self) -> bool {
        self.invariant
    }

    /// Whether the TSC should be used as the system clocksource. True only when
    /// the TSC is validated as a stable, continuously advancing counter: either
    /// CPUID advertises an invariant TSC, or a paravirtual KVM TSC frequency is
    /// present. Calibration alone is **not** a stability guarantee. The caller
    /// falls back to HPET when this is false.
    pub fn usable_as_clocksource(&self) -> bool {
        self.usable_as_clocksource
    }

    /// Consume the backend, yielding the calibrated clocksource for storage.
    pub fn into_source(self) -> ClockSource {
        self.source
    }
}

/// The calibrated TSC frequency in Hz, or 0 before [`super::init`] has run.
///
/// Stored separately from the `ClockSource` so legacy callers (`timer::wait`,
/// procfs MHz display) can reach it without holding a reference to the
/// `OnceCell`-stored clocksource.
static TSC_FREQ_HZ: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Publish the calibrated TSC frequency for legacy callers. Called once by
/// [`super::init`].
pub(crate) fn publish_frequency(hz: u64) {
    TSC_FREQ_HZ.store(hz, core::sync::atomic::Ordering::Release);
}

/// Calibrated TSC frequency in Hz (0 before init).
pub fn frequency_hz() -> u64 {
    TSC_FREQ_HZ.load(core::sync::atomic::Ordering::Acquire)
}

/// Read the current TSC value with serialization.
///
/// `rdtscp` is preferred when available because it serializes the instruction
/// stream (no out-of-order read). Otherwise we use `lfence; rdtsc`, matching the
/// pattern already used elsewhere in the kernel.
#[inline]
pub fn read_cycles() -> u64 {
    if has_rdtscp() {
        // SAFETY: rdtscp is a userspace-safe, non-privileged instruction. The
        // `ecx` output (auxiliary TSC info) is discarded.
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!(
                "rdtscp",
                out("eax") lo,
                out("edx") hi,
                out("ecx") _,
                options(nostack, preserves_flags, readonly),
            );
        }
        ((hi as u64) << 32) | (lo as u64)
    } else {
        // SAFETY: lfence + rdtsc. lfence ensures prior instructions complete
        // before the TSC is read; rdtsc is non-privileged.
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!(
                "lfence",
                "rdtsc",
                out("eax") lo,
                out("edx") hi,
                options(nostack, preserves_flags, readonly),
            );
        }
        ((hi as u64) << 32) | (lo as u64)
    }
}

/// Whether `rdtscp` is available. Probed once during [`detect`] and stored in
/// an atomic so the hot [`read_cycles`] path is a single relaxed load.
static HAS_RDTSCP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn has_rdtscp() -> bool {
    HAS_RDTSCP.load(core::sync::atomic::Ordering::Relaxed)
}

/// Detect and calibrate a TSC clocksource.
///
/// The TSC frequency is always resolved when possible, because `timer::wait`
/// and LAPIC calibration need a TSC frequency for short busy-waits even when
/// the TSC is not usable as the system clocksource.
///
/// Frequency is resolved, in order of preference, by:
///  1. Hypervisor paravirtual TSC frequency (KVM leaf 0x40000010, exact kHz).
///     Also a stability signal: KVM guarantees TSC continuity across deschedule.
///  2. CPUID leaf 0x15 (`TscInfo::tsc_frequency()`) — exact, no measurement.
///  3. CPUID leaf 0x16 processor base frequency (MHz → Hz).
///  4. PIT-calibrated measurement against a known wall interval (cycle count,
///     **not** interrupt count).
///
/// Returns `None` if no frequency can be determined. Otherwise the caller checks
/// [`TscClock::usable_as_clocksource`] to decide whether to use the TSC as the
/// clocksource or fall back to HPET (#65). The TSC is usable only when validated
/// as stable (invariant flag or paravirtual frequency); calibration alone is not
/// a stability guarantee.
pub fn detect() -> Option<TscClock> {
    let cpuid = CpuId::new();

    // Probe rdtscp once and cache it for the hot read path.
    let rdtscp = cpuid
        .get_extended_processor_and_feature_identifiers()
        .map(|f| f.has_rdtscp())
        .unwrap_or(false);
    HAS_RDTSCP.store(rdtscp, core::sync::atomic::Ordering::Release);

    let invariant = cpuid
        .get_advanced_power_mgmt_info()
        .map(|a| a.has_invariant_tsc())
        .unwrap_or(false);

    // Resolve the TSC frequency, most-reliable source first:
    //  1. Hypervisor paravirtual TSC frequency (KVM leaf 0x40000010, exact kHz).
    //     This is the best source under KVM, where CPUID 0x15/0x16 are often
    //     unavailable (e.g. AMD hosts) and PIT calibration is less precise.
    //  2. CPUID leaf 0x15 (TscInfo::tsc_frequency()) — exact, no measurement.
    //  3. CPUID leaf 0x16 processor base frequency (MHz → Hz).
    //  4. PIT-calibrated measurement (fallback; see calibrate_with_pit).
    //
    // A paravirtual KVM TSC frequency (source 1) is also a *stability* signal:
    // KVM guarantees the TSC advances with real elapsed time even when the vCPU
    // is descheduled, so it is a valid clocksource regardless of the invariant
    // flag. We track whether we used it.
    let (freq_hz, freq_source, paravirtual) = cpuid
        .get_hypervisor_info()
        .and_then(|h| h.tsc_frequency())
        .filter(|&khz| khz > 0)
        .map(|khz| (khz as u64 * 1_000, "kvm.0x40000010", true))
        .or_else(|| {
            cpuid
                .get_tsc_info()
                .and_then(|t| t.tsc_frequency())
                .map(|f| (f, "cpuid.0x15", false))
        })
        .or_else(|| {
            cpuid
                .get_processor_frequency_info()
                .map(|p| (p.processor_base_frequency() as u64 * 1_000_000, "cpuid.0x16", false))
        })
        .unwrap_or_else(|| (calibrate_with_pit(), "pit-calibration", false));

    if freq_hz < MIN_FREQ_HZ {
        return None;
    }

    let (mult, shift) = compute_mult_shift(freq_hz);
    let epoch = read_cycles();
    let source = ClockSource::new(epoch, mult, shift, freq_hz);

    // The TSC is a valid clocksource only when validated as stable: either the
    // CPUID invariant-TSC flag is set, or a paravirtual KVM TSC frequency was
    // provided (KVM guarantees TSC continuity across deschedule). Calibration
    // alone (PIT/0x16) is NOT a stability guarantee — under QEMU TCG the TSC
    // advances at host real time while LAPIC/PIT advance at QEMU virtual time,
    // so a calibrated-only TSC diverges during `hlt`. The hypervisor vendor is
    // explicitly not used as a clock-quality test (#65): a stable/invariant or
    // paravirtual TSC is accepted on any hypervisor that advertises one.
    let usable_as_clocksource = invariant || paravirtual;

    Some(TscClock {
        source,
        freq_source,
        invariant,
        usable_as_clocksource,
    })
}

/// Calibrate the TSC frequency by counting TSC cycles over a PIT-measured
/// interval, using the PIT **counter latch** rather than the output pulse.
///
/// Reading the PIT's down-counter (via the read-latch command) gives a stable
/// wall-clock reference that does not depend on interrupt delivery and is not a
/// transient signal the vCPU can miss while descheduled. This mirrors Linux's
/// `pit_verify_tsc` approach. It is the fallback when CPUID does not enumerate
/// the TSC frequency directly (e.g. KVM `-cpu host` on AMD, where leaves
/// 0x15/0x16 are unsupported and the paravirtual leaf 0x40000010 is not exposed).
///
/// We program channel 2 in mode 2 (rate generator) with a large divisor, then
/// sample (TSC, PIT-count) twice. The PIT counts down at PIT_BASE_HZ; the
/// difference in counts (handling the modular wrap at the divisor) gives the
/// elapsed PIT ticks, and the TSC difference gives the elapsed cycles.
fn calibrate_with_pit() -> u64 {
    use x86_64::instructions::port::Port;

    const PIT_BASE_HZ: u64 = 1_193_182;
    // Large divisor so the counter rarely wraps during the measurement window.
    // 65535 gives ~55 ms per full countdown.
    const PIT_DIVISOR: u16 = 0xFFFF;

    unsafe {
        let mut cmd: Port<u8> = Port::new(0x43);
        let mut ch2: Port<u8> = Port::new(0x42);
        let mut port_b: Port<u8> = Port::new(0x61);

        // Channel 2, mode 2 (rate generator), lo/hi access, binary.
        cmd.write(0xb4);
        ch2.write((PIT_DIVISOR & 0xff) as u8);
        ch2.write((PIT_DIVISOR >> 8) as u8);

        // Enable channel 2 gate (bit 0), disable speaker (bit 1).
        let saved_b = port_b.read();
        port_b.write((saved_b & !0x02) | 0x01);

        // First sample.
        let tsc0 = read_cycles();
        let pit0 = read_ch2_count(&mut cmd, &mut ch2);

        // Spin ~20 ms by counting PIT down-ticks. The counter wraps modulo
        // PIT_DIVISOR+1, so use wrapping subtraction to get the elapsed count.
        let mut pit1 = pit0;
        let mut tsc1 = tsc0;
        // Target ~20 ms = ~23864 PIT ticks at 1.193 MHz.
        const TARGET_TICKS: u32 = 23_864;
        for _ in 0..1_000_000 {
            pit1 = read_ch2_count(&mut cmd, &mut ch2);
            let elapsed = (pit0.wrapping_sub(pit1)) & 0xFFFF;
            if elapsed >= TARGET_TICKS {
                tsc1 = read_cycles();
                break;
            }
        }

        // Restore port B.
        port_b.write(saved_b);

        let pit_delta = ((pit0.wrapping_sub(pit1)) & 0xFFFF) as u64;
        let tsc_delta = tsc1.wrapping_sub(tsc0);
        if pit_delta == 0 || tsc_delta == 0 {
            return 0;
        }
        // tsc_hz = tsc_delta * PIT_BASE_HZ / pit_delta
        tsc_delta * PIT_BASE_HZ / pit_delta
    }
}

/// Read the PIT channel 2 current count via the read-latch command.
///
/// The counter counts *down*, so a smaller reading means more time has elapsed.
/// We issue the latch command then read two bytes (lo then hi).
unsafe fn read_ch2_count(cmd: &mut x86_64::instructions::port::Port<u8>, ch2: &mut x86_64::instructions::port::Port<u8>) -> u32 {
    // Latch channel 2 count (read-back without disturbing operation).
    unsafe {
        cmd.write(0x80);
        let lo = ch2.read() as u32;
        let hi = ch2.read() as u32;
        (hi << 8) | lo
    }
}
