/*
 * twland_libwayland_client - a minimal Wayland client built against the
 * official libwayland-client (the .so cross-built for musl in issue #57).
 *
 * It maps both a background layer surface and an xdg-shell toplevel through
 * generated, typed wayland-scanner bindings. This proves the layer protocol's
 * configure lifecycle and its composition order relative to normal windows.
 *
 * Flow:
 *   connect -> bind compositor/shm/layer-shell/xdg-shell
 *     -> create both shell roles -> empty commits -> configure/ack
 *     -> attach distinct shm buffers -> map layer, then xdg -> run until closed
 *
 * Build: see Makefile (links -lwayland-client, generates xdg-shell bindings).
 */

#define _GNU_SOURCE

#include "buffer.h"
#include "registry.h"
#include "wlr-layer-shell-unstable-v1-client-protocol.h"
#include "xdg-shell-client-protocol.h"

#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wayland-client.h>

/* State shared between the xdg-shell listeners and the main loop. */
struct client {
	struct wl_surface *surface;
	struct xdg_surface *xdg_surface;
	struct xdg_toplevel *toplevel;
	struct wl_surface *layer_wl_surface;
	struct zwlr_layer_surface_v1 *layer_surface;
	int configured;  /* set when the initial configure arrives */
	int layer_configured;
	int closed;      /* set when the toplevel close event arrives */
	int layer_closed;
};

/* --- wlr layer-shell: ack the compositor's initial size suggestion --- */

static void layer_surface_configure(void *data,
                                    struct zwlr_layer_surface_v1 *surface,
                                    uint32_t serial, uint32_t width,
                                    uint32_t height) {
	struct client *c = data;
	zwlr_layer_surface_v1_ack_configure(surface, serial);
	c->layer_configured = 1;
	printf("twland_libwayland_client: layer configured %ux%u\n", width,
	       height);
}

static void layer_surface_closed(void *data,
                                 struct zwlr_layer_surface_v1 *surface) {
	(void)surface;
	struct client *c = data;
	c->layer_closed = 1;
}

static const struct zwlr_layer_surface_v1_listener layer_surface_listener = {
	.configure = layer_surface_configure,
	.closed = layer_surface_closed,
};

/* --- xdg_wm_base: answer pings or twland considers us unresponsive --- */

static void xdg_wm_base_ping(void *data, struct xdg_wm_base *wm,
                             uint32_t serial) {
	(void)data;
	xdg_wm_base_pong(wm, serial);
}

static const struct xdg_wm_base_listener xdg_wm_base_listener = {
	.ping = xdg_wm_base_ping,
};

/* --- xdg_surface: ack every configure; the first one maps the window --- */

static void xdg_surface_configure(void *data, struct xdg_surface *xdg,
                                  uint32_t serial) {
	struct client *c = data;
	xdg_surface_ack_configure(xdg, serial);
	c->configured = 1;
}

static const struct xdg_surface_listener xdg_surface_listener = {
	.configure = xdg_surface_configure,
};

/* --- xdg_toplevel: track close requests --- */

static void xdg_toplevel_configure(void *data, struct xdg_toplevel *toplevel,
                                  int32_t width, int32_t height,
                                  struct wl_array *states) {
	(void)data; (void)toplevel; (void)width; (void)height; (void)states;
}

static void xdg_toplevel_close(void *data, struct xdg_toplevel *toplevel) {
	(void)toplevel;
	struct client *c = data;
	c->closed = 1;
}

static const struct xdg_toplevel_listener xdg_toplevel_listener = {
	.configure = xdg_toplevel_configure,
	.close = xdg_toplevel_close,
};

int main(void) {
	/*
	 * twland binds the socket at /run/user/0/wayland-0.  libwayland resolves
	 * $WAYLAND_DISPLAY (default "wayland-0") under $XDG_RUNTIME_DIR, so point
	 * the runtime dir there and let wl_display_connect(NULL) do the rest.
	 */
	if (setenv("XDG_RUNTIME_DIR", "/run/user/0", 1) < 0) {
		perror("setenv");
		return 1;
	}

	struct wl_display *display = wl_display_connect(NULL);
	if (!display) {
		perror("twland_libwayland_client: connect");
		return 1;
	}
	puts("twland_libwayland_client: connected");

	struct wl_registry *registry = wl_display_get_registry(display);
	struct globals g;
	if (globals_bind(display, registry, &g) < 0) {
		fprintf(stderr, "twland_libwayland_client: missing required global\n");
		return 2;
	}
	puts("twland_libwayland_client: globals bound");

	if (xdg_wm_base_add_listener(g.xdg_wm_base, &xdg_wm_base_listener,
	                             NULL) < 0) {
		return 3;
	}

	struct buffer layer_buf;
	struct buffer window_buf;
	if (buffer_create(&layer_buf, g.shm, BUFFER_PATTERN_LAYER) < 0 ||
	    buffer_create(&window_buf, g.shm, BUFFER_PATTERN_WINDOW) < 0) {
		return 4;
	}

	struct client c = {0};
	c.layer_wl_surface = wl_compositor_create_surface(g.compositor);
	if (!c.layer_wl_surface) {
		return 5;
	}
	c.layer_surface = zwlr_layer_shell_v1_get_layer_surface(
	    g.layer_shell, c.layer_wl_surface, NULL,
	    ZWLR_LAYER_SHELL_V1_LAYER_BACKGROUND, "twland-layer-demo");
	if (!c.layer_surface) {
		return 6;
	}
	if (zwlr_layer_surface_v1_add_listener(c.layer_surface,
	                                       &layer_surface_listener, &c) < 0) {
		return 7;
	}
	zwlr_layer_surface_v1_set_size(c.layer_surface, BUFFER_WIDTH,
	                               BUFFER_HEIGHT);
	/* The required empty commit triggers layer_surface.configure. */
	wl_surface_commit(c.layer_wl_surface);

	c.surface = wl_compositor_create_surface(g.compositor);
	if (!c.surface) {
		return 8;
	}
	c.xdg_surface = xdg_wm_base_get_xdg_surface(g.xdg_wm_base, c.surface);
	if (!c.xdg_surface) {
		return 9;
	}
	if (xdg_surface_add_listener(c.xdg_surface, &xdg_surface_listener, &c) < 0) {
		return 10;
	}
	c.toplevel = xdg_surface_get_toplevel(c.xdg_surface);
	if (!c.toplevel) {
		return 11;
	}
	xdg_toplevel_set_title(c.toplevel, "twland libwayland window");
	if (xdg_toplevel_add_listener(c.toplevel, &xdg_toplevel_listener, &c) < 0) {
		return 12;
	}

	/* Empty commit triggers the initial xdg_surface.configure. */
	wl_surface_commit(c.surface);

	/* Dispatch until configured, then attach the buffer and map. */
	while ((!c.configured || !c.layer_configured) && !c.closed &&
	       !c.layer_closed) {
		if (wl_display_dispatch(display) < 0) {
			perror("twland_libwayland_client: dispatch");
			return 13;
		}
	}
	if (c.closed || c.layer_closed) {
		goto done;
	}
	puts("twland_libwayland_client: shells configured, attaching buffers");

	wl_surface_attach(c.layer_wl_surface, layer_buf.wl, 0, 0);
	wl_surface_damage(c.layer_wl_surface, 0, 0, BUFFER_WIDTH, BUFFER_HEIGHT);
	wl_surface_commit(c.layer_wl_surface);
	puts("twland_libwayland_client: background layer mapped");

	wl_surface_attach(c.surface, window_buf.wl, 0, 0);
	wl_surface_damage(c.surface, 0, 0, BUFFER_WIDTH, BUFFER_HEIGHT);
	wl_surface_commit(c.surface);
	puts("twland_libwayland_client: window mapped");

	/* Run until the compositor closes the toplevel or the socket drops. */
	while (!c.closed) {
		if (wl_display_dispatch(display) < 0) {
			perror("twland_libwayland_client: event loop");
			break;
		}
	}

done:
	puts("twland_libwayland_client: done");
	if (c.toplevel) {
		xdg_toplevel_destroy(c.toplevel);
	}
	if (c.xdg_surface) {
		xdg_surface_destroy(c.xdg_surface);
	}
	if (c.surface) {
		wl_surface_destroy(c.surface);
	}
	if (c.layer_surface) {
		zwlr_layer_surface_v1_destroy(c.layer_surface);
	}
	if (c.layer_wl_surface) {
		wl_surface_destroy(c.layer_wl_surface);
	}
	buffer_destroy(&window_buf);
	buffer_destroy(&layer_buf);
	wl_display_disconnect(display);
	return 0;
}
