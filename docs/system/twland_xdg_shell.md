# twland minimal xdg-shell support

This stage adds the first xdg-shell-shaped window flow to `twland`.

It replaces the temporary roleless-surface drawing path with the normal
Wayland order:

```text
wl_compositor.create_surface
xdg_wm_base.get_xdg_surface
xdg_surface.get_toplevel
xdg_toplevel.set_title
xdg_toplevel.set_app_id
initial wl_surface.commit with no buffer
xdg_toplevel.configure
xdg_surface.configure
xdg_surface.ack_configure
wl_surface.attach
wl_surface.damage
wl_surface.frame
wl_surface.commit
wl_buffer.release
wl_callback.done
```

This is not a complete desktop shell yet. It is just enough protocol to create
one configured toplevel and draw it through the existing `wl_shm` software
framebuffer path.

## Registry global

`twland` advertises:

```text
name=5 interface=xdg_wm_base version=6
```

alongside the existing `wl_compositor`, `wl_shm`, `wl_seat`, and `wl_output`
globals.

## xdg objects

Implemented object types:

```text
xdg_wm_base
xdg_surface
xdg_toplevel
```

Implemented requests:

```text
xdg_wm_base.pong
xdg_wm_base.get_xdg_surface
xdg_surface.get_toplevel
xdg_surface.ack_configure
xdg_toplevel.set_title
xdg_toplevel.set_app_id
```

Other `xdg_toplevel` requests are logged and ignored for now.

Implemented events:

```text
xdg_toplevel.configure
xdg_surface.configure
```

`xdg_toplevel.configure` currently uses a default size of `400x300` and sends
the `activated` state.

## Configure / ack_configure

When a client commits an xdg surface with no buffer for the first time,
`twland` sends:

```text
xdg_toplevel.configure(width=400, height=300)
xdg_surface.configure(serial)
```

The client must reply with:

```text
xdg_surface.ack_configure(serial)
```

before attaching and committing a buffer. If a client commits a buffer before
acknowledging the configure serial, `twland` treats it as a protocol error.

## Mapping and placement

After `ack_configure`, a committed shm buffer maps the toplevel and is blitted
to `/dev/fb0`.

Current placement is simple cascade placement:

```text
first window  -> x=60,  y=60
second window -> x=100, y=100
third window  -> x=140, y=140
```

There is no real window manager policy yet.

## Frame callbacks

`wl_surface.frame(new_callback_id)` is supported minimally.

After a successful commit/blit, `twland` sends:

```text
wl_callback.done
```

and removes the callback object.

## Test client

`twland_xdg_client` exercises the complete minimal flow:

```text
connect
get registry
bind wl_compositor
bind wl_shm
bind xdg_wm_base
create wl_surface
create xdg_surface
create xdg_toplevel
set title/app_id
initial empty commit
wait configure
ack configure
create memfd shm buffer
attach/damage/frame/commit
wait buffer release and frame callback
```

Expected output:

```text
twland_xdg_client: connected
twland_xdg_client: globals received
twland_xdg_client: xdg_wm_base bound
twland_xdg_client: surface created
twland_xdg_client: xdg_surface created
twland_xdg_client: xdg_toplevel created
twland_xdg_client: initial empty commit sent
twland_xdg_client: configure received
twland_xdg_client: ack_configure sent
twland_xdg_client: buffer committed
twland_xdg_client: buffer released
twland_xdg_client: frame callback done
twland_xdg_client: PASS
```

Expected visual result:

```text
a 400x300 colored test window appears around x=60, y=60
```

## Logs

Inspect compositor logs with:

```sh
twilogctl show
```

Expected log lines include:

```text
source=twland message=twland: xdg_wm_base.get_xdg_surface ...
source=twland message=twland: xdg_surface.get_toplevel ...
source=twland message=twland: sent xdg_surface.configure ...
source=twland message=twland: xdg_surface.ack_configure ...
source=twland message=twland: mapped xdg_toplevel ...
```

## Current limitations

- No decorations.
- No real window manager policy.
- No resize protocol behavior.
- No maximize/fullscreen behavior.
- No popup support.
- No input.
- No focus.
- No real frame scheduling.
- No multi-client security.
- No GPU acceleration.

## Future stages

1. Keyboard/pointer input
2. Seat capabilities
3. Pointer motion/button events
4. Keyboard key events
5. Focus tracking
6. Simple window dragging
7. Close button/decorations
8. Multi-window scene graph
