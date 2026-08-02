#ifndef TWCLOCK_POINTER_INPUT_H
#define TWCLOCK_POINTER_INPUT_H

#include <stdbool.h>
#include <stdint.h>
#include <wayland-client.h>

struct pointer_input {
	struct wl_seat *seat;
	struct wl_pointer *pointer;
	uint32_t seat_version;
	int32_t x;
	int32_t y;
	bool inside_surface;
	bool primary_pressed;
	bool primary_click_pending;
};

int pointer_input_bind(struct pointer_input *input,
                       struct wl_registry *registry, uint32_t name,
                       uint32_t version);
bool pointer_input_take_primary_click(struct pointer_input *input,
                                      int32_t *x, int32_t *y);
void pointer_input_destroy(struct pointer_input *input);

#endif /* TWCLOCK_POINTER_INPUT_H */
