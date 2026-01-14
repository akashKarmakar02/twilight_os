# Twilight OS

## Overview

Twilight OS is a lightweight operating system designed for general-purpose computing, embedded systems & learning purpose. It is written in Rust programming language.
It currently supports x86_64 architecture. future plans include support for ARM/RISC-V architecture.

## Twilight OS running basic Unix shell utilities

<table>
  <tr>
    <td><img src="docs/screenshots/img1.png" alt="screenshot1" width="400"></td>
    <td><img src="docs/screenshots/img2.png" alt="screenshot2" width="400"></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/img3.png" alt="screenshot1" width="400"></td>
    <td><img src="docs/screenshots/img4.png" alt="screenshot2" width="400"></td>
  </tr>
</table>

## Features

- Lightweight and efficient
- Terminal/TTY (kernel built-in)
- RTC + ACPI power off
- VFS with mount points (`/`, `/dev`, `/boot`)
- Filesystems: initramfs rootfs fallback, TwilightFS (TFS), FAT16 (/boot)
- Storage drivers: ATA + VirtIO block
- Networking: smoltcp stack + NIC drivers (RTL8139, PCNET) + DHCP + TCP/UDP
- Framebuffer device (`/dev/fb0`)
- Input devices (`/dev/input/mice`, keyboard)
- Userspace apps in `rootfs/bin` (e.g. `tsh`, modal `vi`, `logind`, core utils, `doom`, `chip8`)
- SMP detection (no multi-threading yet)

## Goal 0.1.0 Release

- [x] VFS & RamFS
- [x] Better user friendly Terminal
- [x] asynchronous I/O
- [x] memory management
- [x] PCI device detection
- [x] TFS Filesystem (in heavy development)
- [x] Network Stack
- [x] Userspace utilities (work remains on frame deallocation and process)
- [x] Basic shell
- [x] RTC
- [x] ATA
- [ ] Kernel Level NES Emulator
- [ ] DOOM (because why not?)

## Build Instructions

### ✅ Requirements

Twilight OS builds require:

- **Rust** (nightly pinned by `rust-toolchain.toml`, with `x86_64-unknown-none` target)
- `llvm-tools-preview` component
- `cargo build` with build-std
- `nasm` (for assembly boot code)
- `ld` (GNU binutils linker)
- `xorriso` (for ISO creation)
- `qemu` (for virtualization)
- `musl-gcc`
- `cpio` (for initramfs/rootfs archive)
- `mtools` + `sgdisk`/`gdisk` (only for `.hdd` targets)
- `git` + `curl` (Limine + OVMF downloads)

---

### ✅ Installing dependencies

#### **Linux**

- _Debian / Ubuntu_

```bash
sudo apt update
sudo apt install build-essential nasm qemu-system-x86 xorriso gcc musl-tools cpio mtools gdisk git curl
rustup target add x86_64-unknown-none
rustup component add llvm-tools-preview
```

- _Fedora_

```bash
sudo dnf install make nasm qemu-system-x86 xorriso musl-gcc cpio mtools gdisk git curl
rustup target add x86_64-unknown-none
rustup component add llvm-tools-preview
```

- _Arch Linux_

```bash
sudo pacman -S base-devel nasm qemu xorriso musl cpio mtools gptfdisk git curl
rustup target add x86_64-unknown-none
rustup component add llvm-tools-preview
```

---

#### **macOS**

You can use **Homebrew**:

```bash
brew install nasm qemu xorriso cpio mtools gptfdisk
rustup target add x86_64-unknown-none
rustup component add llvm-tools-preview
```

---

#### **Windows**

We recommend using **WSL2** with Ubuntu/Fedora:

1. Install WSL2 following Microsoft’s guide
2. Inside WSL, follow the same instructions as Linux above

---

## ✅ Building & Run

In the workspace directory, run:

```bash
make run
```

Other useful targets:

- `make twilight-os.iso` (build ISO)
- `make run-x86_64` (BIOS + IDE disk)
- `make run-x86_64-uefi` (UEFI + IDE disk; downloads OVMF if needed)
- `make run-blk-bios` (BIOS + VirtIO block device)
- `make all-hdd` / `make run-hdd` (build/run `.hdd` image; requires `mtools` + `sgdisk`)

## ✅ First Boot

On first boot, you **must** initialize the filesystem:

```bash
install
```

inside the VM shell to format your disk.

## Userspace Notes

- `userspace/` is built via `cargo build --release` and its build script compiles apps under `userspace/apps/*` and copies resulting binaries into `rootfs/bin/`.
- `vi` is modal (VIEW/INSERT) with `:w`, `:q`, `:wq`, `:q!`.
- `logind` can create users and writes to `/etc/passwd`.

## Documentation

Twilight OS documentation is available at [https://twilight-os.vercel.app](https://twilight-os.vercel.app).

## License

Twilight OS is licensed under the BSD-3 Clause License. See the [LICENSE](LICENSE) file for details.

## Contributing

Contributions to Twilight OS are welcome!
