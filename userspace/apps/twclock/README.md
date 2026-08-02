# twclock

`twclock` is a small native Wayland clock for Twilight OS. It is intentionally
not the upstream Cairo-based `wlclock`: this app exercises Twilight's own
`wl_shm` and `zwlr_layer_shell_v1` stack without introducing a graphics
toolkit dependency.

## Structure

```text
main.c          clock update loop and buffer selection
wayland_app.c   registry binding and layer-surface lifecycle
pointer_input.c seat/pointer lifecycle and click events
shm_buffer.c    reusable, release-aware shared-memory buffers
clock_face.c    software drawing and the built-in 5x7 digit font
app_config.h    widget size, margins, and buffer count
```

The renderer and Wayland transport do not know about each other's internal
state. `main.c` connects them by drawing only into a released buffer and then
committing that buffer to the layer surface.

## Behavior

- Bottom layer, below normal xdg windows.
- Anchored to the top-right output corner with a 24-pixel margin.
- Does not reserve desktop space or request keyboard focus.
- Provides an in-surface close button; `Ctrl+C` and `SIGTERM` also exit cleanly.
- Uses two XRGB8888 shm buffers and waits for `wl_buffer.release` before reuse.
- Updates once per wall-clock second and uses `/etc/localtime` when available.

## Build and run

Build the app directly:

```sh
make -C userspace/apps/twclock
```

The regular userspace build discovers the app and installs it into
`rootfs/bin`:

```sh
make userspace
```

Inside Twilight OS, run:

```sh
twclock
```

`XDG_RUNTIME_DIR` defaults to `/run/user/0` when it is not already set.
