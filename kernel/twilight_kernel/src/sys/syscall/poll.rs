//! `poll` / `ppoll` / `select` ABI and shared deadline-aware wait loop (#69).
//!
//! This module owns the POSIX multiplexed-wait *policy*: pollfd/fd-set
//! readiness scanning, timeout construction, signal interruption (`-EINTR`),
//! and the `ppoll` temporary signal mask. Blocking happens on the scheduler
//! deadline queue via [`crate::sys::proc::await_io_until`], which arms a single
//! absolute `IoTimeout` deadline; there is no per-tick wake-all.
//!
//! ## Root-cause notes
//!
//! Previously `on_timer_tick()` woke every global poll waiter every millisecond.
//! Each waiter became runnable, rescanned its descriptors, and blocked again —
//! a thundering herd that inflated scheduler and TCG load and delayed unrelated
//! short sleeps. The unconditional broadcast is gone (#68); these syscalls now
//! block on the deadline queue and wake only on a readiness event, a timeout
//! expiry, or an interrupting signal.
//!
//! ## Lost-wakeup-safe order
//!
//! Each wait iteration preserves the order required to close the arm-vs-wake
//! race:
//!
//! 1. inspect readiness (and pump the network stack so protocol-timer-driven
//!    readiness is observed);
//! 2. publish the current waiter on the global poll queue;
//! 3. recheck readiness and deadline;
//! 4. atomically block via `await_io_until`;
//! 5. after wake, unregister and recheck readiness, deadline, and signals.
//!
//! `pending_io` (set by `wake_process`/`wake_from_timer`) handles an event that
//! occurs just before the process enters `AwaitingIo`.

use crate::sys;
use crate::sys::proc::{self, Process, PROCESS_TABLE};
use crate::sys::timer::WakeReason;
use twilight_common::syscall::types::{
    EFAULT, EINTR, EINVAL, ESRCH, FdSet, O_ACCMODE, O_RDONLY, PollFd, POLLERR, POLLHUP, POLLIN,
    POLLNVAL, POLLOUT, POLLPRI, Timeval, FD_SETSIZE,
};

const NSEC_PER_SEC: u64 = 1_000_000_000;

// ---------------------------------------------------------------------------
// Readiness scan
// ---------------------------------------------------------------------------

/// Scan `fds` for readiness, writing `revents` and returning the ready count.
///
/// Borrowed from the previous in-`service` implementation: it clones each fd's
/// `OpenFile` (bumping the `Arc` refcount, never touching the fd table lock)
/// and polls the underlying object. A bad fd records `POLLNVAL` and counts as
/// ready. For TCP/UDP sockets, `poll()` drives `iface.poll` inline, so a
/// readiness scan also advances the smoltcp stack.
fn poll_fd_set(fds: &mut [PollFd], process: &mut Process) -> Result<usize, i64> {
    let mut ready_count = 0;

    for pfd in fds.iter_mut() {
        pfd.revents = 0;
        let fd = pfd.fd;
        if fd < 0 {
            continue;
        }

        let want_in = (pfd.events & POLLIN) != 0;
        let want_out = (pfd.events & POLLOUT) != 0;
        let mut revents: i16 = 0;

        let file_ref = match crate::sys::syscall::service::clone_open_file(process, fd) {
            Ok(file) => file,
            Err(_) => {
                pfd.revents = POLLNVAL;
                ready_count += 1;
                continue;
            }
        };
        let mut file = file_ref.lock();
        use crate::sys::proc::OpenFileKind;
        match &mut file.kind {
            OpenFileKind::Vfs(node_ref) => {
                if want_in {
                    match node_ref.lock().poll() {
                        Ok(true) => revents |= POLLIN,
                        Ok(false) => {}
                        Err(_) => revents |= POLLERR,
                    }
                }
                if want_out && file.status_flags & O_ACCMODE != O_RDONLY {
                    revents |= POLLOUT;
                }
            }
            OpenFileKind::Pipe(pipe) => {
                let state = pipe.poll();
                if want_in && state.readable {
                    revents |= POLLIN;
                }
                if want_out && state.writable {
                    revents |= POLLOUT;
                }
                if state.hangup {
                    revents |= POLLHUP;
                }
                if state.error {
                    revents |= POLLERR;
                }
            }
            OpenFileKind::Socket(sock) => match sock {
                crate::sys::net::socket::SocketFile::Unix(_) => {
                    let ps = sock.poll_unix();
                    if want_in && ps.readable {
                        revents |= POLLIN;
                    }
                    if want_out && ps.writable {
                        revents |= POLLOUT;
                    }
                    if ps.hangup {
                        revents |= POLLHUP;
                    }
                    if ps.error {
                        revents |= POLLERR;
                    }
                }
                _ => {
                    use crate::driver::disk::ata::IO;
                    if want_in && sock.poll(IO::Read) {
                        revents |= POLLIN;
                    }
                    if want_out && sock.poll(IO::Write) {
                        revents |= POLLOUT;
                    }
                }
            },
            OpenFileKind::MemFd(_) => {
                if want_in {
                    revents |= POLLIN;
                }
                if want_out && file.status_flags & O_ACCMODE != O_RDONLY {
                    revents |= POLLOUT;
                }
            }
        }

        if revents != 0 {
            pfd.revents = revents;
            ready_count += 1;
        }
    }

    Ok(ready_count)
}

fn poll_fd_set_for_pid(fds: &mut [PollFd], pid: u16) -> Result<usize, i64> {
    #[allow(static_mut_refs)]
    let proc_opt = unsafe { PROCESS_TABLE.get_mut().unwrap().get_process(pid) };
    let Some(process) = proc_opt else {
        return Err(-(ESRCH as i64));
    };

    poll_fd_set(fds, process)
}

// ---------------------------------------------------------------------------
// Pure helpers (deadline construction)
// ---------------------------------------------------------------------------

/// Convert a `poll`/`ppoll` relative millisecond timeout to an absolute
/// monotonic deadline. `None` (infinite) is preserved. Saturates on overflow so
/// a huge timeout cannot wrap into an immediate expiry.
fn poll_timeout_to_deadline(timeout_ms: isize, now_ns: u64) -> Option<u64> {
    if timeout_ms < 0 {
        None
    } else {
        let delta_ns = (timeout_ms as u64).saturating_mul(1_000_000);
        Some(now_ns.saturating_add(delta_ns))
    }
}

/// Convert a validated `Timeval` to a relative nanosecond duration using
/// saturating arithmetic. The caller must have validated the fields with
/// [`valid_timeval`] first.
fn timeval_to_ns(tv: Timeval) -> u64 {
    (tv.tv_sec as u64)
        .saturating_mul(NSEC_PER_SEC)
        .saturating_add((tv.tv_usec as u64).saturating_mul(1000))
}

// ---------------------------------------------------------------------------
// Shared wait loop
// ---------------------------------------------------------------------------

/// Block on `fds` until ready, `deadline_ns`, or a signal. Returns the ready
/// count (0 on timeout), or `-EINTR` on signal interruption.
///
/// `deadline_ns` is `None` for an infinite wait. The caller has already done
/// the initial readiness scan and zero-timeout short-circuit; this loop only
/// runs when blocking is actually required.
fn wait_for_ready(
    fds: &mut [PollFd],
    current_pid: u16,
    deadline_ns: Option<u64>,
) -> Result<i64, i64> {
    let wait_queue = proc::poll_wait_queue();

    loop {
        // (1) Pump the network stack before each scan so protocol-timer-driven
        //     TCP/UDP readiness (retransmit, peer segment, TIME-WAIT) is
        //     observed without a per-tick broadcast (#69).
        let _ = sys::net::pump_network();

        // Check for a pending interrupting signal first: one queued before we
        // publish must abort now, not after a full block.
        if proc::current_has_sleep_interrupting_signal() {
            return Err(-(EINTR as i64));
        }

        if let Some(deadline) = deadline_ns {
            if crate::driver::time::monotonic_ns() >= deadline {
                return Ok(0);
            }
        }

        // (2) Publish the current waiter.
        let wait_pid = wait_queue.prepare_current();

        // (3) Recheck readiness and deadline under publication.
        let ready = match poll_fd_set_for_pid(fds, current_pid) {
            Ok(n) => n,
            Err(e) => {
                wait_queue.finish_wait(wait_pid);
                return Err(e);
            }
        };

        if ready > 0 {
            wait_queue.finish_wait(wait_pid);
            return Ok(ready as i64);
        }

        if let Some(deadline) = deadline_ns {
            if crate::driver::time::monotonic_ns() >= deadline {
                wait_queue.finish_wait(wait_pid);
                return Ok(0);
            }
        }

        // (4) Atomically block. await_io_until arms/cancels the IoTimeout token
        //     itself after entering AwaitingIo state, closing the arm-vs-wake
        //     race, and returns the reason execution resumed.
        let reason = proc::await_io_until(deadline_ns);
        wait_queue.finish_wait(wait_pid);

        // (5) After wake, recheck readiness, deadline, and signals. A signal
        //     wake takes precedence: return -EINTR regardless of readiness, per
        //     POSIX (a caught signal during poll interrupts it).
        match reason {
            WakeReason::Signal | WakeReason::Cancelled => {
                return Err(-(EINTR as i64));
            }
            WakeReason::Deadline => {
                // Timeout expiry. Perform the final readiness scan first:
                // readiness observable at the moment of expiry wins over the
                // timeout.
                match poll_fd_set_for_pid(fds, current_pid) {
                    Ok(n) if n > 0 => return Ok(n as i64),
                    Ok(_) => return Ok(0),
                    Err(e) => return Err(e),
                }
            }
            WakeReason::Event => {
                // Readiness (or spurious) wake: re-scan and loop.
                match poll_fd_set_for_pid(fds, current_pid) {
                    Ok(n) if n > 0 => return Ok(n as i64),
                    Ok(_) => {}
                    Err(e) => return Err(e),
                }
                // Spurious: fall through to the next iteration.
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Syscall entry points
// ---------------------------------------------------------------------------

/// `poll(fds, nfds, timeout_ms)` — poll a set of descriptors.
///
/// `timeout_ms < 0` means infinite wait; `0` returns immediately; a positive
/// value is an absolute monotonic deadline armed on the one-shot clockevent.
pub fn poll(fds_ptr: usize, nfds: usize, timeout_ms: isize) -> i64 {
    let current_pid = sys::proc::id();

    // nfds == 0 still obeys timeout/signal semantics: zero timeout returns
    // immediately, finite timeout sleeps until expiry or signal, infinite
    // blocks until a signal (#69 ABI fix).
    let fds: &mut [PollFd] = if nfds == 0 {
        &mut []
    } else {
        if fds_ptr == 0 {
            return -(EFAULT as i64);
        }
        // SAFETY: the caller provides a valid user buffer of `nfds` PollFd.
        // The slice is only used within this syscall and not retained across
        // the block (results are written back through it before return).
        unsafe { core::slice::from_raw_parts_mut(fds_ptr as *mut PollFd, nfds) }
    };

    // Initial readiness scan.
    let ready = match poll_fd_set_for_pid(fds, current_pid) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if ready > 0 {
        return ready as i64;
    }
    if timeout_ms == 0 {
        return 0;
    }

    let deadline_ns = poll_timeout_to_deadline(timeout_ms, crate::driver::time::monotonic_ns());

    match wait_for_ready(fds, current_pid, deadline_ns) {
        Ok(n) => n,
        Err(e) => e,
    }
}

/// `ppoll(fds, nfds, tmo_p, sigmask, sigsetsize)` — poll with a temporary
/// signal mask.
///
/// The temporary mask is installed atomically before the wait and restored on
/// every return path. `tmo_p == 0` means infinite wait. A NULL `sigmask` leaves
/// the mask unchanged.
pub fn ppoll(
    fds_ptr: usize,
    nfds: usize,
    tmo_p: usize,
    sigmask_ptr: usize,
    sigsetsize: usize,
) -> i64 {
    let current_pid = sys::proc::id();

    let fds: &mut [PollFd] = if nfds == 0 {
        &mut []
    } else {
        if fds_ptr == 0 {
            return -(EFAULT as i64);
        }
        unsafe { core::slice::from_raw_parts_mut(fds_ptr as *mut PollFd, nfds) }
    };

    // Resolve the timeout into an absolute monotonic deadline before touching
    // the signal mask, so a validation failure does not leave the mask swapped.
    let deadline_ns = if tmo_p != 0 {
        let ts_ptr = tmo_p as *const twilight_common::syscall::types::Timespec;
        let ts = unsafe { &*ts_ptr };
        if ts.tv_sec == 0 && ts.tv_nsec == 0 {
            // Zero timeout: still scan once for readiness, but do not block.
            // (Falls through to the initial scan + immediate-return path below.)
        }
        if let Err(e) = crate::sys::syscall::time::validate_timespec(ts) {
            return e;
        }
        let now_ns = crate::driver::time::monotonic_ns();
        let delta_ns = crate::sys::syscall::time::timespec_to_ns(ts);
        Some(now_ns.saturating_add(delta_ns))
    } else {
        None
    };

    // Install the temporary signal mask atomically, saving the old mask.
    let saved_mask = if sigmask_ptr != 0 {
        let copy_len = core::cmp::min(sigsetsize, core::mem::size_of::<[u64; 2]>());
        let mut new_mask = [0u64; 2];
        if copy_len != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    sigmask_ptr as *const u8,
                    new_mask.as_mut_ptr() as *mut u8,
                    copy_len,
                )
            };
        }
        Some(proc::swap_signal_mask(new_mask))
    } else {
        None
    };

    // Ensure the mask is restored on every return path.
    let result = ppoll_inner(fds, current_pid, tmo_p, deadline_ns);

    if let Some(mask) = saved_mask {
        proc::restore_signal_mask(mask);
    }
    result
}

fn ppoll_inner(
    fds: &mut [PollFd],
    current_pid: u16,
    tmo_p: usize,
    deadline_ns: Option<u64>,
) -> i64 {
    // Initial readiness scan.
    let ready = match poll_fd_set_for_pid(fds, current_pid) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if ready > 0 {
        return ready as i64;
    }
    // A zero timespec means "return immediately" (already scanned: nothing ready).
    if tmo_p != 0 {
        let ts_ptr = tmo_p as *const twilight_common::syscall::types::Timespec;
        let ts = unsafe { &*ts_ptr };
        if ts.tv_sec == 0 && ts.tv_nsec == 0 {
            return 0;
        }
    }

    match wait_for_ready(fds, current_pid, deadline_ns) {
        Ok(n) => n,
        Err(e) => e,
    }
}

/// `select(nfds, readfds, writefds, exceptfds, timeout)` — select on three
/// fd sets.
///
/// `timeout == NULL` means infinite wait. A zero timeval returns immediately.
/// On timeout, all three output fd sets are cleared. Twilight does not update
/// the timeout timeval on return (the raw-syscall ABI is preserved).
pub fn select(
    nfds: i32,
    readfds_ptr: usize,
    writefds_ptr: usize,
    exceptfds_ptr: usize,
    timeout_ptr: usize,
) -> i64 {
    if nfds < 0 || nfds > FD_SETSIZE as i32 {
        return -(EINVAL as i64);
    }

    let n = nfds as usize;
    let current_pid = sys::proc::id();

    let mut pfd_array: [PollFd; FD_SETSIZE] = unsafe { core::mem::zeroed() };
    let mut pfd_count: usize = 0;
    let mut readfds_local: FdSet = FdSet::default();
    let mut writefds_local: FdSet = FdSet::default();
    let mut exceptfds_local: FdSet = FdSet::default();

    if readfds_ptr != 0 {
        readfds_local = unsafe { *(readfds_ptr as *const FdSet) };
    }
    if writefds_ptr != 0 {
        writefds_local = unsafe { *(writefds_ptr as *const FdSet) };
    }
    if exceptfds_ptr != 0 {
        exceptfds_local = unsafe { *(exceptfds_ptr as *const FdSet) };
    }

    for fd in 0..n {
        let fd_i32 = fd as i32;
        let mut events: i16 = 0;

        if readfds_local.isset(fd) {
            events |= POLLIN;
        }
        if writefds_local.isset(fd) {
            events |= POLLOUT;
        }
        if exceptfds_local.isset(fd) {
            events |= POLLPRI;
        }

        if events != 0 {
            pfd_array[pfd_count] = PollFd {
                fd: fd_i32,
                events,
                revents: 0,
            };
            pfd_count += 1;
        }
    }

    let timeout_is_null = timeout_ptr == 0;
    let deadline_ns = if timeout_is_null {
        None
    } else {
        let tv = unsafe { &*(timeout_ptr as *const Timeval) };
        if tv.tv_sec == 0 && tv.tv_usec == 0 {
            // Zero timeout: scan once, write back, return.
            let pfds = &mut pfd_array[..pfd_count];
            let ready = match poll_fd_set_for_pid(pfds, current_pid) {
                Ok(n) => n,
                Err(e) => return e,
            };
            write_back_select_results(
                pfds,
                readfds_ptr,
                writefds_ptr,
                exceptfds_ptr,
                &mut readfds_local,
                &mut writefds_local,
                &mut exceptfds_local,
                n,
            );
            return ready as i64;
        }
        if !valid_timeval(*tv) {
            return -(EINVAL as i64);
        }
        let now_ns = crate::driver::time::monotonic_ns();
        let delta_ns = timeval_to_ns(*tv);
        Some(now_ns.saturating_add(delta_ns))
    };

    let pfds = &mut pfd_array[..pfd_count];

    // Initial readiness scan.
    let ready = match poll_fd_set_for_pid(pfds, current_pid) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if ready > 0 {
        write_back_select_results(
            pfds,
            readfds_ptr,
            writefds_ptr,
            exceptfds_ptr,
            &mut readfds_local,
            &mut writefds_local,
            &mut exceptfds_local,
            n,
        );
        return ready as i64;
    }

    // nfds == 0 (no watched fds): still obey timeout/signal semantics (#69).
    // wait_for_ready handles the empty-scan case correctly — it blocks until
    // the deadline or a signal, returning 0 on timeout.
    match wait_for_ready(pfds, current_pid, deadline_ns) {
        Ok(0) => {
            // Timeout: clear the returned fd sets.
            readfds_local.zero();
            writefds_local.zero();
            exceptfds_local.zero();
            write_back_select_results(
                pfds,
                readfds_ptr,
                writefds_ptr,
                exceptfds_ptr,
                &mut readfds_local,
                &mut writefds_local,
                &mut exceptfds_local,
                n,
            );
            0
        }
        Ok(count) => {
            write_back_select_results(
                pfds,
                readfds_ptr,
                writefds_ptr,
                exceptfds_ptr,
                &mut readfds_local,
                &mut writefds_local,
                &mut exceptfds_local,
                n,
            );
            count
        }
        Err(e) => e,
    }
}

fn valid_timeval(value: Timeval) -> bool {
    value.tv_sec >= 0 && (0..1_000_000).contains(&value.tv_usec)
}

fn write_back_select_results(
    pfds: &[PollFd],
    readfds_ptr: usize,
    writefds_ptr: usize,
    exceptfds_ptr: usize,
    readfds: &mut FdSet,
    writefds: &mut FdSet,
    exceptfds: &mut FdSet,
    nfds: usize,
) {
    for fd in 0..nfds {
        readfds.clr(fd);
        writefds.clr(fd);
        exceptfds.clr(fd);
    }

    for pfd in pfds {
        let fd = pfd.fd as usize;
        if fd >= nfds {
            continue;
        }
        if (pfd.revents & (POLLIN | POLLHUP | POLLERR)) != 0 {
            readfds.set(fd);
        }
        if (pfd.revents & POLLOUT) != 0 {
            writefds.set(fd);
        }
        if (pfd.revents & (POLLERR | POLLNVAL)) != 0 {
            exceptfds.set(fd);
        }
    }

    if readfds_ptr != 0 {
        unsafe {
            *(readfds_ptr as *mut FdSet) = *readfds;
        }
    }
    if writefds_ptr != 0 {
        unsafe {
            *(writefds_ptr as *mut FdSet) = *writefds;
        }
    }
    if exceptfds_ptr != 0 {
        unsafe {
            *(exceptfds_ptr as *mut FdSet) = *exceptfds;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_timeout_infinite_is_none() {
        assert_eq!(poll_timeout_to_deadline(-1, 1_000_000), None);
    }

    #[test]
    fn poll_timeout_zero_is_now() {
        assert_eq!(poll_timeout_to_deadline(0, 1_000_000), Some(1_000_000));
    }

    #[test]
    fn poll_timeout_finite_adds_delta() {
        // 5 ms -> 5_000_000 ns
        assert_eq!(
            poll_timeout_to_deadline(5, 1_000_000_000),
            Some(1_005_000_000)
        );
    }

    #[test]
    fn poll_timeout_saturates_on_overflow() {
        // isize::MAX ms must not wrap into the past.
        let d = poll_timeout_to_deadline(isize::MAX, u64::MAX - 1);
        assert!(d.is_some());
        assert_eq!(d, Some(u64::MAX));
    }

    #[test]
    fn timeval_to_ns_basic() {
        let tv = Timeval { tv_sec: 2, tv_usec: 500_000 };
        assert_eq!(timeval_to_ns(tv), 2_500_000_000);
    }

    #[test]
    fn timeval_to_ns_zero() {
        let tv = Timeval { tv_sec: 0, tv_usec: 0 };
        assert_eq!(timeval_to_ns(tv), 0);
    }

    #[test]
    fn valid_timeval_accepts_zero() {
        assert!(valid_timeval(Timeval { tv_sec: 0, tv_usec: 0 }));
    }

    #[test]
    fn valid_timeval_rejects_negative_sec() {
        assert!(!valid_timeval(Timeval { tv_sec: -1, tv_usec: 0 }));
    }

    #[test]
    fn valid_timeval_rejects_negative_usec() {
        assert!(!valid_timeval(Timeval { tv_sec: 1, tv_usec: -1 }));
    }

    #[test]
    fn valid_timeval_rejects_overflowing_usec() {
        assert!(!valid_timeval(Timeval { tv_sec: 1, tv_usec: 1_000_000 }));
    }

    #[test]
    fn valid_timeval_accepts_max_usec() {
        assert!(valid_timeval(Timeval { tv_sec: 1, tv_usec: 999_999 }));
    }
}
