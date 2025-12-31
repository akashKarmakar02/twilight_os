#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
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

enum { CHIP8_MEM_SIZE = 4096, CHIP8_REGS = 16, CHIP8_STACK = 16 };
enum { CHIP8_WIDTH = 64, CHIP8_HEIGHT = 32 };
enum { CHIP8_MAX_WIDTH = 128, CHIP8_MAX_HEIGHT = 64 };
enum { CHIP8_PLANE_SIZE = CHIP8_MAX_WIDTH * CHIP8_MAX_HEIGHT };
enum { CHIP8_PROG_START = 0x200 };
enum { CHIP8_KEYS = 16 };
enum { CHIP8_FONT_START = 0x000, CHIP8_FONT_SIZE = 16 * 5 };
enum { CHIP8_FONT_LARGE_START = CHIP8_FONT_START + CHIP8_FONT_SIZE };
enum { CHIP8_FONT_LARGE_SIZE = 16 * 10 };

typedef struct {
    uint8_t mem[CHIP8_MEM_SIZE];
    uint8_t V[CHIP8_REGS];
    uint16_t I;
    uint16_t pc;
    uint16_t stack[CHIP8_STACK];
    uint8_t sp;
    uint8_t delay;
    uint8_t sound;
    uint8_t gfx[CHIP8_PLANE_SIZE];
    uint8_t draw_flag;
    uint8_t wait_for_key;
    uint8_t wait_reg;
    uint8_t keys[CHIP8_KEYS];
    uint64_t key_hold_until[CHIP8_KEYS];
    uint8_t rpl_flags[16];
    uint16_t screen_width;
    uint16_t screen_height;
    uint8_t high_res;
    uint8_t halted;
} Chip8;

static struct termios term_old;
static int raw_installed = 0;

static const uint8_t base_fontset[CHIP8_FONT_SIZE] = {
    0xF0, 0x90, 0x90, 0x90, 0xF0, // 0
    0x20, 0x60, 0x20, 0x20, 0x70, // 1
    0xF0, 0x10, 0xF0, 0x80, 0xF0, // 2
    0xF0, 0x10, 0xF0, 0x10, 0xF0, // 3
    0x90, 0x90, 0xF0, 0x10, 0x10, // 4
    0xF0, 0x80, 0xF0, 0x10, 0xF0, // 5
    0xF0, 0x80, 0xF0, 0x90, 0xF0, // 6
    0xF0, 0x10, 0x20, 0x40, 0x40, // 7
    0xF0, 0x90, 0xF0, 0x90, 0xF0, // 8
    0xF0, 0x90, 0xF0, 0x10, 0xF0, // 9
    0xF0, 0x90, 0xF0, 0x90, 0x90, // A
    0xE0, 0x90, 0xE0, 0x90, 0xE0, // B
    0xF0, 0x80, 0x80, 0x80, 0xF0, // C
    0xE0, 0x90, 0x90, 0x90, 0xE0, // D
    0xF0, 0x80, 0xF0, 0x80, 0xF0, // E
    0xF0, 0x80, 0xF0, 0x80, 0x80  // F
};

static uint64_t ms_now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t now = (uint64_t)ts.tv_sec * 1000ull + ts.tv_nsec / 1000000ull;
    return now;
}

static void chip8_generate_large_fonts(uint8_t *dest) {
    for (int digit = 0; digit < 16; ++digit) {
        for (int row = 0; row < 5; ++row) {
            uint8_t pattern = base_fontset[digit * 5 + row];
            uint8_t expanded = 0;
            for (int col = 0; col < 4; ++col) {
                if (pattern & (0x80 >> col)) {
                    int bit = 7 - (col * 2);
                    expanded |= (1u << bit);
                    expanded |= (1u << (bit - 1));
                }
            }
            dest[digit * 10 + row * 2 + 0] = expanded;
            dest[digit * 10 + row * 2 + 1] = expanded;
        }
    }
}

static void chip8_clear_display(Chip8 *c) {
    memset(c->gfx, 0, sizeof(c->gfx));
    c->draw_flag = 1;
}

static void chip8_set_resolution(Chip8 *c, int high) {
    c->high_res = high ? 1 : 0;
    c->screen_width = high ? CHIP8_MAX_WIDTH : CHIP8_WIDTH;
    c->screen_height = high ? CHIP8_MAX_HEIGHT : CHIP8_HEIGHT;
    chip8_clear_display(c);
}

static void chip8_reset(Chip8 *c) {
    memset(c, 0, sizeof(*c));
    memcpy(c->mem + CHIP8_FONT_START, base_fontset, CHIP8_FONT_SIZE);
    chip8_generate_large_fonts(c->mem + CHIP8_FONT_LARGE_START);
    chip8_set_resolution(c, 0);
    c->pc = CHIP8_PROG_START;
}

static int chip8_load_rom(Chip8 *c, const char *path) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        perror("open ROM");
        return -1;
    }
    uint8_t *dest = c->mem + CHIP8_PROG_START;
    ssize_t total = 0;
    while (1) {
        ssize_t n = read(fd, dest + total, CHIP8_MEM_SIZE - CHIP8_PROG_START - total);
        if (n < 0) {
            perror("read ROM");
            close(fd);
            return -1;
        }
        if (n == 0) {
            break;
        }
        total += n;
        if ((size_t)total >= CHIP8_MEM_SIZE - CHIP8_PROG_START) {
            break;
        }
    }
    close(fd);
    if (total <= 0) {
        fprintf(stderr, "ROM file empty\n");
        return -1;
    }
    return 0;
}

static void chip8_set_key(Chip8 *c, int key, int pressed, uint64_t now) {
    if (key < 0 || key >= CHIP8_KEYS) {
        return;
    }
    if (pressed) {
        c->key_hold_until[key] = now + 3;
        c->keys[key] = 1;
        if (c->wait_for_key) {
            c->V[c->wait_reg] = (uint8_t)key;
            c->wait_for_key = 0;
        }
    }
}

static void chip8_update_keys(Chip8 *c, uint64_t now) {
    for (int i = 0; i < CHIP8_KEYS; ++i) {
        if (c->keys[i] && now > c->key_hold_until[i]) {
            c->keys[i] = 0;
        }
    }
}

static void chip8_scroll_down(Chip8 *c, uint8_t rows) {
    uint16_t shift = rows % c->screen_height;
    if (shift == 0) {
        return;
    }
    if (shift >= c->screen_height) {
        chip8_clear_display(c);
        return;
    }
    uint16_t w = c->screen_width;
    uint16_t h = c->screen_height;

    for (int y = (int)h - 1; y >= 0; --y) {
        int src = y - (int)shift;
        for (uint16_t x = 0; x < w; ++x) {
            size_t dst_idx = (size_t)y * w + x;
            if (src >= 0) {
                c->gfx[dst_idx] = c->gfx[(size_t)src * w + x];
            } else {
                c->gfx[dst_idx] = 0;
            }
        }
    }
    c->draw_flag = 1;
}

static void chip8_scroll_right(Chip8 *c) {
    uint16_t w = c->screen_width;
    uint16_t h = c->screen_height;
    if (w <= 4) {
        chip8_clear_display(c);
        return;
    }
    for (uint16_t y = 0; y < h; ++y) {
        size_t row_off = (size_t)y * w;
        for (int x = (int)w - 1; x >= 0; --x) {
            int src = x - 4;
            size_t dst = row_off + (size_t)x;
            c->gfx[dst] = (src >= 0) ? c->gfx[row_off + (size_t)src] : 0;
        }
    }
    c->draw_flag = 1;
}

static void chip8_scroll_left(Chip8 *c) {
    uint16_t w = c->screen_width;
    uint16_t h = c->screen_height;
    if (w <= 4) {
        chip8_clear_display(c);
        return;
    }
    for (uint16_t y = 0; y < h; ++y) {
        size_t row_off = (size_t)y * w;
        for (uint16_t x = 0; x < w; ++x) {
            size_t dst = row_off + x;
            uint16_t src = x + 4;
            c->gfx[dst] = (src < w) ? c->gfx[row_off + src] : 0;
        }
    }
    c->draw_flag = 1;
}

static void chip8_draw_sprite(Chip8 *c, uint8_t x, uint8_t y, uint8_t height) {
    uint8_t rows = height ? height : 16;
    uint8_t sprite_width = 8;
    uint8_t row_bytes = 1;

    if (height == 0) {
        if (c->high_res) {
            sprite_width = 16;
            row_bytes = 2;
        }
        rows = 16;
    }

    c->V[0xF] = 0;
    for (uint8_t row = 0; row < rows; ++row) {
        uint16_t sprite_bits;
        if (row_bytes == 1) {
            sprite_bits = c->mem[c->I + row];
        } else {
            size_t idx = (size_t)c->I + (size_t)row * row_bytes;
            sprite_bits = ((uint16_t)c->mem[idx] << 8) | c->mem[idx + 1];
        }

        for (uint8_t col = 0; col < sprite_width; ++col) {
            uint16_t mask = (row_bytes == 1) ? (0x80 >> col) : (0x8000 >> col);
            if ((sprite_bits & mask) == 0) {
                continue;
            }
            uint16_t px = (x + col) % c->screen_width;
            uint16_t py = (y + row) % c->screen_height;
            size_t index = (size_t)py * c->screen_width + px;
            if (c->gfx[index]) {
                c->V[0xF] = 1;
            }
            c->gfx[index] ^= 1;
        }
    }
    c->draw_flag = 1;
}

static void chip8_step(Chip8 *c) {
    if (c->wait_for_key || c->halted) {
        return;
    }
    uint16_t opcode = (c->mem[c->pc] << 8) | c->mem[c->pc + 1];
    c->pc += 2;
    uint16_t nnn = opcode & 0x0FFF;
    uint8_t x = (opcode >> 8) & 0x0F;
    uint8_t y = (opcode >> 4) & 0x0F;
    uint8_t kk = opcode & 0xFF;
    uint8_t n = opcode & 0x0F;

    switch (opcode & 0xF000) {
    case 0x0000:
        if ((opcode & 0xF0F0) == 0x00C0) {
            chip8_scroll_down(c, opcode & 0x000F);
        } else {
            switch (opcode & 0x00FF) {
            case 0xE0:
                chip8_clear_display(c);
                break;
            case 0xEE:
                if (c->sp > 0) {
                    --c->sp;
                    c->pc = c->stack[c->sp];
                }
                break;
            case 0xFB:
                chip8_scroll_right(c);
                break;
            case 0xFC:
                chip8_scroll_left(c);
                break;
            case 0xFD:
                c->halted = 1;
                break;
            case 0xFE:
                chip8_set_resolution(c, 0);
                break;
            case 0xFF:
                chip8_set_resolution(c, 1);
                break;
            default:
                break;
            }
        }
        break;
    case 0x1000:
        c->pc = nnn;
        break;
    case 0x2000:
        if (c->sp < CHIP8_STACK) {
            c->stack[c->sp++] = c->pc;
            c->pc = nnn;
        }
        break;
    case 0x3000:
        if (c->V[x] == kk) {
            c->pc += 2;
        }
        break;
    case 0x4000:
        if (c->V[x] != kk) {
            c->pc += 2;
        }
        break;
    case 0x5000:
        if ((opcode & 0x000F) == 0 && c->V[x] == c->V[y]) {
            c->pc += 2;
        }
        break;
    case 0x6000:
        c->V[x] = kk;
        break;
    case 0x7000:
        c->V[x] += kk;
        break;
    case 0x8000: {
        switch (opcode & 0x000F) {
        case 0x0: c->V[x] = c->V[y]; break;
        case 0x1: c->V[x] |= c->V[y]; break;
        case 0x2: c->V[x] &= c->V[y]; break;
        case 0x3: c->V[x] ^= c->V[y]; break;
        case 0x4: {
            uint16_t sum = c->V[x] + c->V[y];
            c->V[0xF] = sum > 0xFF;
            c->V[x] = sum & 0xFF;
        } break;
        case 0x5:
            c->V[0xF] = c->V[x] > c->V[y];
            c->V[x] -= c->V[y];
            break;
        case 0x6:
            c->V[0xF] = c->V[x] & 0x1;
            c->V[x] >>= 1;
            break;
        case 0x7:
            c->V[0xF] = c->V[y] > c->V[x];
            c->V[x] = c->V[y] - c->V[x];
            break;
        case 0xE:
            c->V[0xF] = (c->V[x] & 0x80) != 0;
            c->V[x] <<= 1;
            break;
        }
    } break;
    case 0x9000:
        if ((opcode & 0x000F) == 0 && c->V[x] != c->V[y]) {
            c->pc += 2;
        }
        break;
    case 0xA000:
        c->I = nnn;
        break;
    case 0xB000:
        c->pc = nnn + c->V[0];
        break;
    case 0xC000:
        c->V[x] = (rand() & 0xFF) & kk;
        break;
    case 0xD000:
        chip8_draw_sprite(c, c->V[x], c->V[y], n);
        break;
    case 0xE000:
        if ((opcode & 0x00FF) == 0x9E) {
            if (c->keys[c->V[x] & 0xF]) {
                c->pc += 2;
            }
        } else if ((opcode & 0x00FF) == 0xA1) {
            if (!c->keys[c->V[x] & 0xF]) {
                c->pc += 2;
            }
        }
        break;
    case 0xF000:
        switch (opcode & 0x00FF) {
        case 0x07: c->V[x] = c->delay; break;
        case 0x0A:
            c->wait_for_key = 1;
            c->wait_reg = x;
            break;
        case 0x15: c->delay = c->V[x]; break;
        case 0x18: c->sound = c->V[x]; break;
        case 0x1E: {
            uint16_t sum = c->I + c->V[x];
            c->V[0xF] = sum > 0x0FFF;
            c->I = sum & 0x0FFF;
        } break;
        case 0x29:
            c->I = CHIP8_FONT_START + (c->V[x] & 0xF) * 5;
            break;
        case 0x30:
            c->I = CHIP8_FONT_LARGE_START + (c->V[x] & 0xF) * 10;
            break;
        case 0x33: {
            uint8_t val = c->V[x];
            c->mem[c->I + 0] = val / 100;
            c->mem[c->I + 1] = (val / 10) % 10;
            c->mem[c->I + 2] = val % 10;
        } break;
        case 0x55:
            for (uint8_t i = 0; i <= x; ++i) {
                c->mem[c->I + i] = c->V[i];
            }
            c->I += x + 1;
            break;
        case 0x65:
            for (uint8_t i = 0; i <= x; ++i) {
                c->V[i] = c->mem[c->I + i];
            }
            c->I += x + 1;
            break;
        case 0x75:
            for (uint8_t i = 0; i <= x && i < 16; ++i) {
                c->rpl_flags[i] = c->V[i];
            }
            break;
        case 0x85:
            for (uint8_t i = 0; i <= x && i < 16; ++i) {
                c->V[i] = c->rpl_flags[i];
            }
            break;
        default:
            break;
        }
        break;
    default:
        break;
    }
}

static int enable_raw_input(void) {
    struct termios t;
    if (tcgetattr(STDIN_FILENO, &t) != 0) {
        return -1;
    }
    term_old = t;
    t.c_lflag &= ~(ICANON | ECHO);
    t.c_cc[VMIN] = 0;
    t.c_cc[VTIME] = 0;
    if (tcsetattr(STDIN_FILENO, TCSANOW, &t) != 0) {
        return -1;
    }
    int fl = fcntl(STDIN_FILENO, F_GETFL, 0);
    if (fl < 0) {
        return -1;
    }
    if (fcntl(STDIN_FILENO, F_SETFL, fl | O_NONBLOCK) < 0) {
        return -1;
    }
    raw_installed = 1;
    return 0;
}

static void restore_terminal(void) {
    tcsetattr(STDIN_FILENO, TCSANOW, &term_old);
    raw_installed = 0;
}

static int key_char_to_chip8(int ch) {
    switch (ch) {
    case '1': return 0x1;
    case '2': return 0x2;
    case '3': return 0x3;
    case '4': return 0xC;
    case 'q':
    case 'Q': return 0x4;
    case 'w':
    case 'W': return 0x5;
    case 'e':
    case 'E': return 0x6;
    case 'r':
    case 'R': return 0xD;
    case 'a':
    case 'A': return 0x7;
    case 's':
    case 'S': return 0x8;
    case 'd':
    case 'D': return 0x9;
    case 'f':
    case 'F': return 0xE;
    case 'z':
    case 'Z': return 0xA;
    case 'x':
    case 'X': return 0x0;
    case 'c':
    case 'C': return 0xB;
    case 'v':
    case 'V': return 0xF;
    default:
        return -1;
    }
}

static int process_input(Chip8 *c) {
    struct pollfd pfd = {
        .fd = STDIN_FILENO,
        .events = POLLIN,
        .revents = 0,
    };
    int ret = poll(&pfd, 1, 0);
    if (ret <= 0) {
        return 0;
    }

    unsigned char buf[1];
    ssize_t n = read(STDIN_FILENO, buf, sizeof(buf));
    if (n <= 0) {
        return 0;
    }
    uint64_t now = ms_now();
    for (ssize_t i = 0; i < n; ++i) {
        int b = buf[i];
        if (b == 0x03) { // Ctrl+C
            return -1;
        }
        if (b == 0x1b) {
            continue;
        }
        int mapped = key_char_to_chip8(b);
        if (mapped >= 0) {
            chip8_set_key(c, mapped, 1, now);
            c->draw_flag = 1;
        }
    }
    return 0;
}

static void draw_rect(uint32_t *buf, uint32_t width, uint32_t height,
                      int x0, int y0, int w, int h, uint32_t color) {
    if (w <= 0 || h <= 0 || x0 >= (int)width || y0 >= (int)height) {
        return;
    }
    int start_x = x0 < 0 ? 0 : x0;
    int start_y = y0 < 0 ? 0 : y0;
    int end_x = x0 + w;
    int end_y = y0 + h;
    if (end_x > (int)width) end_x = (int)width;
    if (end_y > (int)height) end_y = (int)height;
    for (int y = start_y; y < end_y; ++y) {
        uint32_t *row = buf + y * width;
        for (int x = start_x; x < end_x; ++x) {
            row[x] = color;
        }
    }
}

static void render_framebuffer(uint32_t *fb, uint32_t width, uint32_t height, const Chip8 *c) {
    size_t total = (size_t)width * (size_t)height;
    uint32_t bg = 0xFF10121A;
    uint32_t fg = 0xFF50FA7B;
    for (size_t i = 0; i < total; ++i) {
        fb[i] = bg;
    }

    int scale_x = width / c->screen_width;
    int scale_y = height / c->screen_height;
    int scale = scale_x < scale_y ? scale_x : scale_y;
    if (scale < 1) {
        scale = 1;
    }
    int disp_w = scale * c->screen_width;
    int disp_h = scale * c->screen_height;
    int off_x = (int)(width - disp_w) / 2;
    int off_y = (int)(height - disp_h) / 2;

    for (uint16_t y = 0; y < c->screen_height; ++y) {
        for (uint16_t x = 0; x < c->screen_width; ++x) {
            size_t idx = (size_t)y * c->screen_width + x;
            uint32_t color = c->gfx[idx] ? fg : bg;
            if (!c->gfx[idx]) {
                continue;
            }
            draw_rect(fb, width, height,
                      off_x + x * scale,
                      off_y + y * scale,
                      scale,
                      scale,
                      color);
        }
    }
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "Usage: %s /path/to/rom\n", argv[0]);
        return 1;
    }

    if (enable_raw_input() != 0) {
        perror("termios");
    }

    int fb = open(FB_PATH, O_RDWR);
    if (fb < 0) {
        perror("open /dev/fb0");
        restore_terminal();
        return 1;
    }

    struct fb_var_screeninfo var = {0};
    struct fb_fix_screeninfo fix = {0};
    if (ioctl(fb, FBIOGET_VSCREENINFO, &var) < 0 ||
        ioctl(fb, FBIOGET_FSCREENINFO, &fix) < 0) {
        perror("ioctl fb");
        close(fb);
        restore_terminal();
        return 1;
    }

    size_t frame_bytes = (size_t)fix.smem_len;
    uint32_t *frame = mmap(NULL, frame_bytes, PROT_READ | PROT_WRITE, MAP_SHARED, fb, 0);
    if (frame == MAP_FAILED) {
        perror("mmap framebuffer");
        close(fb);
        restore_terminal();
        return 1;
    }
    memset(frame, 0, frame_bytes);

    Chip8 chip;
    chip8_reset(&chip);
    if (chip8_load_rom(&chip, argv[1]) != 0) {
        munmap(frame, frame_bytes);
        close(fb);
        restore_terminal();
        return 1;
    }

    uint64_t last_timer = ms_now();
    const uint64_t timer_interval = 1000 / 60;
    const int cycles_per_frame = 10;
    struct timespec sleep_ts = {.tv_sec = 0, .tv_nsec = 1 * 1000 * 10 };
    int running = 1;

    while (running) {
        if (process_input(&chip) < 0) {
            running = 0;
            break;
        }

        if (chip.wait_for_key) {
            nanosleep(&sleep_ts, NULL);
            continue;
        }

        for (int i = 0; i < cycles_per_frame && !chip.halted; ++i) {
            chip8_step(&chip);
        }
        if (chip.halted) {
            running = 0;
        }

        uint64_t now = ms_now();
        chip8_update_keys(&chip, now);
        if (now - last_timer >= timer_interval) {
            if (chip.delay > 0) chip.delay--;
            if (chip.sound > 0) chip.sound--;
            last_timer = now;
        }

        if (chip.draw_flag) {
            render_framebuffer(frame, var.xres, var.yres, &chip);
            if (ioctl(fb, FBIOPAN_DISPLAY, NULL) < 0) {
                perror("fb flush");
                running = 0;
                break;
            }
            chip.draw_flag = 0;
        }

        nanosleep(&sleep_ts, NULL);
    }

    memset(frame, 0, frame_bytes);
    ioctl(fb, FBIOPAN_DISPLAY, NULL);
    munmap(frame, frame_bytes);
    close(fb);
    restore_terminal();
    return 0;
}
