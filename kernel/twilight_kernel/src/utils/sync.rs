use alloc::vec::Vec;
use core::mem::ManuallyDrop;
use x86_64::instructions::interrupts;

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
        MutexGuard {
            guard: ManuallyDrop::new(self.inner.lock()),
            irq_lock: false,
            _preempt_guard: preempt_guard,
        }
    }

    pub fn lock_irq(&self) -> MutexGuard<T> {
        let irq_lock = interrupts::are_enabled();

        interrupts::disable();
        let preempt_guard = PreemptGuard::new_no_resched();

        MutexGuard {
            guard: ManuallyDrop::new(self.inner.lock()),
            irq_lock,
            _preempt_guard: preempt_guard,
        }
    }

    pub fn force_unlock(&self) {
        unsafe { self.inner.force_unlock() }
    }
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    guard: ManuallyDrop<spin::MutexGuard<'a, T>>,
    irq_lock: bool,
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
        unsafe {
            ManuallyDrop::drop(&mut self.guard);
        }

        if self.irq_lock {
            interrupts::enable();
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
        RwLockReadGuard {
            guard: ManuallyDrop::new(self.inner.read()),
            _preempt_guard: preempt_guard,
        }
    }

    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        let preempt_guard = PreemptGuard::new_no_resched();
        RwLockWriteGuard {
            guard: ManuallyDrop::new(self.inner.write()),
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
    }
}

pub struct IrqGuard {
    locked: bool,
}

impl IrqGuard {
    /// Creates a new IRQ guard. See the [`IrqGuard`] documentation for more.
    pub fn new() -> Self {
        let locked = interrupts::are_enabled();

        interrupts::disable();

        Self { locked }
    }
}

impl Drop for IrqGuard {
    /// Drops the IRQ guard, enabling interrupts again. See the [`IrqGuard`]
    /// documentation for more.
    fn drop(&mut self) {
        if self.locked {
            interrupts::enable();
        }
    }
}

pub struct WaitQueue {
    waiters: Mutex<Vec<u16>>,
}

impl WaitQueue {
    pub const fn new() -> Self {
        Self {
            waiters: Mutex::new(Vec::new()),
        }
    }

    pub fn prepare_current(&self) -> u16 {
        let pid = crate::sys::proc::id();
        let mut waiters = self.waiters.lock_irq();

        if !waiters.contains(&pid) {
            waiters.push(pid);
        }

        pid
    }

    pub fn finish_wait(&self, pid: u16) {
        let mut waiters = self.waiters.lock_irq();

        if let Some(idx) = waiters.iter().position(|&waiter| waiter == pid) {
            waiters.remove(idx);
        }
    }

    pub fn notify_one(&self) {
        let waiter = {
            let mut waiters = self.waiters.lock_irq();
            if waiters.is_empty() {
                None
            } else {
                Some(waiters.remove(0))
            }
        };

        if let Some(pid) = waiter {
            crate::sys::proc::wake_process(pid);
        }
    }

    pub fn notify_all(&self) {
        loop {
            let waiter = {
                let mut waiters = self.waiters.lock_irq();
                if waiters.is_empty() {
                    None
                } else {
                    Some(waiters.remove(0))
                }
            };

            let Some(pid) = waiter else {
                break;
            };

            crate::sys::proc::wake_process(pid);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.waiters.lock_irq().is_empty()
    }
}

impl Default for WaitQueue {
    fn default() -> Self {
        Self::new()
    }
}
