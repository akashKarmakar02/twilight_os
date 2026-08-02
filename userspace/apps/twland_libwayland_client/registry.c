/*
 * registry.c — bind the Wayland globals twland advertises.
 */
#include "registry.h"

#include <string.h>

static void registry_global(void *data, struct wl_registry *registry,
                            uint32_t name, const char *interface,
                            uint32_t version) {
	struct globals *g = data;

	if (strcmp(interface, "wl_compositor") == 0) {
		g->compositor = wl_registry_bind(registry, name,
		                                 &wl_compositor_interface, 1);
	} else if (strcmp(interface, "wl_shm") == 0) {
		g->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
	} else if (strcmp(interface, "xdg_wm_base") == 0) {
		g->xdg_wm_base = wl_registry_bind(registry, name,
		                                  &xdg_wm_base_interface, 1);
	} else if (strcmp(interface, "zwlr_layer_shell_v1") == 0) {
		uint32_t bind_version = version < 4 ? version : 4;
		g->layer_shell = wl_registry_bind(registry, name,
		                                  &zwlr_layer_shell_v1_interface,
		                                  bind_version);
	}
}

static const struct wl_registry_listener registry_listener = {
	.global = registry_global,
	.global_remove = NULL,
};

int globals_bind(struct wl_display *display, struct wl_registry *registry,
                struct globals *g) {
	memset(g, 0, sizeof(*g));
	if (wl_registry_add_listener(registry, &registry_listener, g) < 0) {
		return -1;
	}
	/* Round-trip so the .global callbacks fire before we return. */
	if (wl_display_roundtrip(display) < 0) {
		return -1;
	}
	return (g->compositor && g->shm && g->xdg_wm_base && g->layer_shell)
	           ? 0
	           : -1;
}
