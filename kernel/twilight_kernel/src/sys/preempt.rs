//! Phase 1 deferred-rescheduling and preemption accounting.
//!
//! This module does not make kernel code preemptible. Timer interrupts may
//! request rescheduling, but kernel task context only honors the request at an
//! explicit `cond_resched()` safe point. The existing user-mode timer return
//! path remains the only interrupt-context scheduling exception.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// Bootstrap implementation: userspace scheduling currently runs only on the
// BSP, while APs remain in their halt loop. Twilight's existing CpuLocal<T>
// accessor uses GS, but scheduled processes currently repurpose kernel GS for
// KernelGsData, so that accessor cannot safely back runtime scheduler state
// yet. Phase 2 must move these fields into genuinely per-CPU storage before
// enabling scheduling on APs.
static PREEMPT_COUNT: AtomicUsize = AtomicUsize::new(0);

static IRQ_DEPTH: AtomicUsize = AtomicUsize::new(0);

static IN_SCHEDULER: AtomicBool = AtomicBool::new(false);

static NEED_RESCHED: AtomicBool = AtomicBool::new(false);

// Enable temporarily when diagnosing deferred scheduling. Keeping this false
// avoids printing on every timer tick in normal builds.
const PREEMPT_DEBUG: bool = false;

#[inline]
pub fn preempt_disable() {
    PREEMPT_COUNT.fetch_add(1, Ordering::SeqCst);
}

fn decrement(counter: &AtomicUsize, name: &str) -> Option<usize> {
    loop {
        let current = counter.load(Ordering::SeqCst);
        if current == 0 {
            crate::serial_println!("[preempt] {} underflow prevented", name);
            return None;
        }

        if counter
            .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return Some(current - 1);
        }
    }
}

#[inline]
pub fn preempt_enable() {
    if decrement(&PREEMPT_COUNT, "preempt_count") == Some(0) {
        cond_resched();
    }
}

#[inline]
pub fn preempt_enable_no_resched() {
    let _ = decrement(&PREEMPT_COUNT, "preempt_count");
}

#[inline]
pub fn preempt_count() -> usize {
    PREEMPT_COUNT.load(Ordering::SeqCst)
}

/// RAII token for a critical section that must not be preempted.
#[must_use = "dropping the guard re-enables preemption"]
pub struct PreemptGuard {
    active: bool,
    resched_on_drop: bool,
}

impl PreemptGuard {
    #[inline]
    pub fn new() -> Self {
        preempt_disable();
        Self {
            active: true,
            resched_on_drop: true,
        }
    }

    /// Enter a critical section whose scope exit is not itself a scheduling
    /// point. Spinlock guards use this form so IRQ-side unlocks cannot invoke
    /// the scheduler; explicit task-context safe points call cond_resched().
    #[inline]
    pub fn new_no_resched() -> Self {
        preempt_disable();
        Self {
            active: true,
            resched_on_drop: false,
        }
    }

    #[inline]
    pub fn release(mut self) {
        if self.active {
            self.active = false;
            if self.resched_on_drop {
                preempt_enable();
            } else {
                preempt_enable_no_resched();
            }
        }
    }
}

impl Default for PreemptGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PreemptGuard {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            if self.resched_on_drop {
                preempt_enable();
            } else {
                preempt_enable_no_resched();
            }
        }
    }
}

#[inline]
pub fn irq_enter() {
    IRQ_DEPTH.fetch_add(1, Ordering::SeqCst);
}

#[inline]
pub fn irq_exit() {
    let _ = decrement(&IRQ_DEPTH, "irq_depth");
}

#[inline]
pub fn irq_depth() -> usize {
    IRQ_DEPTH.load(Ordering::SeqCst)
}

#[inline]
pub fn set_need_resched() {
    let was_set = NEED_RESCHED.swap(true, Ordering::SeqCst);
    if PREEMPT_DEBUG && !was_set {
        crate::serial_println!("[preempt] set_need_resched");
    }
}

#[inline]
pub fn clear_need_resched() {
    NEED_RESCHED.store(false, Ordering::SeqCst);
}

#[inline]
pub fn need_resched() -> bool {
    NEED_RESCHED.load(Ordering::SeqCst)
}

#[inline]
pub fn in_scheduler() -> bool {
    IN_SCHEDULER.load(Ordering::SeqCst)
}

#[inline]
pub fn can_resched_now() -> bool {
    need_resched() && preempt_count() == 0 && irq_depth() == 0 && !in_scheduler()
}

/// Honor a deferred reschedule request from a known-safe task-context point.
pub fn cond_resched() {
    if preempt_count() != 0 {
        crate::serial_println!(
            "[preempt] cond_resched blocked: preempt_count={} irq_depth={} in_scheduler={}",
            preempt_count(),
            irq_depth(),
            in_scheduler()
        );
        return;
    }

    if can_resched_now() {
        if PREEMPT_DEBUG {
            crate::serial_println!(
                "[preempt] cond_resched schedule pid={}",
                crate::sys::proc::id()
            );
        }
        crate::sys::proc::schedule_now();
    } else if PREEMPT_DEBUG && need_resched() {
        crate::serial_println!(
            "[preempt] skip: preempt_count={} irq_depth={} in_scheduler={}",
            preempt_count(),
            irq_depth(),
            in_scheduler()
        );
    }
}

/// Prevents scheduler selection and process-table switching from re-entering.
///
/// The guard must be released immediately before the low-level context switch:
/// a newly started task does not return through the previous task's Rust stack
/// and therefore cannot be relied upon to drop an armed guard.
pub(crate) struct SchedulerGuard {
    active: bool,
}

impl SchedulerGuard {
    pub(crate) fn try_enter() -> Option<Self> {
        if in_scheduler() {
            crate::serial_println!("[sched] nested schedule prevented");
            return None;
        }

        if preempt_count() != 0 {
            crate::serial_println!(
                "[sched] schedule while preempt disabled: pid={} preempt_count={}",
                crate::sys::proc::id(),
                preempt_count()
            );
            return None;
        }

        preempt_disable();
        if IN_SCHEDULER
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            preempt_enable_no_resched();
            crate::serial_println!("[sched] nested schedule prevented");
            return None;
        }

        Some(Self { active: true })
    }

    pub(crate) fn release_before_switch(mut self) {
        self.release();
    }

    fn release(&mut self) {
        if self.active {
            IN_SCHEDULER.store(false, Ordering::SeqCst);
            preempt_enable_no_resched();
            self.active = false;
        }
    }
}

impl Drop for SchedulerGuard {
    fn drop(&mut self) {
        self.release();
    }
}
