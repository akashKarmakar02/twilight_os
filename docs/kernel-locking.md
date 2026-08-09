# Kernel locking

Twilight's kernel is single-CPU and **non-preemptive** (`ENABLE_KERNEL_PREEMPTION =
false`). Concurrency is therefore bounded: the only way a second context can run
"concurrently" with a task-context critical section is an **external interrupt**
arriving while interrupts are enabled. This document is the audit of every lock
that can be acquired from both task context (IF on) and IRQ context (IF cleared
by hardware on entry) — the *IRQ-shared* locks — and the rules that keep them
deadlock-free.

## The two Mutex flavors

`crate::utils::sync::Mutex` wraps `spin::Mutex` and offers two acquisition modes:

- **`lock()`** — disables preemption only. Interrupts stay on. Use this for locks
  that are **never** acquired from an IRQ handler on the same CPU.
- **`lock_irq()`** — disables interrupts (nest-safe) **and** preemption. Use this
  for any lock that is acquired from at least one IRQ handler, regardless of
  whether the other site is task or IRQ context.

The rule is one-directional and absolute:

> If a lock is *ever* acquired from an interrupt handler, **every** acquisition
> must use `lock_irq()`. A single `lock()` site on an IRQ-shared lock is a
> deadlock: the task-context holder spins with IF on, the IRQ fires, the handler
  tries to acquire the same lock, and spins forever because the holder can never
> make progress.

### Nest-safe IRQ disable

`lock_irq()` and `IrqGuard` route through `preempt::irq_save()` /
`preempt::irq_restore()`, which maintain a per-CPU depth counter
(`IRQ_DISABLE_DEPTH`). An inner `lock_irq()` made while interrupts are already off
increments the depth but records `was_enabled = false`, so its matching restore
does **not** re-enable interrupts prematurely. Only the outermost guard whose
`irq_save()` observed IF=1 re-enables. This makes nested `lock_irq()` calls safe.

## Why syscalls don't need `lock_irq()` for `PROCESS_TABLE`

`PROCESS_TABLE` is a `Once<ProcessTable>` accessed via raw `get_mut()` — no spin
lock at all. This is safe because of two invariants:

1. **Syscalls run with IF cleared.** `IA32_FMASK = 0x300` clears RFLAGS.IF on
   `syscall` entry, and the return path executes `cli` before restoring
   userspace. No syscall handler re-enables interrupts. So a syscall's
   `PROCESS_TABLE` access cannot be interrupted by the timer IRQ.
2. **The timer path is single-CPU non-concurrent.** `wake_from_timer` runs either
   under `irq_enter` (hard-IRQ context, IF off) or in `process_deferred_expiry`
   after `irq_exit` — but the latter runs in the interrupted task's own context,
   not alongside it. There is no second CPU.

Because both accessors have IF off (or are the same context), there is no
reentrant access. Wrapping every `PROCESS_TABLE.get_mut()` in an `IrqGuard` would
be redundant noise. The same reasoning applies to `UCHI_DEVICES` and the other
`Once<T>` / `static mut` structures accessed from both init and IRQ handlers:
init runs before interrupts are enabled, and the IRQ handler runs with IF off.

## IRQ-shared lock inventory

Every lock below is acquired from at least one IRQ handler. All sites use
`lock_irq()`.

| Lock | Location | Task-context sites | IRQ-context sites |
|------|----------|--------------------|--------------------|
| `PICS` | `arch/x86_64/idt.rs` | `init_pics`, `register_irq_handler`, `main` PIT mask | `dispatch_irq` EOI, `timer_preempt` EOI, keyboard/mouse ISR EOI |
| `Tty.input_buffer` | `sys/console/tty.rs` | `input_read_ready`, `pop_input_now`, `poll`, `ioctl` TCSETSF | keyboard ISR → `put_input`, UART IRQ4 → `put_char_in_tty` |
| `KEY_EVENT_QUEUE` | `driver/keyboard/mod.rs` | `KeyboardDev::pop_events`, `poll` | keyboard ISR → `enqueue_key_event` |
| `PACKETS` | `driver/mouse/mod.rs` | `MouseDev::read`, `poll` | mouse ISR → `enqueue_packet` |
| `TIMER_QUEUE` | `sys/timer/mod.rs` | `block_current_until`, `cancel_wait`, inserters | timer ISR → `expire_due`, `process_deferred_expiry` |
| `WaitQueue.waiters` | `utils/sync.rs` | `prepare_current`, `finish_wait` | keyboard/UART ISR → `notify_all`, timer → `notify_one` |

### Locks that are IRQ-context-only (not shared)

`KEYBOARD`, `PS2_KEYBOARD_STATE`, and `PACKET_STATE` are acquired only from their
respective ISRs today. They use `lock_irq()` for contract consistency and
future-safety: if a task-context reader is ever added, it will already be correct.

## Allocation-free WaitQueue

`WaitQueue` stores waiters in a fixed `[Option<u16>; 64]` slab, **not** a
`Vec<u16>`. This is mandatory, not an optimization: `notify_all` is called from
the keyboard ISR, and the global allocator's internal lock (`LockedHeap`) is
preempt-disable-only, not IRQ-safe. A heap allocation from IRQ context against a
task-context allocation in progress would deadlock. The fixed cap means no
`WaitQueue` operation ever allocates. Overflow drops the wake (logged); the caller
recovers via its deadline timeout or re-poll.

## Lock ordering

When two locks must be held simultaneously, take them in this order. Violating the
order risks ABBA deadlock across nested acquisitions.

1. `TIMER_QUEUE`
2. `NET` → `SOCKETS` (network poll path takes NET then SOCKETS; per-socket call
   sites mirror this)
3. `WaitQueue.waiters`
4. `PICS` (always released immediately after EOI; never held across another lock)

`Tty.input_buffer`, `KEY_EVENT_QUEUE`, and `PACKETS` are leaf locks — never held
while acquiring another lock.

## Scheduling rules

- The kernel is non-preemptive. A task-context critical section can only be
  interrupted by an external IRQ, never by another task. Preempt-disable
  (`PreemptGuard`) is therefore about disabling preemption at explicit safe points
  (`cond_resched`), not about preventing concurrent execution.
- `lock_irq()` is the only mechanism that prevents IRQ reentrancy. Use it for any
  IRQ-shared lock (see table above).
- Context-tracking counters (`HELD_LOCK_COUNT`, `FAULT_CONTEXT`,
  `ALLOCATOR_CONTEXT`, etc.) are **always active**, even with
  `ENABLE_KERNEL_PREEMPTION = false`. They back diagnostic assertions
  (`warn_if_schedule_unsafe`) and document which context a code path runs in.
  Keeping them active has negligible cost (one atomic op per lock/context) and
  makes lock-balance bugs observable in the non-preemptive kernel.
- `can_preempt_kernel()` remains gated on `ENABLE_KERNEL_PREEMPTION`; when false
  it returns `false` immediately. The counters it consults are now maintained
  regardless, so enabling kernel preemption later requires no further changes.

## Adding a new lock

1. Determine whether any IRQ handler will acquire it. If yes → `lock_irq()` at
   **every** site, and add a row to the inventory table.
2. If the lock is held across another lock acquisition, place it in the ordering
   list above and verify the order.
3. If the lock is acquired from an IRQ handler, ensure no operation under it can
   allocate (or that the allocation is IRQ-safe). The heap lock is not IRQ-safe.
4. Prefer leaf locks (no nested acquisition). If nesting is unavoidable, document
   the order here.
