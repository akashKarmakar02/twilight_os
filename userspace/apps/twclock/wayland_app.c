#define _POSIX_C_SOURCE 200809L

#include "wayland_app.h"

#include "app_config.h"
#include "wlr-layer-shell-unstable-v1-client-protocol.h"

#include <errno.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

enum { INITIAL_CONFIGURE_TIMEOUT_MS = 5000 };

static int64_t monotonic_time_ms(void) {
	struct timespec now;
	if (clock_gettime(CLOCK_MONOTONIC, &now) < 0) {
		return -1;
	}
	return (int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

static uint32_t minimum_version(uint32_t advertised, uint32_t supported) {
	return advertised < supported ? advertised : supported;
}

static void handle_registry_global(
    void *data, struct wl_registry *registry, uint32_t name,
    const char *interface, uint32_t version) {
	struct wayland_app *app = data;
	if (strcmp(interface, wl_compositor_interface.name) == 0) {
		app->compositor = wl_registry_bind(
		    registry, name, &wl_compositor_interface,
		    minimum_version(version, 4));
	} else if (strcmp(interface, wl_shm_interface.name) == 0) {
		app->shm = wl_registry_bind(registry, name, &wl_shm_interface, 1);
	} else if (strcmp(interface, zwlr_layer_shell_v1_interface.name) == 0) {
		app->layer_shell_version = minimum_version(version, 4);
		app->layer_shell = wl_registry_bind(
		    registry, name, &zwlr_layer_shell_v1_interface,
		    app->layer_shell_version);
	} else if (strcmp(interface, wl_seat_interface.name) == 0) {
		(void)pointer_input_bind(&app->pointer_input, registry, name, version);
	}
}

static void handle_registry_remove(
    void *data, struct wl_registry *registry, uint32_t name) {
	(void)data;
	(void)registry;
	(void)name;
}

static const struct wl_registry_listener registry_listener = {
	.global = handle_registry_global,
	.global_remove = handle_registry_remove,
};

static void handle_layer_configure(
    void *data, struct zwlr_layer_surface_v1 *surface, uint32_t serial,
    uint32_t width, uint32_t height) {
	struct wayland_app *app = data;
	zwlr_layer_surface_v1_ack_configure(surface, serial);
	if (width != 0) {
		app->width = width;
	}
	if (height != 0) {
		app->height = height;
	}
	app->configured = true;
}

static void handle_layer_closed(
    void *data, struct zwlr_layer_surface_v1 *surface) {
	(void)surface;
	struct wayland_app *app = data;
	app->closed = true;
}

static const struct zwlr_layer_surface_v1_listener layer_surface_listener = {
	.configure = handle_layer_configure,
	.closed = handle_layer_closed,
};

int wayland_app_init(struct wayland_app *app, uint32_t width, uint32_t height) {
	memset(app, 0, sizeof(*app));
	app->width = width;
	app->height = height;

	if (!getenv("XDG_RUNTIME_DIR") &&
	    setenv("XDG_RUNTIME_DIR", "/run/user/0", 0) < 0) {
		perror("twclock: setenv XDG_RUNTIME_DIR");
		return -1;
	}

	app->display = wl_display_connect(NULL);
	if (!app->display) {
		perror("twclock: wl_display_connect");
		return -1;
	}
	app->registry = wl_display_get_registry(app->display);
	if (!app->registry ||
	    wl_registry_add_listener(app->registry, &registry_listener, app) < 0 ||
	    wl_display_roundtrip(app->display) < 0) {
		fprintf(stderr, "twclock: failed to read Wayland globals\n");
		goto fail;
	}
	if (!app->compositor || !app->shm || !app->layer_shell) {
		fprintf(stderr,
		        "twclock: compositor, shm, or layer-shell global is missing\n");
		goto fail;
	}

	app->surface = wl_compositor_create_surface(app->compositor);
	if (!app->surface) {
		fprintf(stderr, "twclock: wl_compositor_create_surface failed\n");
		goto fail;
	}
	app->layer_surface = zwlr_layer_shell_v1_get_layer_surface(
	    app->layer_shell, app->surface, NULL,
	    ZWLR_LAYER_SHELL_V1_LAYER_BOTTOM, "twclock");
	if (!app->layer_surface ||
	    zwlr_layer_surface_v1_add_listener(
	        app->layer_surface, &layer_surface_listener, app) < 0) {
		fprintf(stderr, "twclock: failed to create layer surface\n");
		goto fail;
	}

	zwlr_layer_surface_v1_set_size(app->layer_surface, width, height);
	zwlr_layer_surface_v1_set_anchor(
	    app->layer_surface,
	    ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP |
	        ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT);
	zwlr_layer_surface_v1_set_margin(app->layer_surface, TWCLOCK_MARGIN_TOP,
	                                 TWCLOCK_MARGIN_RIGHT, 0, 0);
	zwlr_layer_surface_v1_set_exclusive_zone(app->layer_surface, -1);
	zwlr_layer_surface_v1_set_keyboard_interactivity(
	    app->layer_surface,
	    ZWLR_LAYER_SURFACE_V1_KEYBOARD_INTERACTIVITY_NONE);

	/* The first bufferless commit starts the configure/ack handshake. */
	wl_surface_commit(app->surface);
	return 0;

fail:
	wayland_app_destroy(app);
	return -1;
}

int wayland_app_wait_until_configured(struct wayland_app *app) {
	int64_t now = monotonic_time_ms();
	if (now < 0) {
		return -1;
	}
	const int64_t deadline = now + INITIAL_CONFIGURE_TIMEOUT_MS;

	while (!app->configured && !app->closed) {
		now = monotonic_time_ms();
		if (now < 0) {
			return -1;
		}
		if (now >= deadline) {
			fprintf(stderr, "twclock: initial configure timed out\n");
			return -1;
		}
		if (wayland_app_dispatch(app, (int)(deadline - now)) < 0) {
			return -1;
		}
	}
	return app->configured ? 0 : -1;
}

int wayland_app_dispatch(struct wayland_app *app, int timeout_ms) {
	if (wl_display_dispatch_pending(app->display) < 0) {
		return -1;
	}
	int flush_result = wl_display_flush(app->display);
	if (flush_result < 0 && errno != EAGAIN) {
		return -1;
	}

	struct pollfd descriptor = {
		.fd = wl_display_get_fd(app->display),
		.events = POLLIN | (flush_result < 0 ? POLLOUT : 0),
	};
	int result;
	do {
		result = poll(&descriptor, 1, timeout_ms);
	} while (result < 0 && errno == EINTR);
	if (result < 0 ||
	    (descriptor.revents & (POLLERR | POLLHUP | POLLNVAL)) != 0) {
		return -1;
	}
	if (result > 0 && (descriptor.revents & POLLIN) != 0) {
		return wl_display_dispatch(app->display);
	}
	if (result > 0 && (descriptor.revents & POLLOUT) != 0 &&
	    wl_display_flush(app->display) < 0 && errno != EAGAIN) {
		return -1;
	}
	return 0;
}

int wayland_app_present(struct wayland_app *app, struct wl_buffer *buffer) {
	if (!buffer || app->closed) {
		return -1;
	}
	wl_surface_attach(app->surface, buffer, 0, 0);
	wl_surface_damage(app->surface, 0, 0, (int32_t)app->width,
	                  (int32_t)app->height);
	wl_surface_commit(app->surface);
	return wl_display_flush(app->display) < 0 && errno != EAGAIN ? -1 : 0;
}

void wayland_app_destroy(struct wayland_app *app) {
	if (!app) {
		return;
	}
	if (app->layer_surface) {
		zwlr_layer_surface_v1_destroy(app->layer_surface);
	}
	pointer_input_destroy(&app->pointer_input);
	if (app->surface) {
		wl_surface_destroy(app->surface);
	}
	if (app->layer_shell) {
		if (app->layer_shell_version >= 3) {
			zwlr_layer_shell_v1_destroy(app->layer_shell);
		} else {
			wl_proxy_destroy((struct wl_proxy *)app->layer_shell);
		}
	}
	if (app->shm) {
		wl_shm_destroy(app->shm);
	}
	if (app->compositor) {
		wl_compositor_destroy(app->compositor);
	}
	if (app->registry) {
		wl_registry_destroy(app->registry);
	}
	if (app->display) {
		wl_display_disconnect(app->display);
	}
	memset(app, 0, sizeof(*app));
}
