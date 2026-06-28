# twland window management foundation

This stage adds the first compositor-owned window-management layer to `twland`.

It is still intentionally small. There is no full desktop shell, no resize, no
xdg-decoration protocol, and no GPU acceleration. The goal is to prove that
`twland` can keep a scene graph, draw multiple xdg toplevels, route input
through compositor hit testing, and redraw the framebuffer after focus, move, or
close changes.

## Window model

`twland` now keeps a compositor-side window list:

```text
Window {
  surface_id
  x, y
  width, height
  mapped
  focused
  title
  app_id
  decoration
}
```

The list is also the z-order:

```text
bottom -> windows[0]
top    -> windows[last]
```

When a window is focused, it is raised to the end of the list.

## Mapping xdg_toplevels

When a configured `xdg_toplevel` commits its first buffer, `twland` creates or
updates a `Window` entry, copies the title and app id from xdg state, enables
debug decorations, and redraws the entire scene.

Expected log:

```text
twland: mapped window surface=... title="..." app_id="..." pos=... size=...
```

## Redraw path

After commits, focus changes, drag moves, and unmaps, `twland` uses a simple
full-screen redraw:

```text
clear desktop background
for window in z-order:
  draw decoration
  blit client shm buffer into content area
sync framebuffer
```

This is deliberately simple and easy to reason about. Damage tracking can come
later.

## Server-side debug decorations

`twland` draws temporary compositor-owned decorations:

```text
+--------------------------------+
| title                     [ X ] |
+--------------------------------+
| client buffer                  |
|                                |
+--------------------------------+
```

Metrics:

```text
titlebar height = 24
border width    = 2
close button    = 18x18
```

Focused windows get a brighter titlebar and border. Unfocused windows get a
darker titlebar and softer border.

These are not the `xdg-decoration` protocol. They are debug server-side
decorations to make the first window-management layer visible and testable.

## Hit testing

The compositor hit-tests from topmost to bottommost window.

Hit results:

```text
None
ClientArea(surface)
Titlebar(surface)
CloseButton(surface)
Border(surface)
```

Close button wins over titlebar. Titlebar wins over client area. Border is
currently a placeholder for future resize.

Pointer events over the client area are forwarded to the client with coordinates
relative to the content area. Pointer events over decorations are handled by the
compositor and are not sent to the client.

## Focus model

Clicking a window focuses it and raises it.

On focus changes, `twland`:

1. updates the focused window,
2. raises it to the top of z-order,
3. sends keyboard focus events,
4. sends xdg configure events with activated state for the new focused window,
5. sends xdg configure events without activated state for the old focused
   window,
6. redraws the scene.

Expected logs:

```text
twland: focus changed old=... new=...
twland: raised window surface=...
```

## Titlebar dragging

Pointer button press on a titlebar starts a compositor drag. Pointer motion
while dragging updates the window position and redraws the scene. Button release
ends the drag.

Expected logs:

```text
twland: begin drag surface=...
twland: drag move surface=... pos=...
twland: end drag surface=...
```

## Close button

Clicking the close button sends:

```text
xdg_toplevel.close
```

to the client. `twland` does not kill the client. The client is expected to
destroy or unmap the window.

Expected log:

```text
twland: close requested surface=...
```

When the client destroys the window, `twland` removes it from the window list and
redraws the scene.

## Test client

`twland_window_client` creates two xdg toplevel windows:

```text
Twilight Window A
Twilight Window B
```

Each window uses a different `wl_shm` color buffer. The client binds
`wl_seat`, creates pointer and keyboard objects, handles focus configure events,
and handles `xdg_toplevel.close` by destroying the requested window.

Expected output:

```text
twland_window_client: connected
twland_window_client: globals received
twland_window_client: window A mapped
twland_window_client: window B mapped
twland_window_client: pointer event window=focused
twland_window_client: keyboard event window=focused
twland_window_client: close requested window=A
twland_window_client: PASS
```

## Manual verification

```sh
twinitctl status
twland_window_client
twilogctl show
```

Expected visual result:

```text
two decorated windows are visible
focused window has brighter decoration
focused window is raised
titlebar drag moves a window
close button sends close to the client
remaining window redraws correctly
```

## Current limitations

- Decorations are compositor debug decorations, not `xdg-decoration`.
- No resize yet.
- No maximize/fullscreen behavior yet.
- No minimize/taskbar.
- No cursor surfaces.
- Input source is still the simple Twilight/synthetic bridge from the input
  foundation stage.
- No animations.
- No GPU acceleration.
- No compositor security policy.
- No clipboard.
- No drag-and-drop.

## Future stages

1. Resize via borders.
2. Maximize/fullscreen configure states.
3. Cursor surface support.
4. Real Twilight input event backend.
5. `xdg-decoration` protocol.
6. Keyboard shortcuts.
7. Task switcher.
8. Clipboard/data-device.
9. Better damage tracking.
10. Double buffering / tearing reduction.
