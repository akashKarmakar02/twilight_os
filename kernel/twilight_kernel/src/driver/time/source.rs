//! Selected clocksource backend policy.
//!
//! The monotonic timeline is read from exactly one continuously advancing
//! hardware backend, chosen once at boot by [`super::init`]. This module owns the
//! selected backend and the dispatch from `monotonic_ns()` to a hardware read.
//!
//! Selection order (see #65):
//!  1. validated invariant/paravirtual TSC (KVM `-cpu host` on an invariant
//!     host, or a paravirtual KVM TSC frequency);
//!  2. HPET (under QEMU TCG or when TSC validation fails);
//!  3. explicit initialization failure — there is no silent interrupt-count
//!     fallback.

use super::hpet::HpetClock;
use super::clocksource::ClockSource;

/// The continuously readable backend backing `CLOCK_MONOTONIC`.
///
/// Stored in a `OnceCell` so the hot read path is a single `get()` + match.
pub enum SelectedClocksource {
    /// Validated invariant/paravirtual TSC.
    Tsc(ClockSource),
    /// HPET main counter.
    Hpet(HpetClock),
}

impl SelectedClocksource {
    /// Read elapsed monotonic nanoseconds from the selected backend.
    ///
    /// Lock-free and allocation-free for both backends. The TSC path reads the
    /// TSC and applies the calibrated mult/shift; the HPET path reads the main
    /// counter MMIO register.
    #[inline]
    pub fn read_ns(&self) -> u64 {
        match self {
            SelectedClocksource::Tsc(cs) => cs.elapsed_ns(super::tsc::read_cycles()),
            SelectedClocksource::Hpet(hpet) => hpet.read_ns(),
        }
    }
}
