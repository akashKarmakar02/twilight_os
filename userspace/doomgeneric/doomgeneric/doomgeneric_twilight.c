// doomgeneric for Twilight OS
//
// Uses /dev/fb0 for display and the Twilight input devices for input.
// Modeled after the soso and chip8 ports already in the Twilight OS userspace.

#include "doomkeys.h"
#include "doomgeneric.h"
#include "doomtype.h"

#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

// ---------------------------------------------------------------------------
// Framebuffer definitions (same as twc / chip8 in this OS)
// ---------------------------------------------------------------------------

#define FB_PATH "/dev/fb0"
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOGET_FSCREENINFO 0x4602
#define FBIOPAN_DISPLAY     0x4606
#define KEYBOARD_PATH "/dev/input/event0"
#define MOUSE_PATH    "/dev/input/mice"
#define EV_KEY 1

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

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

static int fb_fd = -1;
static uint32_t *fb_ptr = NULL;
static uint32_t screen_w = 0;
static uint32_t screen_h = 0;
static size_t   fb_size  = 0;
static int keyboard_fd = -1;
static int mouse_fd = -1;

struct input_event {
    int64_t tv_sec;
    int64_t tv_usec;
    uint16_t type;
    uint16_t code;
    int32_t value;
};

static unsigned char translateKey(uint16_t code)
{
    switch (code) {
    case 1: return KEY_ESCAPE;
    case 2: return '1';
    case 3: return '2';
    case 4: return '3';
    case 5: return '4';
    case 6: return '5';
    case 7: return '6';
    case 8: return '7';
    case 9: return '8';
    case 10: return '9';
    case 11: return '0';
    case 12: return KEY_MINUS;
    case 13: return KEY_EQUALS;
    case 14: return KEY_BACKSPACE;
    case 15: return KEY_TAB;
    case 16: return 'q';
    case 17: return KEY_UPARROW;    // W
    case 18: return 'e';
    case 19: return 'r';
    case 20: return 't';
    case 21: return 'y';
    case 22: return 'u';
    case 23: return 'i';
    case 24: return 'o';
    case 25: return 'p';
    case 26: return '[';
    case 27: return ']';
    case 28: return KEY_ENTER;
    case 29:
    case 97: return KEY_FIRE;
    case 30: return KEY_LEFTARROW;  // A
    case 31: return KEY_DOWNARROW;  // S
    case 32: return KEY_RIGHTARROW; // D
    case 33: return 'f';
    case 34: return 'g';
    case 35: return 'h';
    case 36: return 'j';
    case 37: return 'k';
    case 38: return 'l';
    case 39: return ';';
    case 40: return '\'';
    case 41: return '`';
    case 42:
    case 54: return KEY_RSHIFT;
    case 43: return '\\';
    case 44: return 'z';
    case 45: return 'x';
    case 46: return 'c';
    case 47: return 'v';
    case 48: return 'b';
    case 49: return 'n';
    case 50: return 'm';
    case 51: return ',';
    case 52: return '.';
    case 53: return '/';
    case 56:
    case 100: return KEY_RALT;
    case 57: return KEY_USE;
    case 58: return KEY_CAPSLOCK;
    case 59: return KEY_F1;
    case 60: return KEY_F2;
    case 61: return KEY_F3;
    case 62: return KEY_F4;
    case 63: return KEY_F5;
    case 64: return KEY_F6;
    case 65: return KEY_F7;
    case 66: return KEY_F8;
    case 67: return KEY_F9;
    case 68: return KEY_F10;
    case 87: return KEY_F11;
    case 88: return KEY_F12;
    case 102: return KEY_HOME;
    case 103: return KEY_UPARROW;
    case 104: return KEY_PGUP;
    case 105: return KEY_LEFTARROW;
    case 106: return KEY_RIGHTARROW;
    case 107: return KEY_END;
    case 108: return KEY_DOWNARROW;
    case 109: return KEY_PGDN;
    case 110: return KEY_INS;
    case 111: return KEY_DEL;
    default: return 0;
    }
}

static void discardPendingInput(void)
{
    struct input_event key_events[16];
    unsigned char mouse_packet[3];

    if (keyboard_fd >= 0) {
        while (read(keyboard_fd, key_events, sizeof(key_events)) > 0) {
        }
    }
    if (mouse_fd >= 0) {
        while (read(mouse_fd, mouse_packet, sizeof(mouse_packet)) > 0) {
        }
    }
}

// ---------------------------------------------------------------------------
// DG_Init — initialize framebuffer and keyboard
// ---------------------------------------------------------------------------

void DG_Init(void)
{
    keyboard_fd = open(KEYBOARD_PATH, O_RDONLY | O_NONBLOCK);
    if (keyboard_fd < 0) {
        perror("fbdoom: open /dev/input/event0");
        exit(1);
    }

    mouse_fd = open(MOUSE_PATH, O_RDONLY | O_NONBLOCK);
    discardPendingInput();

    // Open framebuffer
    fb_fd = open(FB_PATH, O_RDWR);
    if (fb_fd < 0) {
        perror("fbdoom: open /dev/fb0");
        exit(1);
    }

    struct fb_var_screeninfo vinfo;
    struct fb_fix_screeninfo finfo;
    memset(&vinfo, 0, sizeof(vinfo));
    memset(&finfo, 0, sizeof(finfo));

    if (ioctl(fb_fd, FBIOGET_VSCREENINFO, &vinfo) < 0) {
        perror("fbdoom: ioctl VSCREENINFO");
        exit(1);
    }
    if (ioctl(fb_fd, FBIOGET_FSCREENINFO, &finfo) < 0) {
        perror("fbdoom: ioctl FSCREENINFO");
        exit(1);
    }

    screen_w = vinfo.xres;
    screen_h = vinfo.yres;
    fb_size  = finfo.smem_len;

    printf("fbdoom: framebuffer %ux%u, %u bpp, %zu bytes\n",
           screen_w, screen_h, vinfo.bits_per_pixel, fb_size);

    fb_ptr = (uint32_t *)mmap(NULL, fb_size, PROT_READ | PROT_WRITE,
                              MAP_SHARED, fb_fd, 0);
    if (fb_ptr == MAP_FAILED) {
        perror("fbdoom: mmap framebuffer");
        exit(1);
    }

    // Clear screen to black
    memset(fb_ptr, 0, fb_size);
    ioctl(fb_fd, FBIOPAN_DISPLAY, 0);

    printf("fbdoom: initialized. DOOM res=%dx%d, screen=%ux%u\n",
           DOOMGENERIC_RESX, DOOMGENERIC_RESY, screen_w, screen_h);
}

// ---------------------------------------------------------------------------
// DG_DrawFrame — blit DG_ScreenBuffer to framebuffer, poll keyboard
// ---------------------------------------------------------------------------

void DG_DrawFrame(void)
{
    if (!fb_ptr)
        return;

    // Calculate scaling and centering
    // DG_ScreenBuffer is DOOMGENERIC_RESX * DOOMGENERIC_RESY pixels (uint32_t ARGB)
    int doom_w = DOOMGENERIC_RESX;
    int doom_h = DOOMGENERIC_RESY;

    // If screen is large enough, blit 1:1 centered
    // If screen is smaller, scale down
    int scale = 1;
    if ((uint32_t)doom_w <= screen_w && (uint32_t)doom_h <= screen_h) {
        // Screen is large enough — can we scale up?
        int sx = (int)screen_w / doom_w;
        int sy = (int)screen_h / doom_h;
        scale = sx < sy ? sx : sy;
        if (scale < 1) scale = 1;
    }

    int blit_w = doom_w * scale;
    int blit_h = doom_h * scale;
    int off_x = ((int)screen_w - blit_w) / 2;
    int off_y = ((int)screen_h - blit_h) / 2;
    if (off_x < 0) off_x = 0;
    if (off_y < 0) off_y = 0;

    if (scale == 1) {
        // Fast path: memcpy row by row
        int copy_w = doom_w;
        if (off_x + copy_w > (int)screen_w)
            copy_w = (int)screen_w - off_x;
        int copy_h = doom_h;
        if (off_y + copy_h > (int)screen_h)
            copy_h = (int)screen_h - off_y;

        for (int y = 0; y < copy_h; y++) {
            uint32_t *dst = fb_ptr + (off_y + y) * screen_w + off_x;
            uint32_t *src = (uint32_t *)DG_ScreenBuffer + y * doom_w;
            memcpy(dst, src, copy_w * sizeof(uint32_t));
        }
    } else {
        // Scaled blit
        for (int y = 0; y < doom_h; y++) {
            uint32_t *src_row = (uint32_t *)DG_ScreenBuffer + y * doom_w;
            for (int sy = 0; sy < scale; sy++) {
                int screen_y = off_y + y * scale + sy;
                if (screen_y < 0 || (uint32_t)screen_y >= screen_h)
                    continue;
                uint32_t *dst_row = fb_ptr + screen_y * screen_w;
                for (int x = 0; x < doom_w; x++) {
                    uint32_t pixel = src_row[x];
                    for (int sx2 = 0; sx2 < scale; sx2++) {
                        int screen_x = off_x + x * scale + sx2;
                        if (screen_x >= 0 && (uint32_t)screen_x < screen_w) {
                            dst_row[screen_x] = pixel;
                        }
                    }
                }
            }
        }
    }

    // Flush to display
    ioctl(fb_fd, FBIOPAN_DISPLAY, 0);

}

// ---------------------------------------------------------------------------
// DG_SleepMs
// ---------------------------------------------------------------------------

void DG_SleepMs(uint32_t ms)
{
    struct timespec ts;
    ts.tv_sec  = ms / 1000;
    ts.tv_nsec = (ms % 1000) * 1000000L;
    nanosleep(&ts, NULL);
}

// ---------------------------------------------------------------------------
// DG_GetTicksMs — monotonic clock
// ---------------------------------------------------------------------------

uint32_t DG_GetTicksMs(void)
{
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint32_t)(ts.tv_sec * 1000 + ts.tv_nsec / 1000000);
}

// ---------------------------------------------------------------------------
// DG_GetKey — read one input event
// ---------------------------------------------------------------------------

int DG_GetKey(int *pressed, unsigned char *doomKey)
{
    struct input_event event;

    for (;;) {
        unsigned char key;
        ssize_t bytes = read(keyboard_fd, &event, sizeof(event));

        if (bytes != sizeof(event))
            return 0;
        if (event.type != EV_KEY || event.value == 2)
            continue;

        key = translateKey(event.code);
        if (key != 0) {
            *pressed = event.value != 0;
            *doomKey = key;
            return 1;
        }
    }
}

int DG_GetMouse(int *buttons, int *dx, int *dy)
{
    unsigned char packet[3];

    if (mouse_fd < 0 || read(mouse_fd, packet, sizeof(packet)) != sizeof(packet))
        return 0;

    *buttons = packet[0] & 0x07;
    *dx = (int8_t)packet[1];
    *dy = (int8_t)packet[2];
    return 1;
}

// ---------------------------------------------------------------------------
// DG_SetWindowTitle — no-op on framebuffer
// ---------------------------------------------------------------------------

void DG_SetWindowTitle(const char *title)
{
    (void)title;
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

int main(int argc, char **argv)
{
    doomgeneric_Create(argc, argv);

    while (1) {
        doomgeneric_Tick();
    }

    return 0;
}
