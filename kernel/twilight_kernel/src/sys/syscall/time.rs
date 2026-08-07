//! Time-syscall policy: `nanosleep`, `clock_nanosleep`, `clock_gettime` (#67).
//!
//! This module owns the POSIX sleep/wake *policy* — timespec validation,
//! fault-aware user copies, deadline construction, signal-interruption and
//! remainder computation. It deliberately does **not** live in the hardware
//! clocksource module (`driver::time`), which only reads the clock and reports
//! clock events. Process blocking happens on the scheduler deadline queue
//! ([`crate::sys::timer::block_current_until`], #66); userspace sleep never
//! calls [`crate::driver::timer::wait`] and never busy-spins.
//!
//! ## Root-cause notes
//!
//! Previously `sleep_ns`/`sleep_timespec` lived in `driver::time` and the
//! dispatcher dereferenced raw userspace pointers. That conflated policy with
//! the clocksource, routed sub-tick sleeps through a TSC spin-wait meant for
//! ATA/USB hardware delays, ignored signal interruption, and pre-zeroed `rem`
//! before knowing whether interruption occurred. All of that is fixed here.

use twilight_common::syscall::types::{EFAULT, EINTR, EINVAL, Timespec};

use crate::driver::time::{self, CLOCK_MONOTONIC, CLOCK_REALTIME};
use crate::sys::syscall::utils::{copy_from_user, copy_to_user};
use crate::sys::timer::{WakeReason, block_current_until};

const NSEC_PER_SEC: u64 = 1_000_000_000;
const TIMER_ABSTIME: i32 = 1;

/// Why a sleep resumed. The caller translates this into the syscall return value
/// and optional `rem` write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SleepOutcome {
    /// The absolute deadline was reached (or already in the past).
    DeadlineReached,
    /// An interrupting signal (or cancellation) ended the wait before the
    /// deadline. `remaining_ns` is `max(deadline - now, 0)`.
    Interrupted { remaining_ns: u64 },
}

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/// Validate a `Timespec`'s fields. POSIX requires `tv_sec >= 0` and
/// `0 <= tv_nsec < 1_000_000_000`.
fn validate_timespec(req: &Timespec) -> Result<(), i64> {
    if req.tv_sec < 0 || req.tv_nsec < 0 || req.tv_nsec >= NSEC_PER_SEC as i64 {
        return Err(-(EINVAL as i64));
    }
    Ok(())
}

/// Convert a validated `Timespec` to integer nanoseconds using checked/saturating
/// `u128` arithmetic. Saturates to `u64::MAX`; never wraps into an immediate
/// deadline. No `f64`.
fn timespec_to_ns(req: &Timespec) -> u64 {
    let secs = req.tv_sec as u128;
    let nsec = req.tv_nsec as u128;
    let total = secs.saturating_mul(NSEC_PER_SEC as u128).saturating_add(nsec);
    core::cmp::min(total, u64::MAX as u128) as u64
}

/// Read the current value of `clockid`. Returns `-EINVAL` for unsupported clocks.
fn clock_now_ns(clockid: i32) -> Result<u64, i64> {
    match clockid {
        CLOCK_MONOTONIC => Ok(time::monotonic_ns()),
        CLOCK_REALTIME => Ok(time::realtime_ns()),
        _ => Err(-(EINVAL as i64)),
    }
}

/// Translate an absolute `clockid`-domain deadline to a monotonic deadline for
/// the deadline queue. `CLOCK_MONOTONIC` passes through; `CLOCK_REALTIME`
/// subtracts the fixed boot-time realtime-minus-monotonic offset.
fn absolute_to_monotonic_deadline(clockid: i32, abs_ns: u64) -> Result<u64, i64> {
    match clockid {
        CLOCK_MONOTONIC => Ok(abs_ns),
        CLOCK_REALTIME => {
            // REALTIME = OFFSET + MONOTONIC  =>  MONOTONIC = REALTIME - OFFSET.
            // Saturating subtract: a realtime deadline earlier than the boot
            // epoch maps to monotonic 0 (i.e. "in the past"), which the caller
            // returns from immediately.
            //
            // Establish the epoch first; an uninitialized offset of 0 would
            // pass an absolute epoch deadline (~1.7e18 ns) through unchanged as
            // a monotonic deadline, blocking for decades.
            time::ensure_realtime_offset_inited();
            let offset = time::realtime_offset_ns();
            Ok(abs_ns.saturating_sub(offset))
        }
        _ => Err(-(EINVAL as i64)),
    }
}

/// Block on the deadline queue until `deadline_ns` (absolute monotonic) or an
/// interrupting signal. Rechecks the wake reason and deadline after every wake
/// to tolerate spurious wakeups without returning early.
///
/// Returns the outcome and, on interruption, the remaining nanoseconds
/// (`max(deadline - now, 0)`).
fn sleep_until_deadline(deadline_ns: u64) -> SleepOutcome {
    loop {
        let reason = block_current_until(deadline_ns);

        match reason {
            WakeReason::Deadline | WakeReason::Event => {
                // A Deadline/Event wake means the queue expired our entry. Verify
                // the clock actually reached the deadline; if not (spurious or
                // a race), loop and block again rather than returning early.
                let now = time::monotonic_ns();
                if now >= deadline_ns {
                    return SleepOutcome::DeadlineReached;
                }
                // Spurious: re-block on the same deadline.
            }
            WakeReason::Signal | WakeReason::Cancelled => {
                // An interrupting signal (or cancellation) ended the wait. Compute
                // the relative remainder for the caller. `max(deadline - now, 0)`
                // — a signal that races with expiry yields zero remainder.
                let now = time::monotonic_ns();
                let remaining_ns = deadline_ns.saturating_sub(now);
                return SleepOutcome::Interrupted { remaining_ns };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Syscall entry points
// ---------------------------------------------------------------------------

/// `nanosleep(req, rem)` — relative sleep on `CLOCK_MONOTONIC`.
///
/// Copies `req` into kernel storage before validation/blocking. On interruption
/// by a caught, unmasked signal, writes `max(deadline - now, 0)` to `rem` (if
/// non-null) and returns `-EINTR`. Does not pre-zero `rem`.
pub fn sys_nanosleep(req_ptr: *const Timespec, rem_ptr: *mut Timespec) -> i64 {
    let req = match copy_from_user(req_ptr) {
        Ok(r) => r,
        Err(_) => return -(EFAULT as i64),
    };

    if let Err(e) = validate_timespec(&req) {
        return e;
    }

    let duration_ns = timespec_to_ns(&req);

    // A zero-duration relative sleep returns immediately without blocking.
    if duration_ns == 0 {
        return 0;
    }

    let deadline = time::monotonic_ns().saturating_add(duration_ns);
    match sleep_until_deadline(deadline) {
        SleepOutcome::DeadlineReached => 0,
        SleepOutcome::Interrupted { remaining_ns } => {
            // Only a relative sleep writes a remainder, and only on
            // interruption. Best-effort: if rem is unmapped we still report
            // -EINTR (the sleep did run and was interrupted).
            if !rem_ptr.is_null() {
                let rem = ns_to_timespec(remaining_ns);
                let _ = copy_to_user(rem_ptr, rem);
            }
            -(EINTR as i64)
        }
    }
}

/// `clock_nanosleep(clockid, flags, req, rem)`.
///
/// Supports `CLOCK_MONOTONIC` and `CLOCK_REALTIME`, relative (`flags == 0`) and
/// absolute (`TIMER_ABSTIME`). Relative sleeps write `rem` on interruption;
/// absolute sleeps never write `rem`. Past absolute deadlines return immediately.
pub fn sys_clock_nanosleep(
    clockid: i32,
    flags: i32,
    req_ptr: *const Timespec,
    rem_ptr: *mut Timespec,
) -> i64 {
    // Validate flags before touching user memory: only TIMER_ABSTIME is
    // supported in the current ABI.
    if flags & !TIMER_ABSTIME != 0 {
        return -(EINVAL as i64);
    }

    let req = match copy_from_user(req_ptr) {
        Ok(r) => r,
        Err(_) => return -(EFAULT as i64),
    };

    if let Err(e) = validate_timespec(&req) {
        return e;
    }

    // Reject unsupported clock IDs after flag validation but before blocking.
    match clockid {
        CLOCK_MONOTONIC | CLOCK_REALTIME => {}
        _ => return -(EINVAL as i64),
    }

    if flags & TIMER_ABSTIME != 0 {
        // Absolute sleep: translate the requested deadline to monotonic and
        // block on it. Absolute sleeps never provide a remainder.
        let abs_ns = timespec_to_ns(&req);
        let deadline_ns = match absolute_to_monotonic_deadline(clockid, abs_ns) {
            Ok(d) => d,
            Err(e) => return e,
        };

        // A past absolute deadline returns immediately.
        let now = time::monotonic_ns();
        if deadline_ns <= now {
            return 0;
        }

        match sleep_until_deadline(deadline_ns) {
            SleepOutcome::DeadlineReached => 0,
            SleepOutcome::Interrupted { .. } => {
                // Absolute sleeps do not provide a relative remainder.
                -(EINTR as i64)
            }
        }
    } else {
        // Relative sleep on the selected clock domain. Relative sleeps read the
        // clock once and create one absolute monotonic deadline.
        let duration_ns = timespec_to_ns(&req);
        if duration_ns == 0 {
            return 0;
        }

        // A relative duration is identical in the CLOCK_MONOTONIC and
        // CLOCK_REALTIME domains, and the deadline queue is monotonic, so the
        // base must be the monotonic clock. Using clock_now_ns(CLOCK_REALTIME)
        // here would add the boot epoch offset (~1.7e18 ns) to the deadline and
        // block far beyond the requested interval.
        let deadline = time::monotonic_ns().saturating_add(duration_ns);

        match sleep_until_deadline(deadline) {
            SleepOutcome::DeadlineReached => 0,
            SleepOutcome::Interrupted { remaining_ns } => {
                if !rem_ptr.is_null() {
                    let rem = ns_to_timespec(remaining_ns);
                    let _ = copy_to_user(rem_ptr, rem);
                }
                -(EINTR as i64)
            }
        }
    }
}

/// `clock_gettime(clockid, tp)` — write the current clock value to userspace.
///
/// Fault-aware: an unmapped `tp` returns `-EFAULT` without writing. Moved here
/// from `driver::time::sys_clock_gettime` so the clocksource module no longer
/// touches userspace pointers.
pub fn sys_clock_gettime(clockid: i32, tp_ptr: *mut Timespec) -> i64 {
    if tp_ptr.is_null() {
        return -(EFAULT as i64);
    }

    // Ensure the realtime epoch is established before a realtime read. This
    // mirrors the lazy init previously done in driver::time::sys_clock_gettime.
    if clockid == CLOCK_REALTIME {
        time::ensure_realtime_offset_inited();
    }

    let ns = match clock_now_ns(clockid) {
        Ok(n) => n,
        Err(e) => return e,
    };

    let ts = ns_to_timespec(ns);
    match copy_to_user(tp_ptr, ts) {
        Ok(()) => 0,
        Err(_) => -(EFAULT as i64),
    }
}

/// Split integer nanoseconds into a `Timespec`. Used for both `clock_gettime`
/// output and the `rem` remainder.
fn ns_to_timespec(ns: u64) -> Timespec {
    Timespec {
        tv_sec: (ns / NSEC_PER_SEC) as i64,
        tv_nsec: (ns % NSEC_PER_SEC) as i64,
    }
}

// ---------------------------------------------------------------------------
// Host-testable units
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(sec: i64, nsec: i64) -> Timespec {
        Timespec {
            tv_sec: sec,
            tv_nsec: nsec,
        }
    }

    #[test]
    fn valid_timespec_passes_validation() {
        assert!(validate_timespec(&ts(0, 0)).is_ok());
        assert!(validate_timespec(&ts(1, 500_000_000)).is_ok());
        assert!(validate_timespec(&ts(1_000_000, 999_999_999)).is_ok());
    }

    #[test]
    fn negative_tv_sec_rejected() {
        assert_eq!(validate_timespec(&ts(-1, 0)), Err(-(EINVAL as i64)));
    }

    #[test]
    fn negative_tv_nsec_rejected() {
        assert_eq!(validate_timespec(&ts(0, -1)), Err(-(EINVAL as i64)));
    }

    #[test]
    fn tv_nsec_at_or_above_one_second_rejected() {
        assert_eq!(
            validate_timespec(&ts(0, 1_000_000_000)),
            Err(-(EINVAL as i64))
        );
        assert_eq!(
            validate_timespec(&ts(1, 1_000_000_001)),
            Err(-(EINVAL as i64))
        );
    }

    #[test]
    fn timespec_to_ns_basic() {
        assert_eq!(timespec_to_ns(&ts(0, 0)), 0);
        assert_eq!(timespec_to_ns(&ts(0, 500_000_000)), 500_000_000);
        assert_eq!(timespec_to_ns(&ts(1, 0)), 1_000_000_000);
        assert_eq!(timespec_to_ns(&ts(2, 250_000_000)), 2_250_000_000);
    }

    #[test]
    fn timespec_to_ns_saturates_on_overflow() {
        // ~584 years worth of seconds saturates rather than wrapping.
        let huge = ts(i64::MAX, 999_999_999);
        let ns = timespec_to_ns(&huge);
        assert_eq!(ns, u64::MAX);
    }

    #[test]
    fn ns_to_timespec_round_trip() {
        for &ns in &[0u64, 1, 999_999_999, 1_000_000_000, 2_500_000_007] {
            let t = ns_to_timespec(ns);
            assert_eq!(timespec_to_ns(&t), ns);
        }
    }

    #[test]
    fn ns_to_timespec_splits_correctly() {
        let t = ns_to_timespec(1_500_000_007);
        assert_eq!(t.tv_sec, 1);
        assert_eq!(t.tv_nsec, 500_000_007);
    }

    #[test]
    fn absolute_monotonic_passthrough() {
        assert_eq!(
            absolute_to_monotonic_deadline(CLOCK_MONOTONIC, 123_456).unwrap(),
            123_456
        );
    }

    #[test]
    fn absolute_realtime_subtracts_offset() {
        // With offset 1000, realtime deadline 1500 -> monotonic 500.
        let offset = time::realtime_offset_ns();
        let realtime_deadline = offset.saturating_add(500);
        assert_eq!(
            absolute_to_monotonic_deadline(CLOCK_REALTIME, realtime_deadline).unwrap(),
            500
        );
    }

    #[test]
    fn absolute_realtime_before_epoch_clamps_to_zero() {
        // A realtime deadline earlier than the boot offset maps to monotonic 0.
        let offset = time::realtime_offset_ns();
        let earlier = offset.saturating_sub(1);
        let m = absolute_to_monotonic_deadline(CLOCK_REALTIME, earlier).unwrap();
        // If offset is 0 this is 0; otherwise earlier < offset so result is 0.
        assert_eq!(m, 0);
    }

    #[test]
    fn unsupported_clock_rejected() {
        assert_eq!(clock_now_ns(99), Err(-(EINVAL as i64)));
        assert_eq!(
            absolute_to_monotonic_deadline(99, 0),
            Err(-(EINVAL as i64))
        );
    }

    #[test]
    fn ns_to_timespec_max_does_not_panic() {
        let t = ns_to_timespec(u64::MAX);
        // tv_sec is the floor of u64::MAX / 1e9; tv_nsec is the remainder.
        assert!(t.tv_sec >= 0);
        assert!(t.tv_nsec >= 0 && t.tv_nsec < 1_000_000_000);
    }
}
