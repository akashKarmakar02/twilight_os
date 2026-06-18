# doomgeneric for Twilight OS

This directory vendors [ozkl/doomgeneric](https://github.com/ozkl/doomgeneric)
at commit `dcb7a8dbc7a16ce3dda29382ac9aae9d77d21284`.

The upstream license is preserved in `LICENSE`. Generated objects and
executables are intentionally excluded from Git.

## Twilight changes

- `doomgeneric/doomgeneric_twilight.c` implements the `/dev/fb0`,
  `/dev/input/event0`, and `/dev/input/mice` backend.
- `doomgeneric/Makefile.twilight` builds a static `fbdoom` executable.
- `doomgeneric/doomgeneric.h` exposes mouse input to the generic input layer.
- `doomgeneric/i_input.c` posts Twilight keyboard and mouse events.
- `doomgeneric/i_system.c` raises the default and minimum Doom zone memory.

## Build

Build and install all userspace programs, including `fbdoom`:

```bash
make userspace
```

Build only Doom:

```bash
make -C userspace/apps/fbdoom
```

The standalone build produces `userspace/apps/fbdoom/fbdoom`. The normal
userspace builder copies that executable to `rootfs/bin/fbdoom`.

## Run

Build the OS image and launch QEMU:

```bash
make
make run
```

Inside Twilight OS:

```sh
fbdoom -iwad /doom.wad
```

Doom game data is not part of doomgeneric. A compatible IWAD must be present
in the filesystem image.

## Updating upstream

When importing a newer doomgeneric revision:

1. Replace the vendored upstream files while keeping `LICENSE`.
2. Reapply the Twilight files and changes listed above.
3. Update the pinned commit in this document.
4. Run `make -C userspace/apps/fbdoom clean all`.
