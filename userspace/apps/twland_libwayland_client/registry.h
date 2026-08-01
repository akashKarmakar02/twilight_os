/*
 * registry.h — bind the Wayland globals twland advertises.
 *
 * Part of twland_libwayland_client, a demo that does the same xdg-shell +
 * wl_shm window as twland_xdg_client but through the official libwayland-client
 * instead of hand-rolled wire codec — the proof-of-concept that issue #57's
 * cross-built libwayland-client.so actually works under twland.
 */
#ifndef REGISTRY_H
#define REGISTRY_H

#include <wayland-client.h>
#include "xdg-shell-client-protocol.h"

/* The single bound globals a wayland client needs to map a window. */
struct globals {
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct xdg_wm_base *xdg_wm_base;
};

/*
 * Round-trip the registry and bind wl_compositor, wl_shm and xdg_wm_base.
 * Returns 0 on success, -1 if a required global was never advertised.
 */
int globals_bind(struct wl_display *display, struct wl_registry *registry,
                struct globals *g);

#endif /* REGISTRY_H */
