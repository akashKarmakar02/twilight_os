# twland shared-memory software rendering

This stage adds the first visible Wayland-style rendering path to Twilight OS.
It is still a debug compositor path, not a full desktop compositor.

`twland` accepts shared-memory buffers from clients and blits committed pixels
into `/dev/fb0`. This document describes the original shm rendering foundation;
the current preferred visible path is the xdg-shell flow documented in
`docs/system/twland_xdg_shell.md`.

## Implemented objects

The minimal object model includes:

```text
wl_display
wl_registry
wl_callback
wl_compositor
wl_shm
wl_shm_pool
wl_buffer
wl_surface
wl_seat
wl_output
```

Implemented requests:

```text
wl_compositor.create_surface
wl_shm.create_pool
wl_shm_pool.create_buffer
wl_shm_pool.destroy
wl_buffer.destroy
wl_surface.attach
wl_surface.damage
wl_surface.frame
wl_surface.commit
```

Implemented events:

```text
wl_buffer.release
wl_shm.format
wl_registry.global
wl_callback.done
```

## Roleless-surface debug behavior

Real Wayland clients normally need a shell role, such as `xdg_surface` and
`xdg_toplevel`, before a surface is mapped.

The original shm milestone used this temporary debug behavior:

```text
roleless wl_surface.commit -> direct framebuffer blit
```

That path is intentionally not fully Wayland-compliant and is now disabled by
default in `twland` with:

```rust
TWLAND_ALLOW_ROLELESS_DEBUG_SURFACES = false
```

Use `twland_xdg_client` for the current visible rendering test. The old
`twland_shm_client` remains useful as a protocol/debug reference if the constant
is temporarily flipped back on during local experiments.

## wl_shm flow

The rendering test uses the normal Wayland-style shared memory shape:

```text
client creates memfd
client ftruncate(size)
client mmap(PROT_READ | PROT_WRITE, MAP_SHARED)
client draws pixels
client sends wl_shm.create_pool with fd via SCM_RIGHTS
twland mmap(PROT_READ, MAP_SHARED)
client creates wl_buffer from pool
client creates an xdg toplevel surface
client waits for configure / sends ack_configure
client attaches buffer to surface
client damages and commits surface
twland blits to /dev/fb0
twland sends wl_buffer.release
```

Supported buffer formats:

```text
WL_SHM_FORMAT_ARGB8888 = 0
WL_SHM_FORMAT_XRGB8888 = 1
```

Alpha is ignored in this first software blitter.

## Framebuffer path

`twland` opens:

```text
/dev/fb0
```

It queries framebuffer geometry with:

```text
FBIOGET_VSCREENINFO
FBIOGET_FSCREENINFO
```

Then it maps the framebuffer using:

```text
mmap(PROT_READ | PROT_WRITE, MAP_SHARED)
```

On startup it clears the screen to a dark color. On every committed shm buffer,
it clips the damaged rectangle to the buffer and screen bounds, copies rows into
the framebuffer, then calls:

```text
FBIOPAN_DISPLAY
```

to sync the framebuffer.

## Legacy shm test client

`twland_shm_client` draws a 200x120 test rectangle:

- red fill
- green border
- blue diagonal

With roleless debug surfaces enabled, expected output is:

```text
twland_shm_client: connected
twland_shm_client: globals received
twland_shm_client: shm pool created
twland_shm_client: buffer created
twland_shm_client: surface created
twland_shm_client: committed buffer
twland_shm_client: buffer released
twland_shm_client: PASS
```

Expected visual result:

```text
a colored rectangle appears near the top-left of the framebuffer
```

For the default build, use `twland_xdg_client` instead; it follows the
xdg-shell configure/ack/commit sequence and should draw a 400x300 test window.

## Logs

`twland` runs as a `twinit` service with:

```toml
stdout = "log"
stderr = "log"
```

Inspect logs with:

```sh
twilogctl show
```

Expected lines include:

```text
source=twland message=twland: wl_shm.create_pool ...
source=twland message=twland: wl_surface.commit ...
source=twland message=twland: blit ...
```

## Current limitations

- xdg-shell support is minimal and documented separately.
- No real desktop shell policy or decorations.
- No keyboard or pointer input.
- No frame scheduling.
- No damage optimization beyond basic clipping.
- No multi-window compositor policy.
- No GPU acceleration.
- No real client compatibility yet.
- Shared memory pool lifetime is minimal and oriented around this debug client.

## Future stages

1. keyboard/pointer input
2. real focus tracking
3. real window placement
4. decorations/session shell
5. resize/maximize/fullscreen
6. frame scheduling
