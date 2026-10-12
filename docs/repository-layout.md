# Repository Layout

| Path | Purpose |
|---|---|
| `GNUmakefile` | Build, image, QEMU, and regression targets |
| `kernel/twilight_kernel/` | `no_std` kernel crate and kernel build rules |
| `kernel/twilight_common/` | Shared types and syscall ABI |
| `kernel/twilight_proc/` | Workspace procedural macros |
| `userspace/build.rs` | Library builds, app discovery, and rootfs staging |
| `userspace/apps/` | Native applications, port submodules, and build wrappers |
| `userspace/libs/` | libffi/Wayland wrappers and source submodules; native twigl |
| `userspace/doomgeneric/`, `userspace/snes9x-fb/` | Emulator source submodules used by app wrappers |
| `userspace/protocols/` | Shared Wayland protocol XML |
| `rootfs/` | Base filesystem content packed into `system.tfs` |
| `tools/mktfs/` | Host TwilightFS image builder |
| `tools/time-regression/` | QEMU timing regression matrix |
| `tools/net-regression/` | Snapshot QEMU networking/IPC regression |
| `tools/tty-regression/` | Host tests using the production terminal renderer |
| `docs/` | Subsystem documentation and screenshots |
| `limine.conf` | Bootloader configuration |
| `limine/`, `ovmf/` | Downloaded boot assets |

The kernel's owning directories are `arch/x86_64/`, `driver/`,
`compat/freebsd_kpi/`, `sys/`, `task/`, `utils/`, and `kernel_utils/` under
`kernel/twilight_kernel/src/`. Register new modules in their subsystem's
`mod.rs`; see [AGENTS.md](../AGENTS.md).

Git submodules cover TinyCC, vi, oksh, Doomgeneric, Smolnes, Snes9x, libffi,
and Wayland. Initialize them with `git submodule update --init --recursive`.
See [ports.md](ports.md) for source paths, remotes, and the update workflow.
Snes9x source is inside this checkout; its [build wrapper](../userspace/apps/snes9x/README.md)
still requires the Alpine build environment.

Generated outputs include `kernel/target/`, `userspace/target/`,
`kernel/twilight_kernel/kernel`, `rootfs/bin/`, `rootfs/sbin/`,
`build/initramfs/`, `initramfs.cpio`, `system.tfs`, `iso_root/`,
`twilight-os.iso`, `twilight-os.hdd`, `hdd.img`, and `qemu.log`.
The bootstrap initramfs contains the small installer/live-shell payload;
remaining rootfs content lives in `system.tfs`.

Some runtime libraries and sysroot files under `rootfs/usr/` are versioned.
Inspect library rebuilds separately from source changes. Generated app
executables, caches, local packages, ROMs, firmware, and game data do not
belong in commits.
