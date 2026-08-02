#define _GNU_SOURCE

#include "shm_buffer.h"

#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static void handle_buffer_release(void *data, struct wl_buffer *wayland_buffer) {
	(void)wayland_buffer;
	struct shm_buffer *buffer = data;
	buffer->busy = false;
}

static const struct wl_buffer_listener buffer_listener = {
	.release = handle_buffer_release,
};

int shm_buffer_create(struct shm_buffer *buffer, struct wl_shm *shm,
                      int32_t width, int32_t height) {
	memset(buffer, 0, sizeof(*buffer));
	buffer->fd = -1;

	if (!shm || width <= 0 || height <= 0 || width > INT32_MAX / 4) {
		return -1;
	}

	buffer->width = width;
	buffer->height = height;
	buffer->stride = width * 4;
	if ((size_t)height > (size_t)INT32_MAX / (size_t)buffer->stride) {
		return -1;
	}
	buffer->size = (size_t)buffer->stride * (size_t)height;

	buffer->fd = memfd_create("twclock-buffer", 0);
	if (buffer->fd < 0) {
		perror("twclock: memfd_create");
		return -1;
	}
	if (ftruncate(buffer->fd, (off_t)buffer->size) < 0) {
		perror("twclock: ftruncate");
		shm_buffer_destroy(buffer);
		return -1;
	}

	buffer->pixels = mmap(NULL, buffer->size, PROT_READ | PROT_WRITE,
	                      MAP_SHARED, buffer->fd, 0);
	if (buffer->pixels == MAP_FAILED) {
		buffer->pixels = NULL;
		perror("twclock: mmap");
		shm_buffer_destroy(buffer);
		return -1;
	}

	buffer->pool = wl_shm_create_pool(shm, buffer->fd, (int32_t)buffer->size);
	if (!buffer->pool) {
		fprintf(stderr, "twclock: wl_shm_create_pool failed\n");
		shm_buffer_destroy(buffer);
		return -1;
	}

	buffer->wayland_buffer = wl_shm_pool_create_buffer(
	    buffer->pool, 0, width, height, buffer->stride, WL_SHM_FORMAT_XRGB8888);
	if (!buffer->wayland_buffer) {
		fprintf(stderr, "twclock: wl_shm_pool_create_buffer failed\n");
		shm_buffer_destroy(buffer);
		return -1;
	}
	if (wl_buffer_add_listener(buffer->wayland_buffer, &buffer_listener,
	                           buffer) < 0) {
		fprintf(stderr, "twclock: wl_buffer_add_listener failed\n");
		shm_buffer_destroy(buffer);
		return -1;
	}

	return 0;
}

void shm_buffer_destroy(struct shm_buffer *buffer) {
	if (!buffer) {
		return;
	}
	if (buffer->wayland_buffer) {
		wl_buffer_destroy(buffer->wayland_buffer);
	}
	if (buffer->pool) {
		wl_shm_pool_destroy(buffer->pool);
	}
	if (buffer->pixels) {
		munmap(buffer->pixels, buffer->size);
	}
	if (buffer->fd >= 0) {
		close(buffer->fd);
	}

	memset(buffer, 0, sizeof(*buffer));
	buffer->fd = -1;
}
