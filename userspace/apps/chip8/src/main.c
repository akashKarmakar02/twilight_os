#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#define FB_PATH "/dev/fb0"
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOGET_FSCREENINFO 0x4602

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
enum { CHIP8_PROG_START = 0x200 };
enum { CHIP8_KEYS = 16 };

typedef struct {
    uint8_t mem[CHIP8_MEM_SIZE];
    uint8_t V[CHIP8_REGS];
    uint16_t I;
    uint16_t pc;
    uint16_t stack[CHIP8_STACK];
    uint8_t sp;
    uint8_t delay;
    uint8_t sound;
    uint8_t gfx[CHIP8_WIDTH * CHIP8_HEIGHT];
    uint8_t draw_flag;
    uint8_t wait_for_key;
    uint8_t wait_reg;
    uint8_t keys[CHIP8_KEYS];
    uint64_t key_hold_until[CHIP8_KEYS];
} Chip8;

static struct termios term_old;
static int raw_installed = 0;

static uint64_t ms_now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t now = (uint64_t)ts.tv_sec * 1000ull + ts.tv_nsec / 1000000ull;
    return now;
}

static void chip8_reset(Chip8 *c) {
    memset(c, 0, sizeof(*c));
    static const uint8_t fontset[80] = {
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
    memcpy(c->mem, fontset, sizeof(fontset));
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
        c->key_hold_until[key] = now + 10;
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

static void chip8_draw_sprite(Chip8 *c, uint8_t x, uint8_t y, uint8_t height) {
    c->V[0xF] = 0;
    for (uint8_t row = 0; row < height; ++row) {
        uint8_t sprite_byte = c->mem[c->I + row];
        for (uint8_t col = 0; col < 8; ++col) {
            if ((sprite_byte & (0x80 >> col)) == 0) {
                continue;
            }
            uint8_t px = (x + col) % CHIP8_WIDTH;
            uint8_t py = (y + row) % CHIP8_HEIGHT;
            size_t index = py * CHIP8_WIDTH + px;
            if (c->gfx[index]) {
                c->V[0xF] = 1;
            }
            c->gfx[index] ^= 1;
        }
    }
    c->draw_flag = 1;
}

static void chip8_step(Chip8 *c) {
    if (c->wait_for_key) {
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
        if (opcode == 0x00E0) {
            memset(c->gfx, 0, sizeof(c->gfx));
            c->draw_flag = 1;
        } else if (opcode == 0x00EE) {
            if (c->sp > 0) {
                --c->sp;
                c->pc = c->stack[c->sp];
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
        case 0x1E: c->I += c->V[x]; break;
        case 0x29: c->I = (c->V[x] & 0xF) * 5; break;
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

    int scale_x = width / CHIP8_WIDTH;
    int scale_y = height / CHIP8_HEIGHT;
    int scale = scale_x < scale_y ? scale_x : scale_y;
    if (scale < 1) {
        scale = 1;
    }
    int disp_w = scale * CHIP8_WIDTH;
    int disp_h = scale * CHIP8_HEIGHT;
    int off_x = (int)(width - disp_w) / 2;
    int off_y = (int)(height - disp_h) / 2;

    for (int y = 0; y < CHIP8_HEIGHT; ++y) {
        for (int x = 0; x < CHIP8_WIDTH; ++x) {
            uint32_t color = c->gfx[y * CHIP8_WIDTH + x] ? fg : bg;
            if (!c->gfx[y * CHIP8_WIDTH + x]) {
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

static int write_all(int fd, const void *buf, size_t len);

static int write_all(int fd, const void *buf, size_t len) {
    const uint8_t *p = buf;
    size_t done = 0;
    while (done < len) {
        ssize_t n = write(fd, p + done, len - done);
        if (n < 0) {
            if (errno == EINTR) {
                continue;
            }
            return -1;
        }
        if (n == 0) {
            break;
        }
        done += (size_t)n;
    }
    return done == len ? 0 : -1;
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

    size_t pixel_count = (size_t)var.xres * (size_t)var.yres;
    uint32_t *frame = calloc(pixel_count, sizeof(uint32_t));
    if (!frame) {
        perror("calloc frame");
        close(fb);
        restore_terminal();
        return 1;
    }

    Chip8 chip;
    chip8_reset(&chip);
    if (chip8_load_rom(&chip, argv[1]) != 0) {
        free(frame);
        close(fb);
        restore_terminal();
        return 1;
    }

    uint64_t last_timer = ms_now();
    const uint64_t timer_interval = 1000 / 60;
    const int cycles_per_frame = 10;
    struct timespec sleep_ts = {.tv_sec = 0, .tv_nsec = 1 * 1000 * 1000 };
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

        for (int i = 0; i < cycles_per_frame; ++i) {
            chip8_step(&chip);
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
            if (lseek(fb, 0, SEEK_SET) == 0) {
                write_all(fb, frame, pixel_count * sizeof(uint32_t));
            }
            chip.draw_flag = 0;
        }

        nanosleep(&sleep_ts, NULL);
    }

    memset(frame, 0, pixel_count * sizeof(uint32_t));
    lseek(fb, 0, SEEK_SET);
    write_all(fb, frame, pixel_count * sizeof(uint32_t));
    free(frame);
    close(fb);
    restore_terminal();
    return 0;
}
