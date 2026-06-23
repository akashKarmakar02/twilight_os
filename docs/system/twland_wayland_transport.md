# twland Wayland transport foundation

`twland` is Twilight's first Wayland-shaped userspace transport daemon. It is
not a full compositor yet: it does not implement xdg-shell, input, scene
management, GPU acceleration, or real desktop policy. This document describes
the base socket and registry transport. The first shared-memory rendering path
is documented separately in `docs/system/twland_shm_rendering.md`.

## Runtime socket

`twland` listens on:

```text
/run/user/0/wayland-0
```

On startup it creates:

```text
/run
/run/user
/run/user/0
```

and removes any stale `wayland-0` socket before binding a new AF_UNIX
`SOCK_STREAM` listener.

Future clients can use either:

```sh
WAYLAND_DISPLAY=/run/user/0/wayland-0
```

or the conventional pair:

```sh
XDG_RUNTIME_DIR=/run/user/0
WAYLAND_DISPLAY=wayland-0
```

The current test client hardcodes the full socket path.

## Supported protocol subset

`twland` parses the basic Wayland wire header:

```text
u32 object_id
u32 size_opcode
```

where the upper 16 bits of `size_opcode` are the message size and the lower
16 bits are the opcode.

Implemented requests:

```text
wl_display.sync
wl_display.get_registry
wl_registry.bind
wl_compositor.create_surface
wl_shm.create_pool
wl_shm_pool.create_buffer
wl_surface.attach
wl_surface.damage
wl_surface.commit
```

Implemented events:

```text
wl_callback.done
wl_registry.global
wl_shm.format
wl_buffer.release
```

`wl_registry.global` currently advertises:

```text
name=1 interface=wl_compositor version=4
name=2 interface=wl_shm        version=1
name=3 interface=wl_seat       version=5
name=4 interface=wl_output     version=3
```

When a client binds `wl_shm`, `twland` sends:

```text
WL_SHM_FORMAT_ARGB8888 = 0
WL_SHM_FORMAT_XRGB8888 = 1
```

## Service integration

The rootfs includes:

```text
/etc/twinit/services/twland.toml
```

with `stdout = "log"` and `stderr = "log"`, so normal daemon logs are forwarded
through `twilogd` and can be inspected with:

```sh
twilogctl show
```

Expected log lines include:

```text
source=twland message=twland: listening on /run/user/0/wayland-0
source=twland message=twland: client connected
source=twland message=twland: wl_display.get_registry new_id=2
```

## Test client

`twland_test_client` connects to `/run/user/0/wayland-0`, requests the registry,
prints all globals, binds `wl_shm`, reads the two format events, and exits with
`PASS`.

Expected output:

```text
twland_test_client: connected
twland_test_client: global wl_compositor version=4
twland_test_client: global wl_shm version=1
twland_test_client: global wl_seat version=5
twland_test_client: global wl_output version=3
twland_test_client: shm format ARGB8888
twland_test_client: shm format XRGB8888
twland_test_client: PASS
```

## Current limitations

- No xdg-shell or real surface roles.
- No input.
- No real compositor scene graph.
- No frame callbacks beyond minimal `wl_display.sync`.
- No multi-client policy.
- No security model.
- No GPU acceleration.

## Future stages

1. `xdg_wm_base`
2. `xdg_surface` / `xdg_toplevel`
3. configure / ack_configure
4. frame callbacks
5. Keyboard/mouse input
6. Real window placement
7. Window decorations and session shell
