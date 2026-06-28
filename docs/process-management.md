# Process Management

Twilight OS uses a lightweight process model with ELF loading, per-process page tables, and a round-robin scheduler. The implementation is mainly in `twilight_kernel/src/sys/proc/`.

## Process table and state
Processes are stored in a global `ProcessTable` with a `VecDeque<Process>`.

States:
- `Running`
- `Sleeping`
- `Waiting`
- `Dead`

PIDs are allocated from an atomic counter (`NEXT_PID`). The current PID is stored in `PID`.

## Process structure
`Process` contains:
- `page_table_frame` and `mapper`: the process page table and mapper
- `stack`, `stack_size`: user stack
- `context` and `context_switch_rsp`: kernel context for switching
- `fpu_storage`: saved FPU state
- `fs_base`, `gs_base`: user FS/GS base values
- `kernel_gs`: per-process kernel GS data (kernel rsp and user rsp)
- `fd_table`: open file descriptors
- `pwd`: current working directory
- `proc_mm`: per-process memory manager (heap, mmap tracking)
- `preempt_frame`: saved trap frame used by the preemptive timer switch

## Process creation (ELF loading)
`Process::new()` loads an ELF image into a fresh address space:
- Validates ELF magic
- Loads PT_LOAD segments
- Handles PT_INTERP to load a dynamic interpreter
- Builds a user stack with argv/envp/auxv
- Creates a `ProcMM` based on the maximum loaded address

Dynamic loading uses fixed base hints:
- Main image: `0x4000_0000`
- Interpreter: `0x6000_0000`

User stack:
- Top at `0x0000_7FFF_FFFF_F000`
- Size `0x64000`

## exec
`exec()` replaces the current process image:
- Allocates a fresh page table
- Loads the new ELF and rebuilds the stack
- Resets `ProcMM`
- Preserves open file descriptors except those with CLOEXEC

## fork
`fork()` deep-copies the user address space:
- Creates a new page table
- Allocates new pages for each mapped region
- Copies memory page-by-page
- Clones file descriptors and `ProcMM`
- Returns 0 in the child

This is a full copy, not copy-on-write.

## Context switching
Context switching is done by `switch_tasks()` using a saved `Context` structure. The kernel builds a synthetic interrupt frame so it can `iretq` into user space when starting a new process.

Important details:
- The TSS rsp0 is updated to the new kernel stack
- FS/GS base values are saved and restored across switches
- FPU state is saved with `xsave` and restored with `xrstor`

## Scheduling
Scheduling is round-robin:
- `schedule_now()` searches for the next `Running` process
- On a timer interrupt, `timer_preempt()` (PIT) and `apic_timer_preempt()` set `need_resched`
- A timer interrupt from userspace retains the existing direct scheduling path
- A timer interrupt from kernel mode returns without scheduling; the pending request is handled at a safe task-context point
- The central syscall return path calls `cond_resched()` after syscall work and signal delivery are complete

Phase 1 preemption accounting is implemented in `sys/preempt.rs`. It tracks:

- `preempt_count`: nesting of kernel regions that disable preemption
- `irq_depth`: timer interrupt nesting
- `in_scheduler`: scheduler reentrancy protection
- `need_resched`: a deferred scheduling request

`cond_resched()` schedules only when the request is pending and all three safety conditions are clear. Counter underflow is prevented and reported on the serial console. Scheduler bookkeeping is explicitly released immediately before the interrupt-disabled architecture switch because a newly started task may not return through the previous task's Rust stack.

The state is temporarily BSP-global. Although Twilight has a `CpuLocal<T>` facility, scheduled processes currently repurpose kernel GS for syscall stack data, so that accessor cannot safely hold runtime scheduler state. This is sufficient while userspace scheduling remains BSP-only and must be replaced with genuine per-CPU storage before scheduling tasks on APs.

### Phase 2 critical-section rules

Kernel `Mutex` and `RwLock` now use preemption-aware RAII guards. Acquiring a
guard increments `preempt_count`; dropping it releases the underlying spinlock
first and then decrements `preempt_count`. Lock guard destruction uses the
no-reschedule form of preemption enable, so an unlock in device IRQ context
cannot become an implicit scheduling point. Scheduling remains restricted to
explicit, audited task-context safe points.

The allocator is wrapped by the same accounting rule, including allocation,
deallocation, initialization, and statistics. Scheduler/process, VFS, fd,
network socket, page-frame, PCI, console, and device spinlocks use the common
preemption-aware wrappers.

Blocking operations must release every guard before calling `await_io()`.
Pipe and Unix-socket wait paths already follow this pattern. The syscall layer
also clones Unix socket state before a blocking read/write/accept/recvfrom, and
TTY reads check canonical-line readiness before dropping fd/VFS guards and
sleeping. The operation reacquires state and retries after wakeup.

Diagnostics reject and report:

- `cond_resched()` with a nonzero `preempt_count`
- scheduler entry with a nonzero `preempt_count`
- nested scheduler entry
- preemption or IRQ-depth counter underflow

Phase 2 does not enable kernel-mode timer preemption. The `from_user` timer
guard remains in place; kernel-mode ticks only set `need_resched`.

### Phase 3 scheduler states

`ProcessState` now distinguishes CPU ownership from scheduling eligibility:

- `Running`: the single process currently executing on the BSP
- `Runnable`: ready for scheduler selection, but not currently executing
- `Waiting`, `SignalWait`, `AwaitingIo`, and `Stopped`: blocked or stopped and
  therefore ineligible for selection
- `Dead`: exited and never eligible for selection

New processes and threads created by spawn, `fork`, or `clone` begin as
`Runnable`. Wakeups also move blocked processes to `Runnable`. Only the central
context-switch boundary may promote a process to `Running`; when switching
away from a still-running process, it is demoted back to `Runnable`.

The BSP scheduler validates that no non-current process is marked `Running`
before a switch and that exactly the selected PID is `Running` immediately
before entering the architecture switch. A target that is not `Runnable` is
rejected, preventing dead, waiting, I/O-blocked, stopped, or signal-waiting
processes from being scheduled.

Phase 3 still does not enable kernel-mode timer preemption. Timer interrupts
received in kernel mode only defer rescheduling; the `from_user` guard remains.

### Phase 1 test

Boot Twilight and launch the CPU-bound counter test from `tsh`. It forks once
before entering the loop, producing two runnable counter processes:

```sh
preempt_counter &
```

Both PIDs should continue printing increasing counters. While they run, exercise syscall-heavy and network paths from the host:

```sh
curl --max-time 5 http://127.0.0.1:8000/
curl --max-time 5 http://127.0.0.1:8000/style.css
```

In the guest, verify service and request logging remains responsive:

```sh
twinitctl status
twilogctl show
```

Pass conditions:

- both counters advance rather than one monopolizing the CPU
- HTTP requests complete and appear in the log
- no reboot, nested-scheduler warning, or preemption-counter underflow occurs
- timer ticks received in kernel mode only set `need_resched`; they do not switch directly

Stop the counters with `kill PID` after recording their PIDs.

## Process exit
`exit()`:
- Marks the process as dead
- Cleans up its address space
- Switches back to the parent if one exists

## Limitations and TODOs
- Preemptive scheduling is basic and still guarded by safety checks
- Preemption state is BSP-global rather than genuinely per-CPU
- There is no real runqueue
- Kernel-mode timer interrupts must not schedule directly yet
- No copy-on-write for fork
- No full signal model

These choices keep the kernel smaller and easier to reason about for a teaching OS.
