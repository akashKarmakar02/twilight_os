# TwilightOS System Specifications

This document outlines the hardware and software specifications for TwilightOS.

## Hardware Specifications

### Architecture
- **Target Architecture**: `x86_64` (AMD64 / Intel 64)
- **Boot Protocol**: Limine (UEFI & BIOS compatible, generally running on UEFI with OVMF in QEMU)
- **CPU Modes**: Long Mode (64-bit), Paging enabled (4-level paging), Ring 0 (Kernel) / Ring 3 (User).

### Platform Support
- **Primary Platform**: QEMU `q35` or standard `pc` (i440fx) emulation.
- **Physical Hardware**: Generic x86_64 PCs (testing primarily on virtualized environments).

### Peripheral Drivers
TwilightOS includes built-in kernel drivers for the following hardware:

| Category | Driver / Controller | Notes |
| :--- | :--- | :--- |
| **Interrupts** | APIC / IOAPIC | Local APIC and I/O APIC support for IRQ routing. |
| **Timer** | LAPIC, PIT | One-shot LAPIC clockevents; PIT used for calibration/fallback support. |
| **Clocksource** | TSC, HPET | Validated free-running source for monotonic time. |
| **Time** | CMOS / RTC | Real Time Clock for wall-clock time. |
| **Serial** | UART (16550) | Serial console output (`COM1`) for debug logging. |
| **Storage** | ATA / IDE | Legacy PATA support. |
| **Storage** | VirtIO Block | Paravirtualized high-performance disk I/O. |
| **Input** | PS/2 | Dual-channel PS/2 Keyboard and Mouse controller. |
| **USB** | UHCI, XHCI | Universal Host (USB 1.1) and eXtensible Host (USB 3.0) controllers. |
| **USB Devices** | HID, MSC | USB Keyboard/Mouse and Mass Storage Class support. |
| **Network** | RTL8139 | Realtek 8139 10/100 Fast Ethernet. |
| **Network** | PCNET | AMD PCnet-FAST III (Am79C973). |
| **Display** | UEFI GOP | UEFI Graphics Output Protocol (Linear Framebuffer). |

Generic HID gamepads are development work outside current `master`; see the
[gamepad design/status](usb/gamepads.md).

---

## Software Specifications

### Kernel Architecture
- **Language**: Rust (No-std, `alloc` enabled).
- **Design**: Monolithic kernel with modular drivers.
- **Scheduler**: Preemptive userspace with a 10 ms quantum, deferred kernel
  rescheduling, and experimental kernel preemption disabled by default. APs
  are detected and parked; scheduling remains BSP-only.
- **Memory Management**:
    - Physical Memory Manager (Bitmap/Linked-list).
    - Virtual Memory Manager (Recursive mapping / Direct map).
    - Higher-half kernel.

### Userspace Architecture
- **ABI**: System V AMD64 ABI.
- **Binary Format**: ELF64 (Executable and Linkable Format).
    - Supports Static Executables.
    - Supports Dynamic Executables (Interpreter/Loader).
- **C Library (LibC)**: Musl LibC (ported/compatible).
    - Provides standard C library functions (`printf`, `malloc`, etc.).
    - Links against TwilightOS syscalls.

### System Calls
- **Mechanism**: `syscall` / `sysret` instructions (fast system calls).
- **Compatibility**: Linux-compatible syscall numbers and behavior for core functions.
    - Examples: `sys_read` (0), `sys_write` (1), `sys_open` (2), `sys_fork` (57), `sys_execve` (59).
    - *Note: Not all Linux syscalls are implemented; specific subset for common tools (make, gcc, vim) is supported.*

### Filesystem (TwilightFS)
- **Type**: Custom VFS implementation.
- **Features**:
    - Mount points (e.g., `/dev`, `/proc`).
    - FAT16 support (for `/boot` partition or EFI system partition).
    - Custom inode-based structures for persistent storage.
    - Ramfs / Devfs / Procfs virtual filesystems.

### Build System & Toolchain
- **Host Toolchain**:
    - `cargo` + `rustc` (Nightly) for Kernel.
    - Rust stable with `x86_64-unknown-linux-musl` for Rust userspace.
    - `musl-gcc` (via `musl-tools`) for C userspace applications.
    - `nasm` for assembly stubs.
- **Userspace Builder**:
    - Custom `build.rs` script in `userspace/` directory.
    - Detects Makefile, Cargo, Zig, then plain C/C++ sources.
    - Stages app executables under `rootfs/bin` and service aliases under `rootfs/sbin`.
    - See [build-system.md](build-system.md) for image packaging and external Snes9x prerequisites.
