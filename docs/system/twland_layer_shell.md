# twland layer-shell support

`twland` implements version 4 of `wlr-layer-shell-unstable-v1` for desktop
components such as wallpapers, panels, launchers, notifications, and lock
screens. The protocol XML is vendored once under `userspace/protocols/`; demo
client bindings are generated from that shared source at build time.

## Code organization

The implementation keeps wire parsing, protocol state, and placement policy
separate:

```text
userspace/apps/twland/src/shell/
├── mod.rs
└── layer/
    ├── mod.rs       protocol lifecycle and surface collection
    ├── types.rs     validated protocol values and state types
    ├── layout.rs    pure placement and exclusive-zone calculations
    └── wire.rs      request decoding and event encoding
```

Rendering and input dispatch stay in the compositor because they combine
layer surfaces with xdg toplevels, the cursor, seats, and framebuffer state.
This mirrors Smithay's broad boundary between its layer-shell protocol module
and desktop layer-map policy without importing Smithay's larger abstractions.

## Lifecycle

A layer client must use the protocol-defined configure handshake:

```text
create wl_surface
get_layer_surface
set size/anchors and other pending state
commit without a buffer
receive configure(serial, width, height)
ack_configure(serial)
attach a buffer and commit to map
```

Layer properties are double-buffered and take effect on `wl_surface.commit`.
Configure acknowledgements are tracked by send order so serial wraparound is
safe. Attaching a null buffer unmaps the surface and resets it to its initial
layer-shell state; remapping requires a fresh configure/ack cycle.

The generic `wl_surface` state represents a pending attach as a nested option.
That distinction is intentional: no attach request retains the current buffer,
while an explicit null attach removes it and triggers shell-specific unmapping.

## Composition and layout

The scene is rendered in this order:

```text
background → bottom → xdg toplevels → top → overlay → cursor
```

Anchors, margins, client-selected committed sizes, and positive, zero, or
negative exclusive zones are handled by the layout module. Positive exclusive
zones reserve usable output space only when the anchor combination identifies
one effective edge. Xdg windows are constrained to the resulting usable area.

Top and overlay surfaces requesting exclusive keyboard interactivity override
normal xdg focus. Other interactive layer surfaces use click-to-focus and keep
focus until the user selects another eligible surface.

## Demo client

`userspace/apps/twland_libwayland_client` generates bindings with
`wayland-scanner`, maps a background layer surface, and then maps an xdg
toplevel above it. It exercises registry binding, initial configure/ack, shm
buffer attachment, and the cross-shell composition order.

## Native clock

`userspace/apps/twclock` is a small toolkit-free clock built on the same
generated layer-shell bindings. It keeps registry/layer lifecycle, shm buffer
ownership, and software drawing in separate modules. The clock uses the bottom
layer, is anchored to the top-right corner, and updates through release-aware
double buffering. Its client-rendered close button uses `wl_seat`/`wl_pointer`
events and returns control to the launching shell. See its local README for
build and usage instructions.

## Current limitations

- One compositor client and one physical output.
- `get_popup` is not implemented because `twland` does not yet implement
  `xdg_popup`.
- Cairo/Pango and production desktop widgets are separate client-side work;
  this change provides the compositor protocol they require.
