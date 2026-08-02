#ifndef TWCLOCK_SHM_BUFFER_H
#define TWCLOCK_SHM_BUFFER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <wayland-client.h>

struct shm_buffer {
	struct wl_shm_pool *pool;
	struct wl_buffer *wayland_buffer;
	uint32_t *pixels;
	size_t size;
	int32_t width;
	int32_t height;
	int32_t stride;
	int fd;
	bool busy;
};

int shm_buffer_create(struct shm_buffer *buffer, struct wl_shm *shm,
                      int32_t width, int32_t height);
void shm_buffer_destroy(struct shm_buffer *buffer);

#endif /* TWCLOCK_SHM_BUFFER_H */
