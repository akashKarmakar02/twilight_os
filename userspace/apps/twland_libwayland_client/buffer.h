/*
 * buffer.h — a wl_shm backing buffer with a drawn pattern.
 */
#ifndef BUFFER_H
#define BUFFER_H

#include <wayland-client.h>

#include <stdint.h>
#include <stddef.h>

struct buffer {
	struct wl_shm_pool *pool;
	struct wl_buffer *wl;
	uint32_t *pixels;
	size_t size;
	int fd;
};

#define BUFFER_WIDTH 320
#define BUFFER_HEIGHT 200
#define BUFFER_STRIDE (BUFFER_WIDTH * 4)

/*
 * Create a memfd-backed wl_shm pool + buffer of BUFFER_WIDTH x BUFFER_HEIGHT
 * and fill it with a test pattern.  Returns 0 on success, -1 on error.
 */
int buffer_create(struct buffer *b, struct wl_shm *shm);

void buffer_destroy(struct buffer *b);

#endif /* BUFFER_H */
