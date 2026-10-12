# Getting Started

## Requirements
- Rust nightly (see rust-toolchain.toml) and `llvm-tools-preview`
- `nasm`, `ld` (binutils)
- `qemu-system-*`, `qemu-img`
- `xorriso`
- `cpio`
- `musl-gcc` (or set CC to another static-capable compiler)
- `git`, `curl`
- `sgdisk` (gdisk package)
- `mtools` (mformat, mmd, mcopy)
- Rust stable with `x86_64-unknown-linux-musl`
- Meson, Ninja, pkg-config, and host `wayland-scanner`
- [External Snes9x source and Alpine distrobox](../userspace/apps/snes9x/README.md)

## Build and run (x86_64)

Initialize Git submodules with `git submodule update --init --recursive`.
See [build-system.md](build-system.md) for toolchain setup.

1. Build and boot an ISO:
   ```bash
   make run
   ```
   This creates `twilight-os.iso` and boots it in QEMU. A data disk `hdd.img` is created if missing.

2. The live ISO works without installation. To initialize a new development
   data disk for persistent TwilightFS storage, run inside the OS shell:
   ```sh
   install
   ```
   This initializes the target disk. Reboot afterward to use the installation.

## HDD image boot
```bash
make run-hdd
```
This uses `twilight-os.hdd` (GPT/FAT boot partition containing Limine, the
kernel, bootstrap initramfs, and the system image).

For guest-to-host downloads, use `10.0.2.2` with default QEMU user networking;
see [development.md](development.md#qemu-networking-and-host-files).

## Other architectures (experimental)
The Makefile includes targets for `aarch64`, `riscv64`, and `loongarch64` (for example `make run KARCH=riscv64`). These are provided for experimentation and may not be fully supported.
