#define _POSIX_C_SOURCE 200809L
#include <fcntl.h>
// #include <linux/fb.h> // Not available in this userspace env
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h> // for nanosleep
#include <unistd.h>

#define FB_PATH "/dev/fb0"
#define MOUSE_PATH "/dev/input/mice"

// Framebuffer definitions (copied from chip8/linux headers)
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOGET_FSCREENINFO 0x4602
#define FBIOPAN_DISPLAY 0x4606

struct fb_var_screeninfo {
  uint32_t xres;
  uint32_t yres;
  uint32_t bits_per_pixel;
  uint32_t red_offset;
  uint32_t green_offset;
  uint32_t blue_offset;
};

struct fb_fix_screeninfo {
  uint32_t line_length;
  uint32_t smem_len;
};
#define MOUSE_PATH "/dev/input/mice"

// Basic cursor bitmap (8x8 block for simplicity, or we can make it prettier
// later)
#define CURSOR_SIZE 10
#define CURSOR_COLOR 0xFF000000 // Black (ARGB)
#define WINDOW_COLOR 0xFF008080 // Teal (ARGB)

static int screen_w, screen_h;
static uint32_t *fb_ptr;

static void fill_screen(uint32_t color) {
  size_t count = screen_w * screen_h;
  for (size_t i = 0; i < count; ++i) {
    fb_ptr[i] = color;
  }
}

static void draw_rect(int x, int y, int w, int h, uint32_t color) {
  if (x >= screen_w || y >= screen_h)
    return;
  // Clip
  if (x < 0) {
    w += x;
    x = 0;
  }
  if (y < 0) {
    h += y;
    y = 0;
  }
  if (x + w > screen_w)
    w = screen_w - x;
  if (y + h > screen_h)
    h = screen_h - y;
  if (w <= 0 || h <= 0)
    return;

  for (int row = 0; row < h; ++row) {
    uint32_t *dst = fb_ptr + (y + row) * screen_w + x;
    for (int col = 0; col < w; ++col) {
      dst[col] = color;
    }
  }
}

// Simple buffer to store what was under the cursor
static uint32_t under_cursor[CURSOR_SIZE * CURSOR_SIZE];
static int saved_x = -1, saved_y = -1;

static void save_under_cursor(int x, int y) {
  saved_x = x;
  saved_y = y;
  for (int row = 0; row < CURSOR_SIZE; ++row) {
    int sy = y + row;
    for (int col = 0; col < CURSOR_SIZE; ++col) {
      int sx = x + col;
      if (sx >= 0 && sx < screen_w && sy >= 0 && sy < screen_h) {
        under_cursor[row * CURSOR_SIZE + col] = fb_ptr[sy * screen_w + sx];
      } else {
        under_cursor[row * CURSOR_SIZE + col] = 0; // Out of bounds
      }
    }
  }
}

static void restore_under_cursor() {
  if (saved_x == -1)
    return;
  for (int row = 0; row < CURSOR_SIZE; ++row) {
    int sy = saved_y + row;
    for (int col = 0; col < CURSOR_SIZE; ++col) {
      int sx = saved_x + col;
      if (sx >= 0 && sx < screen_w && sy >= 0 && sy < screen_h) {
        fb_ptr[sy * screen_w + sx] = under_cursor[row * CURSOR_SIZE + col];
      }
    }
  }
}

static void draw_cursor(int x, int y) {
  draw_rect(x, y, CURSOR_SIZE, CURSOR_SIZE, CURSOR_COLOR);
}

// PS/2 Packet Packet
// Byte 0: Yovfl, Xovfl, Ysign, Xsign, 1, Mbtn, Rbtn, Lbtn
// Byte 1: X movement
// Byte 2: Y movement

int main() {
  printf("Starting TWC - Twilight Compositor...\n");

  // 1. Open Framebuffer
  int fb_fd = open(FB_PATH, O_RDWR);
  if (fb_fd < 0) {
    perror("open fb");
    return 1;
  }

  struct fb_var_screeninfo vinfo;
  struct fb_fix_screeninfo finfo;

  if (ioctl(fb_fd, FBIOGET_VSCREENINFO, &vinfo) < 0) {
    perror("ioctl get vinfo");
    return 1;
  }
  if (ioctl(fb_fd, FBIOGET_FSCREENINFO, &finfo) < 0) {
    perror("ioctl get finfo");
    return 1;
  }

  screen_w = vinfo.xres;
  screen_h = vinfo.yres;
  size_t screensize = finfo.smem_len;

  fb_ptr = (uint32_t *)mmap(0, screensize, PROT_READ | PROT_WRITE, MAP_SHARED,
                            fb_fd, 0);
  if (fb_ptr == MAP_FAILED) {
    perror("mmap");
    return 1;
  }
  printf("Framebuffer: %dx%d, mapped at %p, size %zu\n", screen_w, screen_h,
         fb_ptr, screensize);

  // Clear screen with Window Color
  fill_screen(WINDOW_COLOR);

  // 2. Open Mouse
  int mouse_fd = open(MOUSE_PATH, O_RDONLY);
  if (mouse_fd < 0) {
    perror("open mouse");
    printf("WARNING: Could not open mouse, drawing static cursor.\n");
  }

  int cur_x = screen_w / 2;
  int cur_y = screen_h / 2;

  // Initial draw
  save_under_cursor(cur_x, cur_y);
  draw_cursor(cur_x, cur_y);

  // SYNC INITIAL FRAME
  ioctl(fb_fd, FBIOPAN_DISPLAY, 0);

  if (mouse_fd < 0) {
    return 0; // Just exit if no mouse, or loop? Let's loop lightly.
    while (1)
      sleep(1);
  }

  // 3. Loop
  unsigned char packet[3];
  while (1) {
    int n = read(mouse_fd, packet, 3);
    if (n == 3) {
      // Parse PS/2
      int dx = packet[1];
      int dy = packet[2];
      uint8_t flags = packet[0];

      // Sign extension
      if ((flags & 0x10) && dx != 0)
        dx |= 0xFFFFFF00; // X sign bit
      if ((flags & 0x20) && dy != 0)
        dy |= 0xFFFFFF00; // Y sign bit

      // Y is usually inverted in PS/2 relative to screen coordinates?
      // "Y data received... 1 = negative (up), 0 = positive (down)" ?
      // Actually, usually Up is positive in PS/2 hardware, but Screen Y is
      // Down. Let's negate dy.
      dy = -dy;

      // Restore old background
      restore_under_cursor();

      // Apply movement
      cur_x += dx;
      cur_y += dy;

      // Clamp
      if (cur_x < 0)
        cur_x = 0;
      if (cur_x > screen_w - CURSOR_SIZE)
        cur_x = screen_w - CURSOR_SIZE;
      if (cur_y < 0)
        cur_y = 0;
      if (cur_y > screen_h - CURSOR_SIZE)
        cur_y = screen_h - CURSOR_SIZE;

      // Save new background
      save_under_cursor(cur_x, cur_y);

      // Draw new cursor
      draw_cursor(cur_x, cur_y);

      // Sync is REQUIRED because we are mmapped to the backbuffer.
      // FBIOPAN_DISPLAY triggers sync_full() in the kernel.
      ioctl(fb_fd, FBIOPAN_DISPLAY, 0);
    } else {
      // If read returns 0 (EOF) or error, or partial data, we shouldn't spin
      // 100%. Sleep for a short duration (10ms).
      struct timespec ts = {0, 10 * 1000 * 1000};
      nanosleep(&ts, NULL);
    }
  }

  close(mouse_fd);
  close(fb_fd);
  return 0;
}
