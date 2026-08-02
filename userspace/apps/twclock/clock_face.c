#include "clock_face.h"

#include <stddef.h>
#include <stdint.h>

/* Each low five bits describe one row of a 5x7 digit. */
static const uint8_t digit_rows[10][7] = {
	{0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e},
	{0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e},
	{0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f},
	{0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e},
	{0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02},
	{0x1f, 0x10, 0x10, 0x1e, 0x01, 0x01, 0x1e},
	{0x0e, 0x10, 0x10, 0x1e, 0x11, 0x11, 0x0e},
	{0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08},
	{0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e},
	{0x0e, 0x11, 0x11, 0x0f, 0x01, 0x01, 0x0e},
};

enum {
	DIGIT_COLUMNS = 5,
	DIGIT_ROWS = 7,
	DIGIT_SCALE = 5,
	DIGIT_GAP = 5,
	COLON_WIDTH = 10,
	TIME_GLYPHS = 8,
	CLOSE_BUTTON_SIZE = 18,
	CLOSE_BUTTON_INSET = 10,
};

static const uint32_t COLOR_BACKGROUND = 0xff101722u;
static const uint32_t COLOR_PANEL = 0xff182333u;
static const uint32_t COLOR_BORDER = 0xff38b2acu;
static const uint32_t COLOR_TIME = 0xffedf2f7u;
static const uint32_t COLOR_SECONDS = 0xff63d5cfu;
static const uint32_t COLOR_CLOSE = 0xfffc8181u;

static void draw_rect(struct shm_buffer *buffer, int x, int y, int width,
                      int height, uint32_t color) {
	if (width <= 0 || height <= 0) {
		return;
	}

	int left = x < 0 ? 0 : x;
	int top = y < 0 ? 0 : y;
	int right = x + width > buffer->width ? buffer->width : x + width;
	int bottom = y + height > buffer->height ? buffer->height : y + height;
	for (int row = top; row < bottom; ++row) {
		for (int column = left; column < right; ++column) {
			buffer->pixels[(size_t)row * (size_t)buffer->width +
			               (size_t)column] = color;
		}
	}
}

static void draw_digit(struct shm_buffer *buffer, int x, int y, int digit,
                       uint32_t color) {
	if (digit < 0 || digit > 9) {
		return;
	}
	for (int row = 0; row < DIGIT_ROWS; ++row) {
		for (int column = 0; column < DIGIT_COLUMNS; ++column) {
			uint8_t mask = (uint8_t)(1u << (DIGIT_COLUMNS - column - 1));
			if ((digit_rows[digit][row] & mask) != 0) {
				draw_rect(buffer, x + column * DIGIT_SCALE,
				          y + row * DIGIT_SCALE, DIGIT_SCALE, DIGIT_SCALE,
				          color);
			}
		}
	}
}

static void draw_colon(struct shm_buffer *buffer, int x, int y,
                       uint32_t color) {
	draw_rect(buffer, x, y + DIGIT_SCALE, COLON_WIDTH, COLON_WIDTH, color);
	draw_rect(buffer, x, y + DIGIT_SCALE * 4, COLON_WIDTH, COLON_WIDTH, color);
}

static int close_button_x(int width) {
	return width - CLOSE_BUTTON_INSET - CLOSE_BUTTON_SIZE;
}

bool clock_face_close_button_contains(int32_t x, int32_t y, int32_t width) {
	int left = close_button_x(width);
	return x >= left && x < left + CLOSE_BUTTON_SIZE &&
	       y >= CLOSE_BUTTON_INSET && y < CLOSE_BUTTON_INSET + CLOSE_BUTTON_SIZE;
}

static void draw_close_button(struct shm_buffer *buffer) {
	int left = close_button_x(buffer->width);
	int top = CLOSE_BUTTON_INSET;
	draw_rect(buffer, left, top, CLOSE_BUTTON_SIZE, 2, COLOR_CLOSE);
	draw_rect(buffer, left, top + CLOSE_BUTTON_SIZE - 2, CLOSE_BUTTON_SIZE, 2,
	          COLOR_CLOSE);
	draw_rect(buffer, left, top, 2, CLOSE_BUTTON_SIZE, COLOR_CLOSE);
	draw_rect(buffer, left + CLOSE_BUTTON_SIZE - 2, top, 2, CLOSE_BUTTON_SIZE,
	          COLOR_CLOSE);
	for (int offset = 5; offset < CLOSE_BUTTON_SIZE - 5; ++offset) {
		draw_rect(buffer, left + offset, top + offset, 2, 2, COLOR_CLOSE);
		draw_rect(buffer, left + CLOSE_BUTTON_SIZE - offset - 2, top + offset,
		          2, 2, COLOR_CLOSE);
	}
}

void clock_face_draw(struct shm_buffer *buffer, const struct tm *local_time) {
	if (!buffer || !buffer->pixels || !local_time) {
		return;
	}

	draw_rect(buffer, 0, 0, buffer->width, buffer->height, COLOR_BACKGROUND);
	draw_rect(buffer, 2, 2, buffer->width - 4, buffer->height - 4, COLOR_BORDER);
	draw_rect(buffer, 5, 5, buffer->width - 10, buffer->height - 10,
	          COLOR_PANEL);
	draw_close_button(buffer);

	const int digits[] = {
		local_time->tm_hour / 10,
		local_time->tm_hour % 10,
		local_time->tm_min / 10,
		local_time->tm_min % 10,
		local_time->tm_sec / 10,
		local_time->tm_sec % 10,
	};
	const int digit_width = DIGIT_COLUMNS * DIGIT_SCALE;
	const int total_width = 6 * digit_width + 2 * COLON_WIDTH +
	                        (TIME_GLYPHS - 1) * DIGIT_GAP;
	int x = (buffer->width - total_width) / 2;
	const int y = (buffer->height - DIGIT_ROWS * DIGIT_SCALE) / 2;

	for (int index = 0; index < 6; ++index) {
		uint32_t color = index >= 4 ? COLOR_SECONDS : COLOR_TIME;
		draw_digit(buffer, x, y, digits[index], color);
		x += digit_width + DIGIT_GAP;
		if (index == 1 || index == 3) {
			draw_colon(buffer, x, y, COLOR_BORDER);
			x += COLON_WIDTH + DIGIT_GAP;
		}
	}
}
