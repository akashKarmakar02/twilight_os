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
- On a timer interrupt, `timer_preempt()` (PIT) and `apic_timer_preempt()` can switch to a different user task if a saved preempt frame exists
- `maybe_schedule()` is currently a stub (used to avoid unsafe switches when the kernel is mid-transition)

This means preemption exists but is conservative and avoids switching in unsafe contexts.

## Process exit
`exit()`:
- Marks the process as dead
- Cleans up its address space
- Switches back to the parent if one exists

## Limitations and TODOs
- Preemptive scheduling is basic and still guarded by safety checks
- No copy-on-write for fork
- No full signal model

These choices keep the kernel smaller and easier to reason about for a teaching OS.
