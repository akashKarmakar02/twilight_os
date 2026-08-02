/*
 * buffer.c — a wl_shm backing buffer with a drawn pattern.
 */
#define _GNU_SOURCE

#include "buffer.h"

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

/* XRGB8888 in Wayland's native byte order (0x00RRGGBB on little-endian). */
#define SHM_FORMAT_XRGB8888 1

static void fill_pattern(uint32_t *pixels, enum buffer_pattern pattern) {
	for (int y = 0; y < BUFFER_HEIGHT; y++) {
		for (int x = 0; x < BUFFER_WIDTH; x++) {
			uint32_t color = pattern == BUFFER_PATTERN_LAYER
			                     ? 0xff243447u /* dark slate */
			                     : 0xff3366ccu; /* soft blue */
			if (x < 6 || y < 6 || x >= BUFFER_WIDTH - 6 ||
			    y >= BUFFER_HEIGHT - 6) {
				color = pattern == BUFFER_PATTERN_LAYER
				            ? 0xff38b2acu /* teal border */
				            : 0xffcc6633u; /* orange border */
			}
			/* a diagonal accent so the window is unmistakably rendered */
			if (x == (y * BUFFER_WIDTH) / BUFFER_HEIGHT ||
			    x == (y * BUFFER_WIDTH) / BUFFER_HEIGHT + 1) {
				color = 0xffffffffu;
			}
			pixels[y * BUFFER_WIDTH + x] = color;
		}
	}
}

int buffer_create(struct buffer *b, struct wl_shm *shm,
                  enum buffer_pattern pattern) {
	memset(b, 0, sizeof(*b));
	b->size = (size_t)BUFFER_STRIDE * BUFFER_HEIGHT;

	b->fd = memfd_create("twland-libwayland-client", 0);
	if (b->fd < 0) {
		perror("buffer: memfd_create");
		return -1;
	}
	if (ftruncate(b->fd, (off_t)b->size) < 0) {
		perror("buffer: ftruncate");
		close(b->fd);
		return -1;
	}

	b->pixels = mmap(NULL, b->size, PROT_READ | PROT_WRITE, MAP_SHARED,
	                 b->fd, 0);
	if (b->pixels == MAP_FAILED) {
		perror("buffer: mmap");
		close(b->fd);
		return -1;
	}
	fill_pattern(b->pixels, pattern);

	b->pool = wl_shm_create_pool(shm, b->fd, (int32_t)b->size);
	if (!b->pool) {
		fprintf(stderr, "buffer: wl_shm_create_pool failed\n");
		munmap(b->pixels, b->size);
		close(b->fd);
		return -1;
	}
	b->wl = wl_shm_pool_create_buffer(b->pool, 0, BUFFER_WIDTH,
	                                  BUFFER_HEIGHT, BUFFER_STRIDE,
	                                  SHM_FORMAT_XRGB8888);
	if (!b->wl) {
		fprintf(stderr, "buffer: wl_shm_pool_create_buffer failed\n");
		wl_shm_pool_destroy(b->pool);
		munmap(b->pixels, b->size);
		close(b->fd);
		return -1;
	}
	return 0;
}

void buffer_destroy(struct buffer *b) {
	if (b->wl) {
		wl_buffer_destroy(b->wl);
	}
	if (b->pool) {
		wl_shm_pool_destroy(b->pool);
	}
	if (b->pixels && b->pixels != MAP_FAILED) {
		munmap(b->pixels, b->size);
	}
	if (b->fd >= 0) {
		close(b->fd);
	}
	memset(b, 0, sizeof(*b));
	b->fd = -1;
}
