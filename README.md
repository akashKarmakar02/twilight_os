# Twilight OS

## Overview

Twilight OS is a lightweight, Unix-like operating system written in Rust, designed for general-purpose computing, embedded systems, and detailed OS dev learning. It bridges the gap between educational kernels and usable systems by implementing advanced features like dynamic binary loading, a self-hosting C compiler, a custom filesystem, a from-scratch Wayland compositor, and a Linux-style timekeeping subsystem.

It currently targets **x86_64** (Limine boot, UEFI + BIOS).

## Screenshots

<table>
  <tr>
    <td><img src="docs/screenshots/img1.png" alt="Basic Shell" width="400"></td>
    <td><img src="docs/screenshots/img2.png" alt="Userspace Apps" width="400"></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/img3.png" alt="Networking" width="400"></td>
    <td><img src="docs/screenshots/img4.png" alt="Graphical Output" width="400"></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/img5.png" alt="twclock — Wayland layer-shell clock" width="400"></td>
    <td><img src="docs/screenshots/img6.png" alt="smolnes — Super Mario Bros. 3" width="400"></td>
  </tr>
</table>

## Key Features

### Userspace & Tooling
- **Native C Compilation**: Includes `tcc` (Tiny C Compiler) to compile and run C programs directly within the OS (`tcc -run hello.c`).
- **Dynamic Binaries**: Full ELF shared object (`.so`) and dynamic executable support with a kernel ELF loader (PT_LOAD, PT_INTERP, auxv).
- **Scripting**: `tpy`, a Python-subset interpreter, and `bc`, an arbitrary-precision calculator.
- **Shell & Utilities**: `oksh` (pdksh-based shell) and `tsh`, plus coreutils-style `cat`, `grep`, `ls`, `curl`, `vi`, `head`/`tail`, `wc`, `cp`, `mkdir`, `rm`, `touch`, `date`, `uname`, and more.
- **Signals**: Full POSIX signal delivery — `rt_sigaction`, `rt_sigprocmask`, `rt_sigsuspend`, `sigaltstack`, `kill`/`tgkill` — with correct sleep interruption (`-EINTR` + remainder).
- **Init System**: `twinit`, a runit-like PID 1 service manager with TOML service files, restart policies, runlevels, and a control socket (`twinitctl`).
- **Logging**: `twilogd`, a system log daemon collecting structured logs over a Unix-domain socket (`twilogctl` to query/tail).
- **Package Management**: `.ipk` packages via a bundled `opkg`.
- **Wayland Compositor**: `twland`, a from-scratch Wayland compositor speaking raw wire protocol over Unix sockets — xdg-shell toplevel windows, wlr-layer-shell desktop surfaces, SHM buffer compositing to `/dev/fb0`, and real keyboard/mouse input. Includes native clients like `twclock`.
- **Emulators & Games**: Doom (`fbdoom`), a CHIP-8 emulator, `smolnes` (NES, with NTSC frame pacing + gamepad input), and a Snes9x framebuffer port.

### Kernel & Core
- **Language**: Written in pure Rust (no standard library), `no_std`.
- **Scheduling**: Preemptive userspace scheduling driven by a one-shot LAPIC clockevent with a 10 ms quantum; deferred-reschedule kernel critical sections (`cond_resched()` safe points); experimental kernel-mode preemption implemented but disabled by default. SMP detection with APs parked (BSP-only scheduling for now).
- **Timekeeping**: A Linux-style split between clocksource and clockevent — invariant-TSC or HPET MMIO clocksource (selected at boot, validated) feeding `CLOCK_MONOTONIC`/`CLOCK_REALTIME`; one-shot LAPIC clockevents programming the earliest of the next software deadline or the scheduler quantum; an absolute-deadline min-heap timer queue backing `nanosleep`, `clock_nanosleep`, `poll`/`ppoll`/`select`.
- **Filesystem**: **TwilightFS** (TFS) — a custom bitmap-based filesystem with hot-file caching, efficient directory lookups, extent-based block mapping, and encrypted home directory support. VFS with FAT16, FAT32, ISO9660, ramfs, devfs, and procfs backends.
- **Networking**: Custom network stack (smoltcp) — TCP/UDP, DHCP, DNS — with drivers for RTL8139 and PCNET. Use `curl` to fetch pages or `httpd` to serve them.
- **IPC**: Pipes, Unix-domain sockets (SOCK_STREAM + SOCK_DGRAM, `SCM_RIGHTS` fd passing), `memfd_create` + `mmap MAP_SHARED`, and `futex`.
- **Locking**: IRQ-aware `Mutex` with nest-safe `lock_irq()` (depth-counted interrupt disable), allocation-free `WaitQueue`s, and a documented lock-ordering audit of every IRQ-shared lock.
- **FreeBSD KPI compat layer**: A shim (`compat/freebsd_kpi/`) providing bus_space, bus_dma, callout, taskqueue, and mtx primitives so FreeBSD-style drivers can compile.
- **Drivers**:
    - **Storage**: ATA/IDE with DMA, ATAPI CD-ROM, VirtIO Block.
    - **USB**: UHCI and XHCI host controllers; HID keyboard/mouse/gamepad (`/dev/input/js0`) and MSC mass storage.
    - **Input**: PS/2 keyboard & mouse, USB keyboard & mouse, gamepad.
    - **Network**: RTL8139, PCNET.
    - **Display**: UEFI GOP framebuffer (`/dev/fb0`).
    - **Time**: TSC, HPET, PIT, CMOS/RTC, LAPIC one-shot timer.
    - **Other**: APIC/IOAPIC, UART 16550, PCI enumeration.

## Roadmap & Status

| Feature              | Status           | Details                                                        |
|:---------------------|:-----------------|:---------------------------------------------------------------|
| **Paging & Memory**  | ✅ Done           | Physical/virtual allocation, user/kernel separation, slab+heap  |
| **VFS & TwilightFS** | ✅ Done           | Custom on-disk FS, FAT16/32, ISO9660, devfs, procfs, mount pts |
| **Timekeeping**      | ✅ Done           | TSC/HPET clocksource, one-shot LAPIC clockevents, deadline queue|
| **Dynamic Linking**  | ✅ Done           | ELF loader, shared libraries, auxv, dynamic linker             |
| **Networking**       | ✅ Done           | TCP/UDP stack, DHCP/DNS, `httpd` server, `curl` client         |
| **IPC**              | ✅ Done           | Unix sockets, pipes, memfd, SCM_RIGHTS fd passing, futex       |
| **Signals**          | ✅ Done           | `rt_sigaction`/sigprocmask/sigsuspend, kill/tgkill, EINTR      |
| **Userspace**        | ✅ Done           | `tcc`, `tpy`, `bc`, `vi`, `oksh`, emulators, compositor        |
| **Multitasking**     | 🚧 Beta          | Preemptive userspace scheduling; deferred-reschedule kernel; SMP detection (APs parked) |
| **Graphics**         | 🚧 Experimenting | Wayland compositor (xdg-shell + layer-shell), framebuffer      |
| **Package Mgmt**     | 🚧 Beta          | `.ipk` packages via bundled `opkg`                             |
| **Doom**             | ✅ Done           | Vendored doomgeneric framebuffer and input port                |

## Build Instructions

### ✅ Requirements

- **Rust** (nightly, via `rust-toolchain.toml`)
- `llvm-tools-preview`
- `nasm`, `ld` (binutils), `xorriso`
- `qemu` (emulator)
- `musl-gcc` (for userspace LibC)
- `cpio` (initramfs)

### ✅ Quick Start

1. **Install Dependencies** (Ubuntu/Debian example):
   ```bash
   sudo apt install build-essential nasm qemu-system-x86 xorriso gcc musl-tools cpio git curl
   rustup target add x86_64-unknown-none
   rustup component add llvm-tools-preview
   ```

2. **Build & Run**:
   ```bash
   make run
   ```
   *This downloads necessary bootloader files (Limine) and OVMF firmware automatically.*

3. **First Boot**:
   Inside the OS shell, initialize the disk:
   ```bash
   install
   ```
   Then reboot to use the persistent TwilightFS.

### Other useful targets

| Target              | Description                                                        |
|:--------------------|:------------------------------------------------------------------|
| `make run-x86_64-uefi` | Boot via OVMF UEFI firmware                                    |
| `make run-bios`     | q35 machine, BIOS boot                                            |
| `make run-blk-bios` | virtio-blk + qemu-xhci                                            |
| `make test-time`    | Non-destructive headless timing regression matrix (no disk write) |
| `make all-hdd`      | Build a GPT HDD image instead of an ISO                           |

## Userspace Highlights

The `userspace/` directory contains a full suite of applications. A workspace `build.rs` auto-detects each app's build system (Cargo.toml, build.zig, Makefile, or plain C) and builds static binaries into `rootfs/bin/`.

- **`tcc`**: The compiler. Try `tcc -run hello.c`!
- **`twland`**: The Wayland compositor — xdg-shell windows, layer-shell desktop surfaces, SHM rendering.
- **`twinit`**: PID 1 service manager supervising `twilogd`, `httpd`, `twland`, and more.
- **`httpd`**: A non-blocking HTTP server serving `/var/www`.
- **`smolnes` / `fbdoom` / `chip8`**: Emulators and games running on the framebuffer.
- **`clockcheck`**: Guest-side timing probe paired with the `tools/time-regression/` host harness.

## Documentation

Developer-facing documentation lives in [`docs/`](docs/) — covering the [timekeeping subsystem](docs/timekeeping.md), [kernel locking](docs/kernel-locking.md), the [init system](docs/system/twinit.md), [Wayland compositor](docs/system/twland_wayland_transport.md), [filesystem internals](docs/filesystem/), [USB drivers](docs/usb/), and more.

View the documentation online at [https://twilight-os.vercel.app](https://twilight-os.vercel.app).

## License

Twilight OS is licensed under the BSD-3 Clause License. See the [LICENSE](LICENSE) file for details.
