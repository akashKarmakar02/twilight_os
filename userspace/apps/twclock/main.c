#define _POSIX_C_SOURCE 200809L

#include "app_config.h"
#include "clock_face.h"
#include "shm_buffer.h"
#include "wayland_app.h"

#include <stdbool.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

static volatile sig_atomic_t stop_requested;

static void request_stop(int signal_number) {
	(void)signal_number;
	stop_requested = 1;
}

static void install_signal_handlers(void) {
	struct sigaction action = {
		.sa_handler = request_stop,
	};
	sigemptyset(&action.sa_mask);
	(void)sigaction(SIGINT, &action, NULL);
	(void)sigaction(SIGTERM, &action, NULL);
}

static struct shm_buffer *available_buffer(struct shm_buffer *buffers) {
	for (int index = 0; index < TWCLOCK_BUFFER_COUNT; ++index) {
		if (!buffers[index].busy) {
			return &buffers[index];
		}
	}
	return NULL;
}

static int milliseconds_until_next_second(void) {
	struct timespec now;
	if (clock_gettime(CLOCK_REALTIME, &now) < 0) {
		return 100;
	}
	int milliseconds = 1000 - (int)(now.tv_nsec / 1000000L);
	return milliseconds > 0 ? milliseconds : 1;
}

static int read_local_time(time_t now, struct tm *result) {
	if (localtime_r(&now, result)) {
		return 0;
	}
	return gmtime_r(&now, result) ? 0 : -1;
}

int main(void) {
	install_signal_handlers();
	if (!getenv("TZ")) {
		(void)setenv("TZ", ":/etc/localtime", 0);
	}
	tzset();

	struct wayland_app app;
	if (wayland_app_init(&app, TWCLOCK_WIDTH, TWCLOCK_HEIGHT) < 0 ||
	    wayland_app_wait_until_configured(&app) < 0) {
		wayland_app_destroy(&app);
		return 1;
	}

	struct shm_buffer buffers[TWCLOCK_BUFFER_COUNT];
	for (int index = 0; index < TWCLOCK_BUFFER_COUNT; ++index) {
		buffers[index].fd = -1;
		if (shm_buffer_create(&buffers[index], app.shm, (int32_t)app.width,
		                      (int32_t)app.height) < 0) {
			for (int built = 0; built < index; ++built) {
				shm_buffer_destroy(&buffers[built]);
			}
			wayland_app_destroy(&app);
			return 1;
		}
	}

	time_t rendered_second = (time_t)-1;
	bool exited_cleanly = false;
	while (!app.closed && !stop_requested) {
		time_t now = time(NULL);
		struct shm_buffer *buffer = available_buffer(buffers);
		if (now != (time_t)-1 && now != rendered_second && buffer) {
			struct tm local_time;
			if (read_local_time(now, &local_time) < 0) {
				fprintf(stderr, "twclock: failed to read local time\n");
				break;
			}
			clock_face_draw(buffer, &local_time);
			buffer->busy = true;
			if (wayland_app_present(&app, buffer->wayland_buffer) < 0) {
				break;
			}
			rendered_second = now;
		}

		if (wayland_app_dispatch(&app,
		                         milliseconds_until_next_second()) < 0) {
			break;
		}

		int32_t click_x;
		int32_t click_y;
		if (pointer_input_take_primary_click(&app.pointer_input, &click_x,
		                                     &click_y) &&
		    clock_face_close_button_contains(click_x, click_y,
		                                     (int32_t)app.width)) {
			exited_cleanly = true;
			break;
		}
	}
	if (app.closed || stop_requested) {
		exited_cleanly = true;
	}

	for (int index = 0; index < TWCLOCK_BUFFER_COUNT; ++index) {
		shm_buffer_destroy(&buffers[index]);
	}
	wayland_app_destroy(&app);
	return exited_cleanly ? 0 : 1;
}
