# Snes9x framebuffer build wrapper

This directory contains build integration only. The Snes9x core and Twilight
`/dev/fb0` frontend live in the external
[`akashKarmakar02/snes9x-fb`](https://github.com/akashKarmakar02/snes9x-fb)
repository. The source is not a submodule and is not downloaded automatically.

The default layout is:

```text
projects/
  twilight_os/userspace/apps/snes9x/Makefile
  snes9x-fb/twilight/Makefile
```

From `twilight_os`, prepare the sibling checkout:

```sh
git clone https://github.com/akashKarmakar02/snes9x-fb.git ../snes9x-fb
```

An existing Alpine distrobox named `alpine` must provide make, gcc/g++, and
musl development headers (Alpine's `build-base` package). The wrapper invokes
its native `gcc` and `g++` instead of the host's `musl-gcc` environment.
The resulting binary is dynamically linked against musl and libstdc++; those
runtimes must be present in the guest rootfs.

Build through the normal app pipeline:

```sh
make userspace
```

For another source location or distrobox, export the overrides first:

```sh
export SNES_DIR=/absolute/path/to/snes9x-fb/twilight
export SNES_BOX=alpine
make userspace
```

`make -C userspace/apps/snes9x` builds just the wrapper and leaves a local
`snes9x` executable. The normal userspace builder stages it into `rootfs/bin`.
Missing source or a failed container build is an error, not a successful skip.
ROM files are user-supplied local assets; do not commit them.
