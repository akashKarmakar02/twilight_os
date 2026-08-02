#ifndef TWCLOCK_CLOCK_FACE_H
#define TWCLOCK_CLOCK_FACE_H

#include "shm_buffer.h"

#include <stdbool.h>
#include <stdint.h>
#include <time.h>

void clock_face_draw(struct shm_buffer *buffer, const struct tm *local_time);
bool clock_face_close_button_contains(int32_t x, int32_t y, int32_t width);

#endif /* TWCLOCK_CLOCK_FACE_H */
