/*
 * registry.h — bind the Wayland globals twland advertises.
 *
 * Part of twland_libwayland_client, which exercises layer-shell and xdg-shell
 * through the official libwayland-client instead of a hand-rolled wire codec.
 */
#ifndef REGISTRY_H
#define REGISTRY_H

#include <wayland-client.h>
#include "wlr-layer-shell-unstable-v1-client-protocol.h"
#include "xdg-shell-client-protocol.h"

/* The single bound globals a wayland client needs to map a window. */
struct globals {
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *xdg_wm_base;
	struct zwlr_layer_shell_v1 *layer_shell;
};

/*
 * Round-trip the registry and bind the compositor, shm and shell globals.
 * Returns 0 on success, -1 if a required global was never advertised.
 */
int globals_bind(struct wl_display *display, struct wl_registry *registry,
                struct globals *g);

#endif /* REGISTRY_H */
