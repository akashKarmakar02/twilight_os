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