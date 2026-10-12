# Build System

The top-level `GNUmakefile` builds the kernel, stages userspace, and packages
Limine boot images. x86_64 is supported; other architecture recipes are
experimental.

## Prerequisites

The kernel uses the pinned nightly in `kernel/rust-toolchain.toml`, the
`x86_64-unknown-none` target, and `llvm-tools-preview`. Userspace uses Rust
stable with `x86_64-unknown-linux-musl`, musl-gcc, and make. Native tools include
nasm, binutils, xorriso, cpio, QEMU, git, and curl. Wayland builds need Meson,
Ninja, pkg-config, a host C compiler, and Expat development headers
(`libexpat1-dev` on Debian/Ubuntu, `expat-devel` on Fedora). The wrapper builds
a matching native scanner from the pinned Wayland source. The libffi source
submodule needs Autoconf, Automake, and Libtool.
HDD images need gdisk and mtools.

Initialize Git submodules and the userspace toolchain:

```sh
git submodule update --init --recursive
rustup toolchain install stable
rustup target add --toolchain stable x86_64-unknown-linux-musl
```

The Snes9x source comes from the `userspace/snes9x-fb` submodule. Its wrapper
also requires an Alpine distrobox with musl-native C++ tools. Set up its
[prerequisites](../userspace/apps/snes9x/README.md) before `make userspace`;
the normal builder discovers this wrapper along with other apps.

All third-party application and library sources are pinned in `.gitmodules`;
see [ports.md](ports.md). Library wrappers build from the Git checkouts into
ignored `build/<commit>/` directories. They no longer download release tarballs.
Initialize submodules before building; `make` does not update source pins.

## Targets

| Target | Effect |
|---|---|
| `make` / `make all` | Build `twilight-os.iso` |
| `make kernel` | Build only the kernel |
| `make userspace` | Build libraries/apps and stage rootfs executables |
| `make initramfs` | Package init, oksh, and install into `initramfs.cpio` |
| `make system-image` | Pack `rootfs/` into `system.tfs` with `tools/mktfs` |
| `make all-hdd` | Build the GPT boot image `twilight-os.hdd` |
| `make run` | Boot the ISO with an IDE data disk and serial console |
| `make run-blk-bios` | Boot with VirtIO block, xHCI, and RTL8139 |
| `make run-bios` | Boot q35 BIOS; this recipe does not configure a NIC |
| `make run-x86_64-uefi` | Boot using OVMF firmware |
| `make run-hdd` | Boot the GPT HDD image |
| `make test-tty` | Run host terminal tests without QEMU |
| `make test-time` | Run the live-ISO timing matrix with no attached disk |
| `make test-net` | Run host checks and snapshot QEMU networking/IPC regression |
| `make clean` | Remove compiled/image outputs, preserving `hdd.img` |
| `make distclean` | Also remove downloaded Limine/OVMF assets |

There is no `make cpio` target. Use `make initramfs` for the bootstrap payload
and `make system-image` for base rootfs content, or rebuild the full ISO.

## Packaging

```text
kernel sources ---------------------> kernel/twilight_kernel/kernel
userspace apps + libraries ---------> rootfs/bin, rootfs/sbin, sysroot
  init + oksh + install ------------> initramfs.cpio (at most 4 MiB)
  complete rootfs ------------------> system.tfs
kernel + initramfs + system.tfs
  + Limine assets ------------------> twilight-os.iso / twilight-os.hdd
```

`hdd.img` is a separate guest data disk. The guest `install` command
initializes it for persistent TwilightFS storage; image-building targets do
not install onto it implicitly.

## Variables

- `KARCH`: architecture, default `x86_64`.
- `QEMUFLAGS`: extra flags used by some QEMU recipes; several x86_64 recipes
  specify their flags directly, so inspect the selected target.
- `RUSTUP_TOOLCHAIN`: optional kernel toolchain override; the Makefile selects
  a GNU host toolchain on supported musl hosts.
- `CC`, `TWILIGHT_NO_PIE`: userspace compiler and PIE policy; see
  [userspace.md](userspace.md).
- `SNES_DIR`, `SNES_BOX`: Snes9x source/build environment overrides.

Limine and required OVMF files are downloaded on demand. `make clean` retains
these assets; `make distclean` removes them.
