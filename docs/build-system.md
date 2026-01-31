# Build System

Twilight OS is built with `make` at the repo root. The top-level `GNUmakefile` orchestrates kernel, userspace, and image creation.

## Common targets
- `make` or `make all`: build `twilight-os.iso`
- `make all-hdd`: build `twilight-os.hdd`
- `make run`: boot the ISO in QEMU (x86_64 by default)
- `make run-hdd`: boot the HDD image in QEMU
- `make cpio`: rebuild `rootfs.cpio` from `rootfs/`
- `make clean`: remove build artifacts
- `make distclean`: also remove downloaded `limine/` and `ovmf/`

## Variables
- `KARCH`: target architecture (default `x86_64`). Other targets exist in the Makefile: `aarch64`, `riscv64`, `loongarch64`.
- `QEMUFLAGS`: extra flags appended to QEMU commands.
- `RUSTUP_TOOLCHAIN`: set automatically for some musl hosts, but you can override if needed.

## Build outputs
- `twilight-os.iso`: bootable ISO with Limine and rootfs.
- `twilight-os.hdd`: bootable HDD image (FAT partition with Limine + rootfs).
- `rootfs.cpio`: initramfs archive built from `rootfs/`.
- `hdd.img`: data disk used by the `make run` target.
