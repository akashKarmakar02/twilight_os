#define _POSIX_C_SOURCE 200809L
#include "twigl.h"

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#define FB_PATH "/dev/fb0"
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOGET_FSCREENINFO 0x4602
#define FBIOPAN_DISPLAY 0x4606
#define GLX_MAX_KEYS 512
#define GLX_KEY_HOLD_SECONDS 0.12

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

typedef struct GlxState {
    int fb_fd;
    uint32_t *pixels;
    size_t map_bytes;
    uint32_t width;
    uint32_t height;
    uint32_t stride_pixels;
    int target_fps;
    double target_frame_seconds;
    double frame_time;
    struct timespec frame_start;
    uint8_t key_down[GLX_MAX_KEYS];
    uint8_t key_pressed[GLX_MAX_KEYS];
    double key_down_until[GLX_MAX_KEYS];
    struct termios old_termios;
    int old_stdin_flags;
    int raw_stdin_enabled;
    int initialized;
} GlxState;

static GlxState glx = {
    .fb_fd = -1,
    .old_stdin_flags = -1,
    .target_fps = 60,
    .target_frame_seconds = 1.0 / 60.0,
};

static volatile sig_atomic_t glx_should_close = 0;

static void glx_sigint(int sig) {
    (void)sig;
    glx_should_close = 1;
}

static double ts_seconds(struct timespec ts) {
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1000000000.0;
}

static struct timespec ts_now(void) {
    struct timespec ts = {0};
    (void)clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts;
}

double GetTime(void) {
    return ts_seconds(ts_now());
}

static void sleep_seconds(double seconds) {
    if (seconds <= 0.0) {
        return;
    }

    struct timespec req;
    req.tv_sec = (time_t)seconds;
    req.tv_nsec = (long)((seconds - (double)req.tv_sec) * 1000000000.0);
    if (req.tv_nsec < 0) {
        req.tv_nsec = 0;
    } else if (req.tv_nsec > 999999999L) {
        req.tv_nsec = 999999999L;
    }

    while (nanosleep(&req, &req) < 0 && errno == EINTR) {
        if (glx_should_close) {
            break;
        }
    }
}

static void glx_reset_pressed_keys(void) {
    memset(glx.key_pressed, 0, sizeof(glx.key_pressed));
}

static void glx_mark_key_pressed(int key) {
    if (key < 0 || key >= GLX_MAX_KEYS) {
        return;
    }

    if (!glx.key_down[key]) {
        glx.key_pressed[key] = 1;
    }
    glx.key_down[key] = 1;
    glx.key_down_until[key] = GetTime() + GLX_KEY_HOLD_SECONDS;
}

static int glx_key_from_ascii(unsigned char ch) {
    switch (ch) {
    case '0': return KEY_0;
    case '1': return KEY_1;
    case '2': return KEY_2;
    case '3': return KEY_3;
    case '4': return KEY_4;
    case '5': return KEY_5;
    case '6': return KEY_6;
    case '7': return KEY_7;
    case '8': return KEY_8;
    case '9': return KEY_9;
    case ' ': return KEY_SPACE;
    case 'a':
    case 'A': return KEY_A;
    case 'b':
    case 'B': return KEY_B;
    case 'c':
    case 'C': return KEY_C;
    case 'd':
    case 'D': return KEY_D;
    case 'e':
    case 'E': return KEY_E;
    case 'f':
    case 'F': return KEY_F;
    case 'g':
    case 'G': return KEY_G;
    case 'h':
    case 'H': return KEY_H;
    case 'i':
    case 'I': return KEY_I;
    case 'j':
    case 'J': return KEY_J;
    case 'k':
    case 'K': return KEY_K;
    case 'l':
    case 'L': return KEY_L;
    case 'm':
    case 'M': return KEY_M;
    case 'n':
    case 'N': return KEY_N;
    case 'o':
    case 'O': return KEY_O;
    case 'p':
    case 'P': return KEY_P;
    case 'q':
    case 'Q': return KEY_Q;
    case 'r':
    case 'R': return KEY_R;
    case 's':
    case 'S': return KEY_S;
    case 't':
    case 'T': return KEY_T;
    case 'u':
    case 'U': return KEY_U;
    case 'v':
    case 'V': return KEY_V;
    case 'w':
    case 'W': return KEY_W;
    case 'x':
    case 'X': return KEY_X;
    case 'y':
    case 'Y': return KEY_Y;
    case 'z':
    case 'Z': return KEY_Z;
    case '\n':
    case '\r': return KEY_ENTER;
    case '\t': return KEY_TAB;
    case 0x7f:
    case '\b': return KEY_BACKSPACE;
    default: return -1;
    }
}

static void glx_update_key_expiry(void) {
    double now = GetTime();
    for (int i = 0; i < GLX_MAX_KEYS; ++i) {
        if (glx.key_down[i] && now >= glx.key_down_until[i]) {
            glx.key_down[i] = 0;
        }
    }
}

static void glx_poll_keyboard(void) {
    if (!glx.raw_stdin_enabled) {
        return;
    }

    for (;;) {
        unsigned char buf[32];
        ssize_t n = read(STDIN_FILENO, buf, sizeof(buf));
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            return;
        }
        if (n == 0) {
            return;
        }

        for (ssize_t i = 0; i < n; ++i) {
            unsigned char ch = buf[i];
            if (ch == 0x03) {
                glx_should_close = 1;
                continue;
            }

            if (ch == 0x1b) {
                if (i + 2 < n && buf[i + 1] == '[') {
                    switch (buf[i + 2]) {
                    case 'A': glx_mark_key_pressed(KEY_UP); break;
                    case 'B': glx_mark_key_pressed(KEY_DOWN); break;
                    case 'C': glx_mark_key_pressed(KEY_RIGHT); break;
                    case 'D': glx_mark_key_pressed(KEY_LEFT); break;
                    default: break;
                    }
                    i += 2;
                } else {
                    glx_mark_key_pressed(KEY_ESC);
                }
                continue;
            }

            glx_mark_key_pressed(glx_key_from_ascii(ch));
        }
    }
}

static int glx_enable_raw_stdin(void) {
    struct termios term;
    if (tcgetattr(STDIN_FILENO, &term) != 0) {
        return -1;
    }

    glx.old_termios = term;
    term.c_lflag &= ~(ICANON | ECHO);
    term.c_cc[VMIN] = 0;
    term.c_cc[VTIME] = 0;
    if (tcsetattr(STDIN_FILENO, TCSANOW, &term) != 0) {
        return -1;
    }

    glx.old_stdin_flags = fcntl(STDIN_FILENO, F_GETFL, 0);
    if (glx.old_stdin_flags < 0) {
        (void)tcsetattr(STDIN_FILENO, TCSANOW, &glx.old_termios);
        return -1;
    }
    if (fcntl(STDIN_FILENO, F_SETFL, glx.old_stdin_flags | O_NONBLOCK) < 0) {
        (void)tcsetattr(STDIN_FILENO, TCSANOW, &glx.old_termios);
        return -1;
    }

    glx.raw_stdin_enabled = 1;
    return 0;
}

static void glx_restore_stdin(void) {
    if (!glx.raw_stdin_enabled) {
        return;
    }
    (void)tcsetattr(STDIN_FILENO, TCSANOW, &glx.old_termios);
    if (glx.old_stdin_flags >= 0) {
        (void)fcntl(STDIN_FILENO, F_SETFL, glx.old_stdin_flags);
    }
    glx.raw_stdin_enabled = 0;
    glx.old_stdin_flags = -1;
}

static void fill_u32(uint32_t *dst, size_t count, uint32_t color) {
    size_t i = 0;
    for (; i + 8 <= count; i += 8) {
        dst[i + 0] = color;
        dst[i + 1] = color;
        dst[i + 2] = color;
        dst[i + 3] = color;
        dst[i + 4] = color;
        dst[i + 5] = color;
        dst[i + 6] = color;
        dst[i + 7] = color;
    }
    for (; i < count; ++i) {
        dst[i] = color;
    }
}

int InitGlx(void) {
    if (glx.initialized) {
        return 0;
    }

    glx.fb_fd = open(FB_PATH, O_RDWR);
    if (glx.fb_fd < 0) {
        perror("InitGlx: open /dev/fb0");
        return -1;
    }

    struct fb_var_screeninfo var = {0};
    struct fb_fix_screeninfo fix = {0};
    if (ioctl(glx.fb_fd, FBIOGET_VSCREENINFO, &var) < 0 ||
        ioctl(glx.fb_fd, FBIOGET_FSCREENINFO, &fix) < 0) {
        perror("InitGlx: ioctl framebuffer");
        CloseGlx();
        return -1;
    }

    if (var.xres == 0 || var.yres == 0 || var.bits_per_pixel != 32) {
        fprintf(stderr, "InitGlx: unsupported framebuffer mode %ux%u %u bpp\n",
                var.xres, var.yres, var.bits_per_pixel);
        CloseGlx();
        return -1;
    }

    size_t expected = (size_t)var.xres * (size_t)var.yres * sizeof(uint32_t);
    glx.map_bytes = (size_t)fix.smem_len;
    if (glx.map_bytes < expected) {
        fprintf(stderr, "InitGlx: framebuffer too small (%zu < %zu)\n", glx.map_bytes,
                expected);
        CloseGlx();
        return -1;
    }

    glx.pixels = mmap(NULL, glx.map_bytes, PROT_READ | PROT_WRITE, MAP_SHARED, glx.fb_fd, 0);
    if (glx.pixels == MAP_FAILED) {
        glx.pixels = NULL;
        perror("InitGlx: mmap framebuffer");
        CloseGlx();
        return -1;
    }

    glx.width = var.xres;
    glx.height = var.yres;
    glx.stride_pixels = var.xres;
    if (glx_enable_raw_stdin() != 0) {
        perror("InitGlx: raw stdin");
    }
    glx.frame_start = ts_now();
    glx.frame_time = glx.target_frame_seconds;
    glx.initialized = 1;
    glx_should_close = 0;

    (void)signal(SIGINT, glx_sigint);
    return 0;
}

void CloseGlx(void) {
    if (glx.pixels) {
        (void)munmap(glx.pixels, glx.map_bytes);
        glx.pixels = NULL;
    }
    if (glx.fb_fd >= 0) {
        (void)close(glx.fb_fd);
        glx.fb_fd = -1;
    }
    glx_restore_stdin();
    glx.map_bytes = 0;
    glx.width = 0;
    glx.height = 0;
    glx.stride_pixels = 0;
    memset(glx.key_down, 0, sizeof(glx.key_down));
    memset(glx.key_pressed, 0, sizeof(glx.key_pressed));
    memset(glx.key_down_until, 0, sizeof(glx.key_down_until));
    glx.initialized = 0;
}

int GlxShouldClose(void) {
    return glx_should_close ? 1 : 0;
}

void BeginDrawing(void) {
    glx_reset_pressed_keys();
    glx_poll_keyboard();
    glx_update_key_expiry();
    glx.frame_start = ts_now();
}

void EndDrawing(void) {
    if (!glx.initialized) {
        return;
    }

    (void)ioctl(glx.fb_fd, FBIOPAN_DISPLAY, 0);

    double elapsed = GetTime() - ts_seconds(glx.frame_start);
    double remaining = glx.target_frame_seconds - elapsed;
    if (remaining > 0.0) {
        sleep_seconds(remaining);
    }

    glx.frame_time = GetTime() - ts_seconds(glx.frame_start);
}

void ClearBackground(GlxColor color) {
    if (!glx.initialized || !glx.pixels) {
        return;
    }

    if (glx.stride_pixels == glx.width) {
        fill_u32(glx.pixels, (size_t)glx.width * (size_t)glx.height, color);
        return;
    }

    for (uint32_t y = 0; y < glx.height; ++y) {
        fill_u32(glx.pixels + (size_t)y * glx.stride_pixels, glx.width, color);
    }
}

void DrawRectangle(int x, int y, int width, int height, GlxColor color) {
    if (!glx.initialized || !glx.pixels || width <= 0 || height <= 0) {
        return;
    }

    int x0 = x;
    int y0 = y;
    int x1 = x + width;
    int y1 = y + height;

    if (x0 < 0) {
        x0 = 0;
    }
    if (y0 < 0) {
        y0 = 0;
    }
    if (x1 > (int)glx.width) {
        x1 = (int)glx.width;
    }
    if (y1 > (int)glx.height) {
        y1 = (int)glx.height;
    }
    if (x0 >= x1 || y0 >= y1) {
        return;
    }

    size_t row_count = (size_t)(x1 - x0);
    for (int yy = y0; yy < y1; ++yy) {
        uint32_t *row = glx.pixels + (size_t)yy * glx.stride_pixels + (size_t)x0;
        fill_u32(row, row_count, color);
    }
}

void SetTargetFPS(int fps) {
    if (fps <= 0) {
        glx.target_fps = 0;
        glx.target_frame_seconds = 0.0;
        return;
    }
    glx.target_fps = fps;
    glx.target_frame_seconds = 1.0 / (double)fps;
}

int IsKeyPressed(int key) {
    if (key < 0 || key >= GLX_MAX_KEYS) {
        return 0;
    }
    glx_poll_keyboard();
    glx_update_key_expiry();
    return glx.key_pressed[key] ? 1 : 0;
}

int IsKeyDown(int key) {
    if (key < 0 || key >= GLX_MAX_KEYS) {
        return 0;
    }
    glx_poll_keyboard();
    glx_update_key_expiry();
    return glx.key_down[key] ? 1 : 0;
}

int GetWidth(void) {
    return (int)glx.width;
}

int GetHeight(void) {
    return (int)glx.height;
}

int GetScreenWidth(void) {
    return GetWidth();
}

int GetScreenHeight(void) {
    return GetHeight();
}

float GetFrameTime(void) {
    return (float)glx.frame_time;
}
