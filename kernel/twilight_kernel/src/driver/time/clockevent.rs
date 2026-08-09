//! One-shot clockevent policy: program the LAPIC for the earliest of the next
//! software deadline or the scheduler quantum deadline (#68).
//!
//! This is the policy layer above the LAPIC *mechanism* ([`crate::driver::apic::lapic`])
//! and below the *consumers* (the deadline queue in [`crate::sys::timer`] and the
//! scheduler quantum in [`crate::sys::preempt`]). It owns the armed-event state:
//! the absolute monotonic deadline currently programmed into the hardware.
//!
//! ## Invariants
//!
//! - The clockevent never advances `CLOCK_MONOTONIC`; it only reads it.
//! - An earlier insertion cannot be delayed behind an older programmed deadline.
//!   [`rearm_if_earlier`] atomically compare/replaces the armed deadline and
//!   reprograms the LAPIC when a new deadline is earlier.
//! - Idle with no deadlines disarms the timer, so no periodic interrupts fire.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::driver::apic::lapic;

/// Sentinel stored in [`ARMED_DEADLINE`] when no event is armed. `u64::MAX` is
/// never a real monotonic deadline (it would block for ~584 years).
const ARMED_NONE: u64 = u64::MAX;

/// The absolute monotonic deadline currently programmed into the LAPIC, or
/// [`ARMED_NONE`] when disarmed.
///
/// Touched under IRQ-disabled sections (hard-IRQ rearm, or `lock_irq` critical
/// sections in the blocking paths). The CAS in [`rearm_if_earlier`] makes an
/// earlier insertion win even if a concurrent rearm is choosing its next event.
static ARMED_DEADLINE: AtomicU64 = AtomicU64::new(ARMED_NONE);

/// Absolute deadline used by the legacy kernel
/// [`executor::sleep`](crate::task::executor::sleep) HLT loop.
///
/// Unlike process sleeps, this early-boot/kernel-context wait has no process
/// that can be inserted into the scheduler deadline queue. It still needs to
/// participate in one-shot clockevent selection; otherwise `enable_and_hlt()`
/// can sleep forever once the periodic PIT tick has been disabled (#68).
/// A value of 0 means that no kernel HLT wake is pending.
static KERNEL_HLT_DEADLINE: AtomicU64 = AtomicU64::new(0);

/// Initialize the clockevent subsystem. Called after the LAPIC is calibrated.
pub fn init() {
    ARMED_DEADLINE.store(ARMED_NONE, Ordering::Relaxed);
    KERNEL_HLT_DEADLINE.store(0, Ordering::Relaxed);
    // Arm the first event for the current earliest deadline (if any). At boot
    // there are no sleepers and the quantum is not yet armed, so this typically
    // disarms; the first context switch arms the quantum.
    rearm(crate::driver::time::monotonic_ns());
}

/// Publish a deadline that must wake a kernel-context HLT loop.
///
/// Bootstrap and scheduled kernel work currently run only on the BSP, so one
/// slot is sufficient. Recompute the complete clockevent target because a
/// previously armed quantum/software deadline may be either earlier or later.
pub fn arm_kernel_hlt_wake(deadline_ns: u64) {
    KERNEL_HLT_DEADLINE.store(deadline_ns, Ordering::Release);
    rearm(crate::driver::time::monotonic_ns());
}

/// Remove a kernel HLT wake published by [`arm_kernel_hlt_wake`].
pub fn clear_kernel_hlt_wake() {
    KERNEL_HLT_DEADLINE.store(0, Ordering::Release);
    rearm(crate::driver::time::monotonic_ns());
}

/// Consume a due kernel HLT deadline before the timer ISR chooses its next
/// event. The interrupt itself wakes `enable_and_hlt()`; clearing here prevents
/// [`rearm`] from repeatedly selecting the already-expired deadline.
pub fn account_kernel_hlt_wake(now_ns: u64) {
    let deadline = KERNEL_HLT_DEADLINE.load(Ordering::Acquire);
    if deadline != 0 && now_ns >= deadline {
        KERNEL_HLT_DEADLINE.store(0, Ordering::Release);
    }
}

/// Program the LAPIC for the earliest software, scheduler-quantum, or kernel-HLT
/// deadline, or disarm if all three are absent.
///
/// Called from hard-IRQ context (after expiry, before EOI) and from task context
/// after a deadline is inserted or cancelled. Task-context callers may run with
/// interrupts enabled, so the sample/store/program sequence is wrapped in
/// [`without_interrupts`]: this prevents a hard-IRQ [`rearm`] from interleaving
/// with this call and clobbering a value sampled before the interrupt. With
/// interrupts disabled on the (only) running CPU, the armed-deadline state is
/// exclusively owned for the duration of the call, so a plain store is safe.
pub fn rearm(now_ns: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let target = earliest_deadline();
        match target {
            None => {
                ARMED_DEADLINE.store(ARMED_NONE, Ordering::Release);
                lapic::cancel_timer();
            }
            Some(abs) => {
                ARMED_DEADLINE.store(abs, Ordering::Release);
                let delta = abs.saturating_sub(now_ns).max(lapic::min_delta_ns());
                lapic::program_oneshot_ns(delta);
            }
        }
    });
}

/// Decide whether `new_abs` should replace the currently armed deadline `cur`.
///
/// A real deadline always replaces [`ARMED_NONE`] (the "no event" sentinel), and
/// a strictly earlier deadline replaces a later one. Equal or later deadlines do
/// not replace — the armed event already fires no later than `new_abs`.
#[inline]
fn should_replace(cur: u64, new_abs: u64) -> bool {
    new_abs < cur
}

/// Atomically reprogram the LAPIC if `new_abs` is earlier than the currently
/// armed deadline. This is the **final earliest-deadline recheck** called inside
/// the queue `lock_irq` critical section before releasing it, so an insertion
/// racing with IRQ rearm cannot be lost: if the new deadline is earlier, it wins
/// the CAS and the hardware is reprogrammed immediately.
///
/// `now_ns` is the current monotonic time, used to convert the absolute
/// deadline to a delta. Callers must hold IRQs disabled.
pub fn rearm_if_earlier(now_ns: u64, new_abs: u64) {
    loop {
        let cur = ARMED_DEADLINE.load(Ordering::Acquire);
        if !should_replace(cur, new_abs) {
            return; // Already armed no later than the new deadline.
        }
        if ARMED_DEADLINE
            .compare_exchange(cur, new_abs, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let delta = new_abs.saturating_sub(now_ns).max(lapic::min_delta_ns());
            lapic::program_oneshot_ns(delta);
            return;
        }
        // Lost the race to a concurrent writer; retry.
    }
}

/// Disarm unconditionally and clear the armed state. Used when the running task
/// enters idle with no peer to preempt for and no sleepers.
pub fn disarm() {
    ARMED_DEADLINE.store(ARMED_NONE, Ordering::Release);
    lapic::cancel_timer();
}

/// The earliest software, scheduler-quantum, or kernel-HLT deadline.
fn earliest_deadline() -> Option<u64> {
    let sw = crate::sys::timer::next_deadline_ns();
    let q = crate::sys::preempt::quantum_deadline_ns();
    let kernel_hlt = match KERNEL_HLT_DEADLINE.load(Ordering::Acquire) {
        0 => None,
        deadline => Some(deadline),
    };
    [sw, q, kernel_hlt].into_iter().flatten().min()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_none_sentinel_is_never_a_real_deadline() {
        // u64::MAX would block ~584 years; safe as "no event".
        assert_eq!(ARMED_NONE, u64::MAX);
    }

    #[test]
    fn earliest_of_all_sources_picks_the_smaller() {
        // Mirrors earliest_deadline's min logic without touching globals.
        fn earliest(sw: Option<u64>, q: Option<u64>, kernel_hlt: Option<u64>) -> Option<u64> {
            [sw, q, kernel_hlt].into_iter().flatten().min()
        }
        assert_eq!(earliest(Some(100), Some(200), Some(300)), Some(100));
        assert_eq!(earliest(Some(200), Some(100), Some(300)), Some(100));
        assert_eq!(earliest(Some(300), Some(200), Some(100)), Some(100));
        assert_eq!(earliest(Some(100), None, None), Some(100));
        assert_eq!(earliest(None, Some(100), None), Some(100));
        assert_eq!(earliest(None, None, Some(100)), Some(100));
        assert_eq!(earliest(None, None, None), None);
    }

    /// `should_replace` must not overwrite an earlier armed deadline with a later
    /// one, and must replace when the new deadline is strictly earlier. Tested
    /// against the pure predicate directly so the test does not depend on LAPIC
    /// hardware.
    #[test]
    fn should_replace_rejects_later_or_equal() {
        // A later deadline must not replace an earlier armed one.
        assert!(!should_replace(1000, 2000));
        // An equal deadline must not replace (no point reprogramming).
        assert!(!should_replace(1000, 1000));
    }

    #[test]
    fn should_replace_accepts_strictly_earlier() {
        assert!(should_replace(2000, 1000));
    }

    #[test]
    fn should_replace_treats_none_as_earliest() {
        // Any real deadline is earlier than ARMED_NONE, so it replaces.
        assert!(should_replace(ARMED_NONE, 5_000_000));
        // ARMED_NONE itself does not replace a real deadline.
        assert!(!should_replace(5_000_000, ARMED_NONE));
    }
}
