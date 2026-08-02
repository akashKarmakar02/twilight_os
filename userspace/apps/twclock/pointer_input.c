#include "pointer_input.h"

#include <string.h>

/* Wayland button values use Linux input event codes; BTN_LEFT is 0x110. */
enum { PRIMARY_BUTTON = 0x110 };

static uint32_t minimum_version(uint32_t advertised, uint32_t supported) {
	return advertised < supported ? advertised : supported;
}

static void destroy_pointer(struct pointer_input *input) {
	if (!input->pointer) {
		return;
	}
	if (input->seat_version >= WL_POINTER_RELEASE_SINCE_VERSION) {
		wl_pointer_release(input->pointer);
	} else {
		wl_proxy_destroy((struct wl_proxy *)input->pointer);
	}
	input->pointer = NULL;
	input->inside_surface = false;
	input->primary_pressed = false;
	input->primary_click_pending = false;
}

static void handle_pointer_enter(void *data, struct wl_pointer *pointer,
                                 uint32_t serial, struct wl_surface *surface,
                                 wl_fixed_t surface_x, wl_fixed_t surface_y) {
	(void)pointer;
	(void)serial;
	(void)surface;
	struct pointer_input *input = data;
	input->x = wl_fixed_to_int(surface_x);
	input->y = wl_fixed_to_int(surface_y);
	input->inside_surface = true;
}

static void handle_pointer_leave(void *data, struct wl_pointer *pointer,
                                 uint32_t serial,
                                 struct wl_surface *surface) {
	(void)pointer;
	(void)serial;
	(void)surface;
	struct pointer_input *input = data;
	input->inside_surface = false;
	input->primary_pressed = false;
}

static void handle_pointer_motion(void *data, struct wl_pointer *pointer,
                                  uint32_t time, wl_fixed_t surface_x,
                                  wl_fixed_t surface_y) {
	(void)pointer;
	(void)time;
	struct pointer_input *input = data;
	input->x = wl_fixed_to_int(surface_x);
	input->y = wl_fixed_to_int(surface_y);
}

static void handle_pointer_button(void *data, struct wl_pointer *pointer,
                                  uint32_t serial, uint32_t time,
                                  uint32_t button, uint32_t state) {
	(void)pointer;
	(void)serial;
	(void)time;
	struct pointer_input *input = data;
	if (button != PRIMARY_BUTTON) {
		return;
	}
	if (state == WL_POINTER_BUTTON_STATE_PRESSED) {
		input->primary_pressed = input->inside_surface;
	} else if (state == WL_POINTER_BUTTON_STATE_RELEASED) {
		input->primary_click_pending =
		    input->primary_pressed && input->inside_surface;
		input->primary_pressed = false;
	}
}

static void handle_pointer_axis(void *data, struct wl_pointer *pointer,
                                uint32_t time, uint32_t axis,
                                wl_fixed_t value) {
	(void)data;
	(void)pointer;
	(void)time;
	(void)axis;
	(void)value;
}

static const struct wl_pointer_listener pointer_listener = {
	.enter = handle_pointer_enter,
	.leave = handle_pointer_leave,
	.motion = handle_pointer_motion,
	.button = handle_pointer_button,
	.axis = handle_pointer_axis,
};

static void handle_seat_capabilities(void *data, struct wl_seat *seat,
                                     uint32_t capabilities) {
	struct pointer_input *input = data;
	bool has_pointer = (capabilities & WL_SEAT_CAPABILITY_POINTER) != 0;
	if (has_pointer && !input->pointer) {
		input->pointer = wl_seat_get_pointer(seat);
		if (!input->pointer ||
		    wl_pointer_add_listener(input->pointer, &pointer_listener, input) <
		        0) {
			destroy_pointer(input);
		}
	} else if (!has_pointer) {
		destroy_pointer(input);
	}
}

static void handle_seat_name(void *data, struct wl_seat *seat,
                             const char *name) {
	(void)data;
	(void)seat;
	(void)name;
}

static const struct wl_seat_listener seat_listener = {
	.capabilities = handle_seat_capabilities,
	.name = handle_seat_name,
};

int pointer_input_bind(struct pointer_input *input,
                       struct wl_registry *registry, uint32_t name,
                       uint32_t version) {
	if (input->seat) {
		return 0;
	}
	input->seat_version = minimum_version(version, 5);
	input->seat = wl_registry_bind(registry, name, &wl_seat_interface,
	                               input->seat_version);
	if (!input->seat ||
	    wl_seat_add_listener(input->seat, &seat_listener, input) < 0) {
		pointer_input_destroy(input);
		return -1;
	}
	return 0;
}

bool pointer_input_take_primary_click(struct pointer_input *input,
                                      int32_t *x, int32_t *y) {
	if (!input->primary_click_pending) {
		return false;
	}
	input->primary_click_pending = false;
	if (x) {
		*x = input->x;
	}
	if (y) {
		*y = input->y;
	}
	return true;
}

void pointer_input_destroy(struct pointer_input *input) {
	if (!input) {
		return;
	}
	destroy_pointer(input);
	if (input->seat) {
		if (input->seat_version >= WL_SEAT_RELEASE_SINCE_VERSION) {
			wl_seat_release(input->seat);
		} else {
			wl_proxy_destroy((struct wl_proxy *)input->seat);
		}
	}
	memset(input, 0, sizeof(*input));
}
