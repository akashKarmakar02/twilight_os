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

// --- Kernel-preemption context trackers -----------------------------------
//
// These counters are only incremented when `ENABLE_KERNEL_PREEMPTION` is true
// (a compile-time const). When the flag is false the compiler eliminates every
// increment/decrement, so existing behavior is preserved with zero overhead.
//
// Each counter is a depth (not a boolean) so that nested entry is balanced.
// `can_preempt_kernel()` requires every counter to be zero before scheduling
// from a kernel-mode timer interrupt.

/// Number of kernel locks (Mutex/RwLock) currently held on this CPU.
static HELD_LOCK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Non-zero while inside a fault/exception handler (page fault, double fault).
static FAULT_CONTEXT: AtomicUsize = AtomicUsize::new(0);

/// Non-zero while inside the heap allocator (alloc/dealloc).
static ALLOCATOR_CONTEXT: AtomicUsize = AtomicUsize::new(0);

/// Non-zero while mutating the process table or per-process fd/signal state.
static PROCESS_TABLE_CONTEXT: AtomicUsize = AtomicUsize::new(0);

/// Non-zero while inside a VFS operation critical section.
static VFS_CONTEXT: AtomicUsize = AtomicUsize::new(0);

/// Non-zero while mapping/unmapping/updating page-table entries.
static PAGETABLE_CONTEXT: AtomicUsize = AtomicUsize::new(0);

// Enable temporarily when diagnosing deferred scheduling. Keeping this false
// avoids printing on every timer tick in normal builds.
const PREEMPT_DEBUG: bool = false;

/// Experimental kernel-mode timer preemption. **Disabled by default.**
///
/// When `false` (the default), timer interrupts schedule only when returning
/// to userspace (`from_user != 0`). Kernel-mode timer ticks set `need_resched`
/// and return; the reschedule is honored later at an explicit `cond_resched()`
/// safe point. This preserves the existing, known-safe behavior exactly.
///
/// When `true`, the timer path may additionally call `schedule_now()` from
/// kernel mode, but only if [`can_preempt_kernel()`] confirms that every safety
/// condition is satisfied. This is experimental and may destabilize the kernel.
pub const ENABLE_KERNEL_PREEMPTION: bool = false;

/// Controls per-tick skip/allow logging for the kernel preemption path.
/// Keeping this false avoids flooding the serial console on every kernel timer
/// tick. Enable when diagnosing kernel preemption behavior.
const KPREEMPT_DEBUG: bool = false;

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

// --- Lock counting --------------------------------------------------------

#[inline]
pub fn lock_count_inc() {
    if ENABLE_KERNEL_PREEMPTION {
        HELD_LOCK_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}

#[inline]
pub fn lock_count_dec() {
    if ENABLE_KERNEL_PREEMPTION {
        decrement(&HELD_LOCK_COUNT, "held_lock_count");
    }
}

#[inline]
pub fn held_lock_count() -> usize {
    HELD_LOCK_COUNT.load(Ordering::SeqCst)
}

#[inline]
pub fn locks_held() -> bool {
    held_lock_count() != 0
}

// --- Context guards -------------------------------------------------------

/// RAII guard that increments a context counter on creation and decrements it
/// on drop. When `ENABLE_KERNEL_PREEMPTION` is false the increment/decrement
/// are compile-time eliminated, so the guard is zero-cost.
#[must_use = "dropping the guard exits the context"]
pub struct ContextGuard {
    counter: &'static AtomicUsize,
    name: &'static str,
    active: bool,
}

impl ContextGuard {
    fn enter(counter: &'static AtomicUsize, name: &'static str) -> Self {
        if ENABLE_KERNEL_PREEMPTION {
            counter.fetch_add(1, Ordering::SeqCst);
        }
        Self {
            counter,
            name,
            active: true,
        }
    }
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        if self.active {
            self.active = false;
            if ENABLE_KERNEL_PREEMPTION {
                decrement(self.counter, self.name);
            }
        }
    }
}

pub fn enter_fault_context() -> ContextGuard {
    ContextGuard::enter(&FAULT_CONTEXT, "fault_context")
}

pub fn enter_allocator_context() -> ContextGuard {
    ContextGuard::enter(&ALLOCATOR_CONTEXT, "allocator_context")
}

pub fn enter_process_table_context() -> ContextGuard {
    ContextGuard::enter(&PROCESS_TABLE_CONTEXT, "process_table_context")
}

pub fn enter_vfs_context() -> ContextGuard {
    ContextGuard::enter(&VFS_CONTEXT, "vfs_context")
}

pub fn enter_pagetable_context() -> ContextGuard {
    ContextGuard::enter(&PAGETABLE_CONTEXT, "pagetable_context")
}

#[inline]
pub fn in_fault_context() -> bool {
    FAULT_CONTEXT.load(Ordering::SeqCst) != 0
}

#[inline]
pub fn in_allocator_context() -> bool {
    ALLOCATOR_CONTEXT.load(Ordering::SeqCst) != 0
}

#[inline]
pub fn in_process_table_context() -> bool {
    PROCESS_TABLE_CONTEXT.load(Ordering::SeqCst) != 0
}

#[inline]
pub fn in_vfs_context() -> bool {
    VFS_CONTEXT.load(Ordering::SeqCst) != 0
}

#[inline]
pub fn in_pagetable_context() -> bool {
    PAGETABLE_CONTEXT.load(Ordering::SeqCst) != 0
}

// --- Kernel preemption predicate -----------------------------------------

/// Returns `true` only if kernel-mode timer preemption is safe right now.
///
/// Every condition is checked in order; the first failure is logged (when
/// `KPREEMPT_DEBUG` is enabled) and the function returns `false`. When
/// uncertain the function returns `false` — it is always safer to skip
/// kernel preemption than to crash.
///
/// # Safety conditions
///
/// * `ENABLE_KERNEL_PREEMPTION` is `true`
/// * `need_resched` is set
/// * `preempt_count == 0` (no preempt-disabled critical sections)
/// * `irq_depth == 0` (not nested inside another interrupt — `irq_exit` has
///   already been called before `timer_preempt_common` runs)
/// * not already inside the scheduler
/// * no kernel locks held
/// * not inside a fault handler
/// * not inside the allocator
/// * not inside process-table/fd/signal mutation
/// * not inside a VFS critical section
/// * not inside page-table map/unmap/update code
pub fn can_preempt_kernel() -> bool {
    if !ENABLE_KERNEL_PREEMPTION {
        return false;
    }
    if !need_resched() {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: need_resched=false");
        }
        return false;
    }
    if preempt_count() != 0 {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: preempt_count={}", preempt_count());
        }
        return false;
    }
    // irq_depth == 0 means we are not nested inside another interrupt.
    // (irq_exit() is called before timer_preempt_common, so the current
    // timer's own depth has already been decremented.)
    if irq_depth() != 0 {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: irq_depth={}", irq_depth());
        }
        return false;
    }
    if in_scheduler() {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: in_scheduler=true");
        }
        return false;
    }
    if locks_held() {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: locks_held={}", held_lock_count());
        }
        return false;
    }
    if in_fault_context() {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: in_fault_context=true");
        }
        return false;
    }
    if in_allocator_context() {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: in_allocator_context=true");
        }
        return false;
    }
    if in_process_table_context() {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: in_process_table_context=true");
        }
        return false;
    }
    if in_vfs_context() {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: in_vfs_context=true");
        }
        return false;
    }
    if in_pagetable_context() {
        if KPREEMPT_DEBUG {
            crate::serial_println!("[kpreempt] skip: in_pagetable_context=true");
        }
        return false;
    }
    true
}

/// Log serial warnings if `schedule_now()` is called from an unsafe context.
///
/// These are diagnostic only — they do not block scheduling. The hard safety
/// gate for kernel-mode preemption is [`can_preempt_kernel()`]. The
/// [`SchedulerGuard`] already prevents reentrant scheduling and scheduling
/// with `preempt_count > 0`. These warnings catch the remaining cases (locks
/// held via non-`PreemptGuard` spinlocks, or raw context counters non-zero)
/// that indicate a bug.
pub fn warn_if_schedule_unsafe() {
    if irq_depth() != 0 {
        crate::serial_println!(
            "[sched] WARNING: schedule_now with irq_depth={}",
            irq_depth()
        );
    }
    if locks_held() {
        crate::serial_println!(
            "[sched] WARNING: schedule_now while locks_held={}",
            held_lock_count()
        );
    }
    if in_fault_context() {
        crate::serial_println!("[sched] WARNING: schedule_now inside fault context");
    }
    if in_allocator_context() {
        crate::serial_println!("[sched] WARNING: schedule_now inside allocator context");
    }
    if in_process_table_context() {
        crate::serial_println!("[sched] WARNING: schedule_now inside process-table context");
    }
    if in_vfs_context() {
        crate::serial_println!("[sched] WARNING: schedule_now inside VFS context");
    }
    if in_pagetable_context() {
        crate::serial_println!("[sched] WARNING: schedule_now inside pagetable context");
    }
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
