# Repository Layout

- `twilight_kernel/` - Rust kernel crate and build system.
- `twilight_common/` - shared code used by kernel/userspace.
- `twilight_proc/` - procedural macros used in the workspace.
- `userspace/` - userspace build pipeline and app sources.
- `userspace/apps/` - individual user programs (each app has its own directory).
- `rootfs/` - base root filesystem content (packed into `rootfs.cpio`).
- `docs/` - project documentation and screenshots.
- `limine.conf` - Limine bootloader configuration.
- `limine/` - Limine bootloader (downloaded by Makefile).
- `ovmf/` - UEFI firmware images (downloaded by Makefile).
- `GNUmakefile` - top-level build and run targets.

Generated artifacts (not committed):
- `twilight-os.iso`, `twilight-os.hdd`, `rootfs.cpio`, `hdd.img`, `qemu.log`, `target/`
