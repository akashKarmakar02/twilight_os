//! Process-timer API: token-safe absolute-deadline blocking and wakeups (#66).
//!
//! This module owns the deadline queue and the atomic blocked/runnable
//! transition. Hardware timekeeping (`driver::time`) reads the clocksource and
//! reports clock events; it does **not** own process state. The queue lives here,
//! under `sys`, and is driven by [`crate::driver::time::handle_timer_event`]
//! calling [`expire_due`].
//!
//! ## Synchronization contract
//!
//! The timer queue is a [`Mutex<TimerQueue>`] acquired with [`Mutex::lock_irq`],
//! which disables interrupts and raises the preempt count. The documented lock
//! order is **timer-queue → process-table**: the queue guard is always dropped
//! before any `wake_process`/scheduler call, so no lock survives a context
//! switch and the process table is touched only through the existing
//! `PROCESS_TABLE.get_mut()` path under a [`SchedulerGuard`].
//!
//! ## Hard-IRQ budget
//!
//! [`expire_due`] runs in hard-IRQ context (called from the timer ISR). It pops
//! a bounded batch of due entries into a stack array — no allocation — wakes
//! them, and sets a deferred-overflow flag if more entries remain due. The
//! remainder is drained by [`process_deferred_expiry`] from
//! [`crate::sys::proc::timer_preempt_common`] after `irq_exit()`, i.e. outside
//! hard-IRQ context.
//!
//! All state and safety guarantees are BSP-only until per-CPU scheduler state is
//! redesigned.

pub mod queue;

pub use queue::{DeadlineEntry, DeadlineKind, TimerQueue, WakeReason, WaitToken};

use crate::sys::preempt::SchedulerGuard;
use crate::utils::sync::Mutex;

/// Maximum number of entries expired per hard-IRQ batch. Bounded so the ISR
/// performs a fixed amount of work; overflow is drained post-`irq_exit` by
/// [`process_deferred_expiry`].
const EXPIRY_BATCH: usize = 32;

/// The single deadline queue. Capacity is reserved per-push in task context
/// before the IRQ-disabled transaction, so insertion in the critical section
/// never allocates.
static TIMER_QUEUE: Mutex<TimerQueue> = Mutex::new(TimerQueue::new());

/// Set by [`expire_due`] when more entries are due than the hard-IRQ batch could
/// drain. Cleared by [`process_deferred_expiry`] once the head is no longer due.
static DEFERRED_EXPIRY_PENDING: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Block the current task until the absolute monotonic deadline `deadline_ns`.
///
/// Returns the reason execution resumed. The transition is atomic with respect
/// to timer and signal wakeups: the wait token, deadline metadata, and
/// `Running -> Sleeping` state change are all published under one queue
/// `lock_irq` guard.
///
/// See the module docs for the synchronization contract and BSP-only scope.
pub fn block_current_until(deadline_ns: u64) -> WakeReason {
    let cur_pid = crate::sys::proc::id();

    // (1) Scheduler ownership. If we cannot enter, we cannot block safely.
    let Some(scheduler_guard) = SchedulerGuard::try_enter() else {
        return WakeReason::Cancelled;
    };

    // (2) Already due? Return without blocking.
    let now = crate::driver::time::monotonic_ns();
    if deadline_ns <= now {
        return WakeReason::Deadline;
    }

    // (3) Pending interrupting signal wins over the sleep.
    #[allow(static_mut_refs)]
    let table = unsafe { crate::sys::proc::PROCESS_TABLE.get_mut() };
    let Some(table) = table else {
        return WakeReason::Cancelled;
    };
    {
        let has_signal = table
            .proc_list
            .iter()
            .find(|p| p.pid == cur_pid)
            .is_some_and(|p| p.has_unblocked_signal());
        if has_signal {
            return WakeReason::Signal;
        }
    }

    // (4)+(5) Atomic publication: mint the token, reserve+push the queue entry,
    // publish the wait metadata on the process, and flip to Sleeping — all under
    // one queue `lock_irq` guard so no timer/signal wakeup can observe a
    // half-published state. Iteration over the VecDeque is allocation-free (no
    // make_contiguous), satisfying the no-allocation-under-IRQ-disabled rule.
    let token;
    {
        let mut q = TIMER_QUEUE.lock_irq();
        token = q.next_token();
        if token == WaitToken::EXHAUSTED {
            return WakeReason::Cancelled;
        }
        let entry = DeadlineEntry {
            deadline_ns,
            pid: cur_pid,
            token,
            kind: DeadlineKind::Sleep,
        };
        if q.try_reserve_push(&entry).is_err() {
            return WakeReason::Cancelled;
        }

        // Publish the wait and flip to Sleeping while still holding the queue
        // guard. A concurrent expire_due() that pops this entry will see the
        // matching wait_token once we release; a pop before this point sees the
        // process still Running (wait_token None) and treats the entry as stale,
        // which is correct because we have not committed to blocking yet.
        #[allow(static_mut_refs)]
        let Some(table) = (unsafe { crate::sys::proc::PROCESS_TABLE.get_mut() }) else {
            // Unreachable: table existed above. Leave the stale entry to be
            // reclaimed lazily; do not block.
            return WakeReason::Cancelled;
        };
        let Some(cur) = table.proc_list.iter_mut().find(|p| p.pid == cur_pid) else {
            return WakeReason::Cancelled;
        };
        cur.wait_token = Some(token);
        cur.wait_deadline_ns = Some(deadline_ns);
        cur.wake_reason = None;
        cur.state = crate::sys::proc::ProcessState::Sleeping;
    }

    // (6) Release the scheduler guard before switching. The next task may enter
    //     userspace without returning through this Rust stack.
    drop(scheduler_guard);

    // (7) Switch to another Runnable task. Since current is now Sleeping,
    //     find_next_runnable_index skips it.
    let switched = crate::sys::proc::schedule_now();

    // (8) No-alternate-runnable path: remain Sleeping and idle until woken.
    if !switched {
        idle_until_woken(cur_pid, token);
    }

    // Resumed. Read the wake reason published by the wake path (or infer
    // Deadline if the deadline passed without an explicit reason).
    read_and_clear_wake_reason(cur_pid, deadline_ns)
}

/// Idle loop for the no-alternate-runnable path: the current task is the only
/// runnable candidate but is logically Sleeping, so halt until an interrupt
/// (timer expiry or I/O) wakes us, then recheck the wait state.
fn idle_until_woken(cur_pid: u16, _token: WaitToken) {
    loop {
        // enable_and_hlt: interrupts fire (timer expiry), then we resume with
        // IRQs disabled by the ISR return path.
        crate::task::executor::halt();

        // After wake, disable IRQs (halt() leaves them as they were) and recheck.
        let _irq_guard = crate::utils::sync::IrqGuard::new();

        #[allow(static_mut_refs)]
        let Some(table) = (unsafe { crate::sys::proc::PROCESS_TABLE.get_mut() }) else {
            return;
        };
        let slice = table.proc_list.make_contiguous();
        let Some(cur_idx) = slice.iter().position(|p| p.pid == cur_pid) else {
            return;
        };
        let cur = &slice[cur_idx];

        // A wake set wake_reason, or the deadline elapsed.
        if cur.wake_reason.is_some() {
            return;
        }
        if crate::driver::time::monotonic_ns() >= cur.wait_deadline_ns.unwrap_or(0) {
            // Deadline elapsed without an explicit wake: self-wake.
            return;
        }
        // Still sleeping and not due: halt again.
    }
}

/// Read the published wake reason, clearing the wait metadata. If no explicit
/// reason was set but the deadline elapsed, infer `Deadline`.
fn read_and_clear_wake_reason(cur_pid: u16, deadline_ns: u64) -> WakeReason {
    let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
    #[allow(static_mut_refs)]
    let Some(table) = (unsafe { crate::sys::proc::PROCESS_TABLE.get_mut() }) else {
        return WakeReason::Deadline;
    };
    let slice = table.proc_list.make_contiguous();
    let Some(cur_idx) = slice.iter().position(|p| p.pid == cur_pid) else {
        return WakeReason::Deadline;
    };
    let cur = &mut slice[cur_idx];
    let reason = cur.wake_reason.unwrap_or_else(|| {
        if crate::driver::time::monotonic_ns() >= deadline_ns {
            WakeReason::Deadline
        } else {
            WakeReason::Cancelled
        }
    });
    cur.wait_token = None;
    cur.wait_deadline_ns = None;
    cur.wake_reason = None;
    // Restore Running: this task is the physical BSP current task again.
    cur.state = crate::sys::proc::ProcessState::Running;
    reason
}

/// Arm a wait for the current process and return its token. Called by
/// `block_current_until` and available to other subsystems (e.g. I/O timeouts)
/// that publish their own blocked state under the same contract.
///
/// The caller must hold the scheduler guard and have already validated the
/// deadline against the clock. Returns [`WaitToken::EXHAUSTED`] if the token
/// space is exhausted (do not block in that case).
#[allow(dead_code)]
pub(crate) fn arm_current_locked(deadline_ns: u64, kind: DeadlineKind) -> WaitToken {
    let cur_pid = crate::sys::proc::id();
    let mut q = TIMER_QUEUE.lock_irq();
    let token = q.next_token();
    if token == WaitToken::EXHAUSTED {
        return token;
    }
    let entry = DeadlineEntry {
        deadline_ns,
        pid: cur_pid,
        token,
        kind,
    };
    if q.try_reserve_push(&entry).is_err() {
        return WaitToken::EXHAUSTED;
    }
    token
}

/// Cancel an outstanding wait owned by `token`. Clears the process's published
/// token so the lazy stale-entry reclamation in the queue treats the entry as
/// dead. Safe to call with a token that is no longer live (no-op).
///
/// This is the explicit cancellation primitive for paths that abort a wait
/// without exiting (e.g. a POSIX signal interrupting a sleep). The exit path
/// uses the lighter `proc::invalidate_wait_token`, which clears the same field
/// without taking the queue lock; both rely on lazy reclamation.
#[allow(dead_code)]
pub(crate) fn cancel_owned(token: WaitToken) {
    let cur_pid = crate::sys::proc::id();
    {
        let _preempt_guard = crate::sys::preempt::PreemptGuard::new();
        #[allow(static_mut_refs)]
        let Some(table) = (unsafe { crate::sys::proc::PROCESS_TABLE.get_mut() }) else {
            return;
        };
        let slice = table.proc_list.make_contiguous();
        if let Some(cur_idx) = slice.iter().position(|p| p.pid == cur_pid) {
            let cur = &mut slice[cur_idx];
            if cur.wait_token == Some(token) {
                cur.wait_token = None;
                cur.wait_deadline_ns = None;
            }
        }
    }
    // The queue entry is reclaimed lazily when it reaches the head; clearing
    // the process's published token is enough to make it stale.
}

/// Expire all due waits. Called from the timer ISR (`handle_timer_event`) in
/// hard-IRQ context. Pops a bounded batch into a stack array (no allocation),
/// wakes matching sleepers, and defers overflow to [`process_deferred_expiry`].
pub fn expire_due(now_ns: u64) {
    // Collect a bounded batch of due entries without holding the queue while
    // waking processes.
    let mut batch: [Option<(u16, WaitToken)>; EXPIRY_BATCH] = [const { None }; EXPIRY_BATCH];
    let mut count = 0;
    let more_due;
    {
        let mut q = TIMER_QUEUE.lock_irq();
        while count < EXPIRY_BATCH {
            match q.pop_due(now_ns, is_live_wait) {
                Some(e) => {
                    batch[count] = Some((e.pid, e.token));
                    count += 1;
                }
                None => break,
            }
        }
        // If the head is still due, more entries remain than the batch held.
        more_due = q
            .peek_deadline_ns(is_live_wait)
            .is_some_and(|d| d <= now_ns);
    }
    // Queue guard dropped: wake processes without holding it.
    for i in 0..count {
        if let Some((pid, token)) = batch[i] {
            crate::sys::proc::wake_from_timer(pid, token);
        }
    }
    if more_due {
        DEFERRED_EXPIRY_PENDING.store(true, core::sync::atomic::Ordering::SeqCst);
    }
}

/// Drain deadline overflow outside hard-IRQ context. Called from
/// `timer_preempt_common` after `irq_exit()`. Repeatedly expires batches until
/// no due entries remain, then clears the deferred flag.
pub fn process_deferred_expiry() {
    if !DEFERRED_EXPIRY_PENDING.load(core::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let now_ns = crate::driver::time::monotonic_ns();
    let mut batch: [Option<(u16, WaitToken)>; EXPIRY_BATCH] = [const { None }; EXPIRY_BATCH];
    loop {
        let mut count = 0;
        let more_due;
        {
            let mut q = TIMER_QUEUE.lock_irq();
            while count < EXPIRY_BATCH {
                match q.pop_due(now_ns, is_live_wait) {
                    Some(e) => {
                        batch[count] = Some((e.pid, e.token));
                        count += 1;
                    }
                    None => break,
                }
            }
            more_due = q
                .peek_deadline_ns(is_live_wait)
                .is_some_and(|d| d <= now_ns);
        }
        for i in 0..count {
            if let Some((pid, token)) = batch[i] {
                crate::sys::proc::wake_from_timer(pid, token);
            }
        }
        if !more_due {
            break;
        }
    }
    DEFERRED_EXPIRY_PENDING.store(false, core::sync::atomic::Ordering::SeqCst);
}

/// Earliest live deadline, or `None` if no live waits exist. Stale heads are
/// drained first so a cancelled entry cannot mask a later deadline. Intended for
/// future one-shot timer programming; exposed here per the issue API.
pub fn next_deadline_ns() -> Option<u64> {
    let mut q = TIMER_QUEUE.lock_irq();
    q.peek_deadline_ns(is_live_wait)
}

/// Liveness predicate: a `(pid, token)` identifies a live wait iff the process
/// currently publishes that token. Runs under the queue lock; touches the
/// process table read-only without allocating (no `make_contiguous`) so it is
/// safe in hard-IRQ context.
fn is_live_wait(pid: u16, token: WaitToken) -> bool {
    #[allow(static_mut_refs)]
    let Some(table) = (unsafe { crate::sys::proc::PROCESS_TABLE.get_mut() }) else {
        return false;
    };
    // Iterate the VecDeque directly; do NOT call make_contiguous (it may
    // allocate, which is forbidden in hard-IRQ expiry context).
    table
        .proc_list
        .iter()
        .find(|p| p.pid == pid)
        .is_some_and(|p| p.wait_token == Some(token))
}
