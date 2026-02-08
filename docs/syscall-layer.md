# TwilightOS Syscall Layer & Linux Compatibility

This document outlines the design and implementation of the system call (syscall) layer in TwilightOS, focusing on its architecture and the strategic benefits of maintaining Linux compatibility.

## Architecture Overview

TwilightOS utilizes the modern `syscall`/`sysret` instruction pair for x86-64 system calls, providing a low-overhead transition between userspace (Ring 3) and kernel space (Ring 0).

### Entry Point
The entry point is defined in `twilight_kernel/src/arch/x86_64/syscall.rs`. The kernel configures the Model Specific Registers (MSRs) `IA32_LSTAR` to point to the `x86_64_syscall_handler` assembly routine. This routine:
1.  Swaps the `gs` register to kernel GS (using `swapgs`).
2.  Saves the userspace stack pointer and registers.
3.  Calls the high-level Rust function `syscall_handler`.

### Dispatcher
The core logic resides in `twilight_kernel/src/sys/syscall/mod.rs`. The `syscall_handler` function receives the syscall number (`rax`) and up to 6 arguments (`rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9`), conforming to the **System V AMD64 ABI**.

A central match statement dispatches the request to the appropriate kernel service:
```rust
match syscall_number {
    SYS_READ => ...,
    SYS_WRITE => ...,
    SYS_OPEN => ...,
    // ...
}
```

## Linux Compatibility

A key design decision in TwilightOS is to maintain strict ABI compatibility with the Linux x86-64 kernel.

### 1. Syscall Numbering
TwilightOS uses the exact same syscall numbers as Linux.
*   `SYS_READ` = 0
*   `SYS_WRITE` = 1
*   `SYS_OPEN` = 2
*   ...and so on.

This definition can be found in `twilight_common/src/syscall/numbers.rs`.

### 2. Data Structures
Kernel structures meant to be shared with userspace (e.g., `Timespec`, `Rlimit64`, `Stat`) are designed to match the memory layout of their Linux counterparts. This ensures that a pointer to a struct passed from a Linux-compatible userspace program is interpreted correctly by the TwilightOS kernel.

### 3. Behavior
Specific syscalls implement Linux-specific behaviors. For example, `SYS_REBOOT` checks for Linux-specific magic numbers (`0xfee1dead`, `672274793`, etc.) to authorize the reboot command.

## Benefits of Linux Compatibility

Adhering to the Linux ABI offers significant advantages for OS development:

### **1. Simplified Porting of Userspace Applications**
By mimicking the Linux syscall interface, we can reuse existing Standard C Libraries (LibC) like **musl** or **glibc** with minimal patching.
*   Applications built against these libraries generate standard Linux syscalls.
*   Since TwilightOS "speaks" the same language, these applications can run with little to no modification to their source code.

### **2. Ecosystem & Toolchain Reuse**
*   **Compilers**: `gcc`, `clang`, and `rustc` already know how to emit code for `x86_64-unknown-linux-gnu` or `x86_64-unknown-linux-musl`. We can target these existing triples instead of maintaining a custom toolchain fork.
*   **Debuggers**: Tools like `gdb` that understand Linux stack frames and signal handling concepts will be easier to integrate.

### **3. Binary Compatibility (Future Goal)**
Strict adherence to the ABI opens the possibility of **binary compatibility**. In theory, a statically linked Linux binary could run directly on TwilightOS without recompilation, provided all requisite syscalls are implemented.

### **4. Documentation & Standards**
Instead of defining and documenting a custom set of system calls, we can rely on the extensive documentation available for Linux (e.g., `man` pages). This lowers the barrier to entry for new developers working on TwilightOS.
