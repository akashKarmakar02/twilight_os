use alloc::vec::Vec;
use x86_64::instructions::interrupts;

pub struct Mutex<T: ?Sized> {
    inner: spin::Mutex<T>,
}

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            inner: spin::Mutex::new(value),
        }
    }

    pub fn lock(&self) -> MutexGuard<T> {
        MutexGuard {
            guard: core::mem::ManuallyDrop::new(self.inner.lock()),
            irq_lock: false,
        }
    }

    pub fn lock_irq(&self) -> MutexGuard<T> {
        let irq_lock = interrupts::are_enabled();

        interrupts::disable();

        MutexGuard {
            guard: core::mem::ManuallyDrop::new(self.inner.lock()),
            irq_lock,
        }
    }

    pub fn force_unlock(&self) {
        unsafe { self.inner.force_unlock() }
    }
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    guard: core::mem::ManuallyDrop<spin::MutexGuard<'a, T>>,
    irq_lock: bool,
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
            core::mem::ManuallyDrop::drop(&mut self.guard);
        }

        if self.irq_lock {
            interrupts::enable();
        }
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
