# Snes9x framebuffer build wrapper

This directory contains build integration. The Snes9x core and Twilight
`/dev/fb0` frontend are pinned in the `userspace/snes9x-fb` submodule from
[`akashKarmakar02/snes9x-fb`](https://github.com/akashKarmakar02/snes9x-fb).
A recursive checkout supplies the source:

```sh
git submodule update --init --recursive
```

The default layout is inside Twilight OS:

```text
userspace/
  apps/snes9x/Makefile
  snes9x-fb/twilight/Makefile
```

An Alpine distrobox named `alpine` must provide make, gcc/g++, and musl
development headers (`build-base`). For example, create it on the host:

```sh
distrobox create --name alpine --image docker.io/library/alpine:latest
distrobox enter alpine -- sudo apk add build-base
```

The wrapper invokes Alpine's native `gcc` and `g++` instead of the host's
`musl-gcc` environment. The output dynamically links musl and libstdc++;
those runtimes must be present in the guest rootfs.

Build through the normal app pipeline:

```sh
make userspace
```

For another source checkout or distrobox, export overrides first:

```sh
export SNES_DIR=/absolute/path/to/snes9x-fb/twilight
export SNES_BOX=alpine
make userspace
```

`make -C userspace/apps/snes9x` builds just the wrapper and leaves a local
`snes9x` executable. The normal userspace builder stages it into `rootfs/bin`.
Missing source or a failed container build is an error. Commit and push port
changes in the source repository before updating the OS gitlink; see
[ports.md](../../../docs/ports.md). ROM files are user-supplied local assets
and must not be committed.
