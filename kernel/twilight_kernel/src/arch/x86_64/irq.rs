//! Common external-interrupt accounting (#70).
//!
//! Every external IRQ vector must bracket its handler body with [`IrqCtx`] so
//! that `IRQ_DEPTH` reflects *all* in-progress interrupts, not only the timer.
//! This is load-bearing for two reasons:
//!
//! 1. `can_preempt_kernel()` and `warn_if_schedule_unsafe()` consult
//!    `irq_depth()` to refuse scheduling inside an interrupt. Without
//!    accounting here, a device IRQ that wakes a process could escape through a
//!    non-timer path with `irq_depth == 0`, hiding a forbidden-schedule bug.
//! 2. The follow-up syscall-IRQ ticket needs proof that no external IRQ exits
//!    through a scheduling path; uniform depth tracking is that evidence.
//!
//! `IrqCtx` is allocation-free (a single atomic increment/decrement) and may be
//! held across the entire handler body including any wake calls.

use crate::sys::preempt;

/// RAII guard that increments `IRQ_DEPTH` on construction and decrements it on
/// drop. Wrap every external interrupt handler body in `let _ctx = IrqCtx::new();`.
pub struct IrqCtx {
    _priv: (),
}

impl IrqCtx {
    #[inline]
    pub fn new() -> Self {
        preempt::irq_enter();
        Self { _priv: () }
    }
}

impl Drop for IrqCtx {
    #[inline]
    fn drop(&mut self) {
        preempt::irq_exit();
    }
}
