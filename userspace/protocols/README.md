# Vendored Wayland protocols

Protocol XML files in this directory are shared inputs for `wayland-scanner`.
Keep generated headers and C sources in each consuming application's build
directory; do not commit generated output here.

Consumers include `../libs/wayland/scanner.mk` to select the native scanner
built from the pinned Wayland source. Standalone app builds prepare it when
missing; `WAYLAND_SCANNER` may override the executable. See
[ports.md](../../docs/ports.md) for host tool prerequisites.

| File | Upstream | Git blob |
| --- | --- | --- |
| `xdg-shell.xml` | `wayland-protocols/stable/xdg-shell` | `f8b78eea7bf3bd686753befd30e83f757326aed0` |
| `wlr-layer-shell-unstable-v1.xml` | `swaywm/wlr-protocols/unstable` | `d62fd51e90d18247a5b8097395c3291b7099a380` |
