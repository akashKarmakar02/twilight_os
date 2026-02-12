# TwilightOS Userspace & Linux Compatibility Architecture

TwilightOS is designed to be **binary-compatible with Linux** on the x86-64 architecture. This allows standard Linux applications, particularly those statically linked with `musl` or dynamically linked against a standard Linux loader, to run on TwilightOS with little to no modification.

This document details the architectural stack that enables this compatibility, from the kernel's binary loader to the userspace runtime.

## 1. The Userspace Environment

Userspace applications in TwilightOS are standard **ELF64** binaries. The primary C library used is **musl libc**, chosen for its lightweight static linking capabilities and strict standards adherence.

### Build System
The build system (`userspace/build.rs`) compiles applications using standard Linux cross-compilation targets:
- **Target**: `x86_64-unknown-linux-musl`
- **Compiler**: `musl-gcc` or `cargo build --target ...`
- **Binary Format**: Static PIE (Position Independent Executable) or standard static ELF.

Because the build target is standard Linux, the compiler emits:
- Standard Linux System Call numbers (e.g., `SYS_read` = 0, `SYS_write` = 1).
- Standard Linux memory layouts for system structs (e.g., `struct stat`, `struct timespec`).

## 2. Dynamic Binary Loading

TwilightOS implements a Linux-compatible ELF loader in the kernel (`twilight_kernel/src/sys/proc/mod.rs`). This allows it to load both static binaries and dynamically linked executables that require an interpreter (dynamic linker).

### The Loading Process
When `Process::exec()` is called:
1.  **Header Parsing**: The kernel validates the ELF magic and parses the Program Headers (`Phdr`).
2.  **Mapping Segments**: It iterates through `PT_LOAD` segments and maps them into the user's virtual address space (clearing BSS if necessary).
3.  **Interpreter Handling (`PT_INTERP`)**:
    - If the binary specifies an interpreter (e.g., `/lib/ld-musl-x86_64.so.1`), the kernel **also** loads this interpreter into memory.
    - The entry point is set to the *interpreter's* entry point, not the main binary's.
4.  **Auxiliary Vector (`auxv`) Setup**:
    - The dynamic linker needs information about the running process to bootstrap itself. TwilightOS places an **Auxiliary Vector** on the stack, just like Linux.
    - Key vectors provided include:
        - `AT_PHDR`: Address of the program headers.
        - `AT_ENTRY`: The original entry point of the application.
        - `AT_BASE`: The base address where the interpreter was loaded.
        - `AT_RANDOM`: Pointer to random bytes (for stack canaries, etc.).

This allows the standard Linux dynamic linker to initialize, load shared libraries (like `libc.so`), and jump to the application's `main()`.

## 3. The Syscall Interface

Transitions between Ring 3 (Userspace) and Ring 0 (Kernel) uses the fast `syscall` instruction, conforming to the **System V AMD64 ABI**.

### Syscall Dispatch
1.  **Userspace**: The application (or libc) places the syscall number in `rax` and arguments in `rdi`, `rsi`, `rdx`, `r10`, `r8`, `r9`. It executes `syscall`.
2.  **Kernel Entry**: The `syscall_handler` (defined in `arch/x86_64/syscall.rs`) handles the trap.
3.  **Dispatch**: The handler decodes `rax` and routes the call to the appropriate kernel service via a large match statement in `sys/syscall/mod.rs`.

### Linux Compatibility Layer
TwilightOS implements "Linux Compatibility" by:
1.  **Matching Numbers**: The kernel uses the exact same syscall numbers as Linux.
    - `SYS_WRITE` (1) maps to the `sys_write` implementation.
    - `SYS_MMAP` (9) maps to `sys_mmap`.
2.  **Matching Structs**: Data structures passed to syscalls are defined to match Linux's memory layout exactly.
    - **Example**: `struct timespec` is defined as `tv_sec` (i64) followed by `tv_nsec` (i64).
    - **Example**: `struct sockaddr_in` matches the standard networking layout.
3.  **Behavioral Mimicry**:
    - **Magic Numbers**: `SYS_REBOOT` checks for Linux-specific magic constants (`0xfee1dead`, etc.).
    - **Error Codes**: The kernel returns standard negative errno values (e.g., `-ENOENT`, `-EBADF`).

## 4. Example: Flow of a custom utility
Consider a custom utility `curl` running on TwilightOS:

1.  **Userspace**: `curl` calls `write(1, "hello", 5)`.
    - This invokes `musl`'s `write` wrapper.
    - `musl` executes `syscall` with `rax=1`, `rdi=1`, `rsi="hello"`.
2.  **Transition**: CPU switches to Ring 0, jumping to `x86_64_syscall_handler`.
3.  **Kernel Dispatch**:
    - `syscall_handler` sees `rax=1`.
    - Calls `service::write(1, ptr, 5)`.
4.  **Execution**:
    - `service::write` looks up File Descriptor 1 in the current process's `fd_table`.
    - It finds the handle for `stdout` (e.g., a TTY or Pipe).
    - It writes the data to the driver.
5.  **Return**:
    - The kernel returns the number of bytes written (5) in `rax`.
    - `sysret` returns to userspace.

## Conclusion
By adopting the Linux ABI as the "native" interface, TwilightOS avoids the need for maintaining a custom userspace toolchain. Custom utilities can be written in standard C/Rust/Zig, compiled with standard Linux targets, and run simply by virtue of this architectural compatibility.
