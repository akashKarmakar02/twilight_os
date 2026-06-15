# Userspace

Userspace apps live in `userspace/apps/`. The `userspace` crate contains a build script that discovers each app directory, builds it, and copies the resulting executables into `rootfs/bin`.

## Build behavior
The build script detects projects in this order:
1. Rust (`Cargo.toml`)
2. Zig (`build.zig` or `zig.toml`)
3. Makefile (`Makefile` or `makefile`)
4. Plain C/C++ files in the directory

Executables are copied into `rootfs/bin` and marked executable.

## Environment variables
- `CC`: overrides the C compiler used for Makefile and C builds (defaults to `musl-gcc`).
- `TWILIGHT_NO_PIE`: set to `0` to allow PIE; by default, PIE is disabled for static userland binaries.

## Bundled apps
`userspace/apps` currently contains:
`bc`, `imgview`, `cat`, `chip8`, `clear`, `cp`, `curl`, `date`, `diskbench`, `echo`, `fbdoom`, `grep`, `head`, `hello`, `httpd`, `init`, `iotest`, `logind`, `ls`, `mkdir`, `poweroff`, `reboot`, `rm`, `rmdir`, `sleep`, `tail`, `tcc`, `touch`, `tpy`, `tsh`, `twifetch`, `uname`, `vi`, `wc`.

## Doom

The complete doomgeneric source is vendored in `userspace/doomgeneric`.
The Twilight framebuffer and input port is built through
`userspace/apps/fbdoom/Makefile`, so the normal userspace build installs
`fbdoom` into `rootfs/bin`:

```bash
make userspace
```

After booting Twilight OS, run:

```sh
fbdoom -iwad /doom.wad
```

See `userspace/doomgeneric/TWILIGHT.md` for upstream provenance, port details,
and standalone build instructions.

## Adding a new app
1. Create a new directory under `userspace/apps/<name>`.
2. Add a `Makefile` or `Cargo.toml` (or a simple `*.c` file).
3. Build with `make userspace` or `make` at the repo root.
4. The resulting binary should appear in `rootfs/bin`.
