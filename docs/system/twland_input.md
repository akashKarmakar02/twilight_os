# twland input foundation

This stage adds the first Wayland-style input path to `twland`.

It is not a full desktop input stack yet. The goal is to prove that a client can
bind `wl_seat`, create pointer and keyboard objects, receive focus, and receive
basic pointer/keyboard events.

## Protocol objects

`twland` already advertises:

```text
wl_seat
```

When a client binds the seat, `twland` sends:

```text
wl_seat.capabilities(pointer | keyboard)
wl_seat.name("seat0")
```

Supported requests:

```text
wl_seat.get_pointer
wl_seat.get_keyboard
```

Created objects:

```text
wl_pointer
wl_keyboard
```

## Pointer support

Implemented events:

```text
wl_pointer.enter(serial, surface, surface_x, surface_y)
wl_pointer.leave(serial, surface)
wl_pointer.motion(time, surface_x, surface_y)
wl_pointer.button(serial, time, button, state)
```

Button state values:

```text
released = 0
pressed  = 1
```

The first test button is Linux/evdev-style `BTN_LEFT = 0x110`.

## Keyboard support

Implemented events:

```text
wl_keyboard.keymap
wl_keyboard.enter(serial, surface, keys)
wl_keyboard.leave(serial, surface)
wl_keyboard.key(serial, time, key, state)
```

For now `twland` sends:

```text
WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP
```

No xkb keymap fd is provided yet. Key events use raw Linux/evdev-style keycodes
where possible. The first test key is:

```text
KEY_SPACE = 57
```

There is no text composition, key repeat, keyboard layout, or IME support yet.

## Focus model

The current focus policy is intentionally tiny:

```text
pointer focus follows pointer position
keyboard focus follows pointer focus
```

Hit testing is rectangle based. The topmost surface is currently the mapped
surface with the highest object id under the pointer. This is enough for the
first one-window tests but is not a real scene graph or window manager policy.

## Input event source

`twland` has an internal event abstraction:

```text
PointerMove
PointerAbsolute
PointerButton
Key
```

The first implementation uses a deterministic synthetic event source once a
client has:

1. bound `wl_seat`,
2. created `wl_pointer`,
3. created `wl_keyboard`,
4. mapped a surface.

That synthetic source emits:

```text
pointer enter
pointer motion
left button press/release
keyboard enter
space key press/release
```

This keeps the protocol path testable before Twilight has a stable userspace
keyboard/mouse event device API for compositors. The same abstraction is the
future integration point for `/dev/input/mice`, keyboard events, or a dedicated
Twilight input event device.

## Test client

`twland_input_client` exercises the input path:

```text
connect
get registry
bind wl_compositor
bind wl_shm
bind xdg_wm_base
bind wl_seat
receive seat capabilities
get pointer
get keyboard
create xdg toplevel
create shm buffer
commit window
receive pointer and keyboard events
repaint on button/key press
PASS
```

Expected output:

```text
twland_input_client: connected
twland_input_client: globals received
twland_input_client: seat capabilities pointer keyboard
twland_input_client: pointer created
twland_input_client: keyboard created
twland_input_client: window mapped
twland_input_client: pointer enter
twland_input_client: pointer motion
twland_input_client: keyboard enter
twland_input_client: keyboard key
twland_input_client: PASS
```

## Logs

Inspect compositor logs with:

```sh
twilogctl show
```

Expected lines include:

```text
source=twland message=twland: wl_seat.get_pointer ...
source=twland message=twland: wl_seat.get_keyboard ...
source=twland message=twland: pointer enter ...
source=twland message=twland: keyboard enter ...
```

## Current limitations

- Synthetic input source only.
- No touch input.
- No key repeat.
- No xkb-compatible keymap fd yet.
- No text input or IME.
- No cursor surfaces.
- No software cursor drawing yet.
- No decorations.
- No click-to-focus.
- No window dragging.
- No resize.
- No compositor security policy.

## Future stages

1. Real Twilight input device bridge.
2. Cursor surface support.
3. Click-to-focus.
4. Simple window dragging.
5. Server-side decorations.
6. Close button.
7. Resize/maximize/fullscreen.
8. xkb-compatible keymap.
9. Text input protocol.
