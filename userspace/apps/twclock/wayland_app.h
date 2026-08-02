#ifndef TWCLOCK_WAYLAND_APP_H
#define TWCLOCK_WAYLAND_APP_H

#include <stdbool.h>
#include <stdint.h>
#include <wayland-client.h>

#include "pointer_input.h"

struct zwlr_layer_shell_v1;
struct zwlr_layer_surface_v1;

struct wayland_app {
	struct wl_display *display;
	struct wl_registry *registry;
	struct wl_compositor *compositor;
	struct wl_shm *shm;
	struct zwlr_layer_shell_v1 *layer_shell;
	struct pointer_input pointer_input;
	struct wl_surface *surface;
	struct zwlr_layer_surface_v1 *layer_surface;
	uint32_t layer_shell_version;
	uint32_t width;
	uint32_t height;
	bool configured;
	bool closed;
};

int wayland_app_init(struct wayland_app *app, uint32_t width, uint32_t height);
int wayland_app_wait_until_configured(struct wayland_app *app);
int wayland_app_dispatch(struct wayland_app *app, int timeout_ms);
int wayland_app_present(struct wayland_app *app, struct wl_buffer *buffer);
void wayland_app_destroy(struct wayland_app *app);

#endif /* TWCLOCK_WAYLAND_APP_H */
