use core::mem::ManuallyDrop;

use crate::sys::preempt::PreemptGuard;

pub struct Mutex<T: ?Sized> {
    inner: spin::Mutex<T>,
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: spin::Mutex::new(value),
        }
    }

}

impl<T: ?Sized> Mutex<T> {
    pub fn lock(&self) -> MutexGuard<T> {
        let preempt_guard = PreemptGuard::new_no_resched();
        let guard = self.inner.lock();
        crate::sys::preempt::lock_count_inc();
        MutexGuard {
            guard: ManuallyDrop::new(guard),
            irq_was_enabled: false,
            owns_irq_disable: false,
            _preempt_guard: preempt_guard,
        }
    }

    pub fn lock_irq(&self) -> MutexGuard<T> {
        // Nest-safe IRQ disable (#70): record whether IF was on, then CLI and
        // push one level onto the per-CPU IRQ-disable depth stack. The guard's
        // drop pops the level and re-enables IRQs only when the stack is empty
        // and IF was originally on — so nested lock_irq()/IrqGuard cannot
        // prematurely re-enable interrupts.
        let was_enabled = crate::sys::preempt::irq_save();
        let preempt_guard = PreemptGuard::new_no_resched();
        let guard = self.inner.lock();
        crate::sys::preempt::lock_count_inc();

        MutexGuard {
            guard: ManuallyDrop::new(guard),
            irq_was_enabled: was_enabled,
            owns_irq_disable: true,
            _preempt_guard: preempt_guard,
        }
    }

    pub fn force_unlock(&self) {
        unsafe { self.inner.force_unlock() }
    }
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    guard: ManuallyDrop<spin::MutexGuard<'a, T>>,
    /// Whether IF was enabled when `lock_irq()` was called. Only meaningful
    /// when `owns_irq_disable` is true.
    irq_was_enabled: bool,
    /// True iff this guard was produced by `lock_irq()` and therefore owns one
    /// level on the IRQ-disable depth stack that must be popped on drop.
    /// `lock()` sets this false and never touches IRQ state.
    owns_irq_disable: bool,
    // Dropped after Drop::drop releases the spinlock and restores IRQ state.
    _preempt_guard: PreemptGuard,
}

impl<T: ?Sized> core::ops::Deref for MutexGuard<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> core::ops::DerefMut for MutexGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // Release the spinlock first, then pop the IRQ-disable depth stack (if
        // this was a lock_irq guard) so IRQs are not re-enabled while the lock
        // is still held.
        let owns_irq = self.owns_irq_disable;
        let was_enabled = self.irq_was_enabled;
        unsafe {
            ManuallyDrop::drop(&mut self.guard);
        }
        crate::sys::preempt::lock_count_dec();

        if owns_irq {
            crate::sys::preempt::irq_restore(was_enabled);
        }
    }
}

pub struct RwLock<T: ?Sized> {
    inner: spin::RwLock<T>,
}

impl<T> RwLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: spin::RwLock::new(value),
        }
    }

    /// Legacy mutable-static compatibility that still takes the write lock.
    pub fn get_mut(&mut self) -> RwLockWriteGuard<'_, T> {
        self.write()
    }
}

impl<T: ?Sized> RwLock<T> {
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        let preempt_guard = PreemptGuard::new_no_resched();
        let guard = self.inner.read();
        crate::sys::preempt::lock_count_inc();
        RwLockReadGuard {
            guard: ManuallyDrop::new(guard),
            _preempt_guard: preempt_guard,
        }
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        let preempt_guard = PreemptGuard::new_no_resched();
        let guard = self.inner.write();
        crate::sys::preempt::lock_count_inc();
        RwLockWriteGuard {
            guard: ManuallyDrop::new(guard),
            _preempt_guard: preempt_guard,
        }
    }
}

pub struct RwLockReadGuard<'a, T: ?Sized + 'a> {
    guard: ManuallyDrop<spin::RwLockReadGuard<'a, T>>,
    _preempt_guard: PreemptGuard,
}

impl<T: ?Sized> core::ops::Deref for RwLockReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> Drop for RwLockReadGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: guard is dropped exactly once here before preemption is
        // re-enabled by the following PreemptGuard field drop.
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        crate::sys::preempt::lock_count_dec();
    }
}

pub struct RwLockWriteGuard<'a, T: ?Sized + 'a> {
    guard: ManuallyDrop<spin::RwLockWriteGuard<'a, T>>,
    _preempt_guard: PreemptGuard,
}

impl<T: ?Sized> core::ops::Deref for RwLockWriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T: ?Sized> core::ops::DerefMut for RwLockWriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<T: ?Sized> Drop for RwLockWriteGuard<'_, T> {
    fn drop(&mut self) {
        // SAFETY: guard is dropped exactly once here before preemption is
        // re-enabled by the following PreemptGuard field drop.
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        crate::sys::preempt::lock_count_dec();
    }
}

/// RAII guard that disables interrupts for its lifetime and restores the
/// previous interrupt state on drop.
///
/// Nest-safe (#70): an `IrqGuard` constructed while another `IrqGuard` (or a
/// `Mutex::lock_irq()` guard) is already live will not re-enable interrupts on
/// drop; only the outermost guard (the one that observed IF enabled) restores
/// interrupts. This is backed by a per-CPU depth counter in
/// [`crate::sys::preempt`].
pub struct IrqGuard {
    was_enabled: bool,
}

impl IrqGuard {
    /// Disable interrupts and push one level onto the IRQ-disable depth stack.
    pub fn new() -> Self {
        let was_enabled = crate::sys::preempt::irq_save();
        Self { was_enabled }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        crate::sys::preempt::irq_restore(self.was_enabled);
    }
}

/// Maximum number of waiters a single [`WaitQueue`] can hold.
///
/// This is a fixed cap so the queue never heap-allocates — critical because
/// `notify_all` is invoked from IRQ context (keyboard ISR), where the global
/// allocator's internal spin lock is not IRQ-safe and could deadlock against a
/// task-context allocation in progress (#70). A wait queue's waiter count is
/// bounded by the number of live processes blocked on the same resource, which
/// in practice is far below this cap; overflow drops the wake (the caller will
/// retry or the waiter times out), never corrupts state.
const WAIT_QUEUE_CAPACITY: usize = 64;

/// A blocking-wait queue storing PIDs in a fixed-size slab.
///
/// All operations are O(n) in the waiter count (n ≤ [`WAIT_QUEUE_CAPACITY`]),
/// which is acceptable for the small waiter sets this serves. The slab is
/// allocation-free so it is safe to drive from IRQ context.
pub struct WaitQueue {
    waiters: Mutex<[Option<u16>; WAIT_QUEUE_CAPACITY]>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            waiters: Mutex::new([const { None }; WAIT_QUEUE_CAPACITY]),
        }
    }

    pub fn prepare_current(&self) -> u16 {
        let pid = crate::sys::proc::id();
        let mut waiters = self.waiters.lock_irq();

        // Already queued?
        if waiters.iter().any(|w| matches!(w, Some(p) if *p == pid)) {
            return pid;
        }
        for slot in waiters.iter_mut() {
            if slot.is_none() {
                *slot = Some(pid);
                return pid;
            }
        }
        // Slab full: leave unqueued. The caller still blocks via its deadline /
        // spurious-wake path; the worst case is a missed wake that the caller's
        // timeout or re-poll recovers from. Log so overflow is observable.
        crate::serial_println!("[WaitQueue] prepare_current: slab full ({}), pid={} not enqueued", WAIT_QUEUE_CAPACITY, pid);
        pid
    }

    pub fn finish_wait(&self, pid: u16) {
        let mut waiters = self.waiters.lock_irq();
        for slot in waiters.iter_mut() {
            if matches!(slot, Some(p) if *p == pid) {
                *slot = None;
                break;
            }
        }
    }

    pub fn notify_one(&self) {
        let waiter = {
            let mut waiters = self.waiters.lock_irq();
            // Take the first occupied slot.
            let taken = waiters.iter_mut().find_map(|slot| slot.take());
            taken
        };

        if let Some(pid) = waiter {
            crate::sys::proc::wake_process(pid);
        }
    }

    pub fn notify_all(&self) {
        loop {
            let waiter = {
                let mut waiters = self.waiters.lock_irq();
                let taken = waiters.iter_mut().find_map(|slot| slot.take());
                taken
            };

            let Some(pid) = waiter else {
                break;
            };

            crate::sys::proc::wake_process(pid);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.waiters.lock_irq().iter().all(|w| w.is_none())
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
