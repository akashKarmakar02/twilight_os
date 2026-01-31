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

## Build and run (x86_64)
1. Build and boot an ISO:
   ```bash
   make run
   ```
   This creates `twilight-os.iso` and boots it in QEMU. A data disk `hdd.img` is created if missing.

2. First boot: inside the OS shell run:
   ```sh
   install
   ```

## HDD image boot
```bash
make run-hdd
```
This uses `twilight-os.hdd` (FAT partition with Limine + rootfs).

## Other architectures (experimental)
The Makefile includes targets for `aarch64`, `riscv64`, and `loongarch64` (for example `make run KARCH=riscv64`). These are provided for experimentation and may not be fully supported.
