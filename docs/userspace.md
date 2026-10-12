# Userspace

Userspace apps live in `userspace/apps/`. The `userspace` crate contains a build script that discovers each app directory, builds it, and copies the resulting executables into `rootfs/bin`.

## Build behavior
The build script first builds libraries in `userspace/libs/`, then detects apps
in this order:

1. Makefile (`Makefile` or `makefile`)
2. Rust (`Cargo.toml`)
3. Zig (`build.zig` or `zig.toml`)
4. Plain C/C++ files in the directory

Executables are copied into `rootfs/bin` and marked executable.
Standalone oksh and vi submodules are built afterward. Service-manager and
logging binaries also receive generated `/sbin` copies. Most programs link
statically; Wayland clients and Snes9x can use shared sysroot libraries.

## Environment variables
- `CC`: overrides the C compiler used for Makefile and C builds (defaults to `musl-gcc`).
- `TWILIGHT_NO_PIE`: set to `0` to allow PIE; by default, PIE is disabled for static userland binaries.
- `SNES_DIR`, `SNES_BOX`: Snes9x source directory override and Alpine distrobox name.

## Bundled apps
Highlights include `tcc`, `oksh`, `vi`, core command-line utilities, `curl`,
`httpd`, `twinit`, `twinitctl`, `twilogd`, `twilogctl`, `twland`, `twclock`,
`smolnes`, `chip8`, and `fbdoom`. Guest probes cover timing, storage, IPC,
shared memory, polling, networking, and Wayland. Browse `userspace/apps/` for
the full list; oksh and vi also have standalone submodule build paths.

## Snes9x port

`userspace/apps/snes9x/Makefile` delegates to the pinned
`userspace/snes9x-fb/twilight` source submodule. It requires an Alpine distrobox
with musl-native C++ tools; the wrapper is discovered by the ordinary userspace
build. Initialize submodules and follow the
[wrapper README](../userspace/apps/snes9x/README.md) before building.

## Smolnes

The `userspace/apps/smolnes` source submodule uses accumulated absolute monotonic deadlines
for frame pacing. Run `make -C userspace/apps/smolnes test` for its host pacing
tests. Current `master` uses keyboard input; the generic gamepad work is
described separately in [usb/gamepads.md](usb/gamepads.md).

ROMs and Doom WADs are user-supplied local assets and must not be committed.

## Known build limitation

The plain-C path currently stages only executable paths absent before the
build. Rebuilding a binary already present in its app directory can therefore
leave `rootfs/bin` stale; this is tracked in
[#89](https://github.com/akashKarmakar02/twilight_os/issues/89). A successful
compiler invocation alone does not prove the staged copy was refreshed.

## Doom

The Doom port source is pinned as the `userspace/doomgeneric` submodule.
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

## Third-party sources

All application ports and downloaded library sources are pinned submodules;
see [ports.md](ports.md) for the inventory and update workflow. The libffi and
Wayland wrappers build their `upstream/` submodules rather than downloading
tarballs. Their objects and staging trees use `build/<commit>/`, so a changed
pin selects a fresh configuration. Autoconf, Automake, and Libtool bootstrap
libffi; the cross-build disables guest documentation. Twilight's own apps and
twigl remain ordinary source directories.

## Adding a new app
1. Create a new directory under `userspace/apps/<name>`.
2. Add a `Makefile` or `Cargo.toml` (or a simple `*.c` file).
3. Build with `make userspace` or `make` at the repo root.
4. The resulting binary should appear in `rootfs/bin`.
