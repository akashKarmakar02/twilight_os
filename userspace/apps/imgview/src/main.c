#define _POSIX_C_SOURCE 200809L

#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <dirent.h>
#include <setjmp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>
#include <jpeglib.h>

#define FB_PATH "/dev/fb0"
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOGET_FSCREENINFO 0x4602
#define FBIOPAN_DISPLAY 0x4606
#define HUD_HEIGHT 28u
#define VIEW_MARGIN_BOTTOM 10u
#define HUD_BG_COLOR 0xFF0B1019

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

// Minimal BMP headers (uncompressed 24/32-bit only)
typedef struct __attribute__((packed)) {
    uint16_t bfType;
    uint32_t bfSize;
    uint16_t bfReserved1;
    uint16_t bfReserved2;
    uint32_t bfOffBits;
} BmpFileHeader;

typedef struct __attribute__((packed)) {
    uint32_t biSize;
    int32_t biWidth;
    int32_t biHeight; // negative = top-down
    uint16_t biPlanes;
    uint16_t biBitCount; // 24 or 32
    uint32_t biCompression; // 0 = BI_RGB
    uint32_t biSizeImage;
    int32_t biXPelsPerMeter;
    int32_t biYPelsPerMeter;
    uint32_t biClrUsed;
    uint32_t biClrImportant;
} BmpInfoHeader;

typedef struct {
    uint32_t width;
    uint32_t height;
    uint32_t *pixels; // ARGB8888
} Image;

enum viewer_key {
    VK_BACKSPACE = 127,
    VK_ARROW_LEFT = 1000,
    VK_ARROW_RIGHT,
    VK_ARROW_UP,
    VK_ARROW_DOWN,
};

static struct termios g_orig_termios;
static int g_raw_enabled = 0;

static void disable_raw_mode(void) {
    if (!g_raw_enabled) {
        return;
    }
    (void)tcsetattr(STDIN_FILENO, TCSAFLUSH, &g_orig_termios);
    g_raw_enabled = 0;
}

static int enable_raw_mode(void) {
    if (tcgetattr(STDIN_FILENO, &g_orig_termios) == -1) {
        return -1;
    }
    struct termios raw = g_orig_termios;
    raw.c_iflag &= ~(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
    raw.c_oflag &= ~(OPOST);
    raw.c_cflag |= (CS8);
    raw.c_lflag &= ~(ECHO | ICANON | IEXTEN | ISIG);
    raw.c_cc[VMIN] = 0;
    raw.c_cc[VTIME] = 1;
    if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw) == -1) {
        return -1;
    }
    g_raw_enabled = 1;
    atexit(disable_raw_mode);
    return 0;
}

static int read_key(void) {
    int nread;
    unsigned char c;
    while ((nread = (int)read(STDIN_FILENO, &c, 1)) != 1) {
        if (nread == -1 && errno != EAGAIN) {
            return -1;
        }
    }

    if (c == '\x1b') {
        unsigned char seq[3];
        if (read(STDIN_FILENO, &seq[0], 1) != 1) return '\x1b';
        if (read(STDIN_FILENO, &seq[1], 1) != 1) return '\x1b';

        if (seq[0] == '[') {
            if (seq[1] >= '0' && seq[1] <= '9') {
                if (read(STDIN_FILENO, &seq[2], 1) != 1) return '\x1b';
                if (seq[2] == '~') {
                    switch (seq[1]) {
                        case '1': return VK_ARROW_LEFT;
                        case '4': return VK_ARROW_RIGHT;
                    }
                }
            } else {
                switch (seq[1]) {
                    case 'A': return VK_ARROW_UP;
                    case 'B': return VK_ARROW_DOWN;
                    case 'C': return VK_ARROW_RIGHT;
                    case 'D': return VK_ARROW_LEFT;
                }
            }
        }
        return '\x1b';
    }
    return c;
}

static void free_image(Image *img) {
    if (img && img->pixels) {
        free(img->pixels);
        img->pixels = NULL;
    }
}

static int load_bmp(const char *path, Image *out) {
    memset(out, 0, sizeof(*out));

    FILE *f = fopen(path, "rb");
    if (!f) {
        perror("open image");
        return -1;
    }

    BmpFileHeader fh;
    if (fread(&fh, 1, sizeof(fh), f) != sizeof(fh)) {
        fprintf(stderr, "imgview: failed to read BMP file header\n");
        fclose(f);
        return -1;
    }
    if (fh.bfType != 0x4D42) { // 'BM'
        fprintf(stderr, "imgview: not a BMP file\n");
        fclose(f);
        return -1;
    }

    BmpInfoHeader ih;
    if (fread(&ih, 1, sizeof(ih), f) != sizeof(ih)) {
        fprintf(stderr, "imgview: failed to read BMP info header\n");
        fclose(f);
        return -1;
    }

    if (ih.biPlanes != 1) {
        fprintf(stderr, "imgview: unsupported BMP planes (%u)\n", ih.biPlanes);
        fclose(f);
        return -1;
    }
    if (ih.biBitCount != 24 && ih.biBitCount != 32) {
        fprintf(stderr, "imgview: only 24-bit and 32-bit BMP images are supported\n");
        fclose(f);
        return -1;
    }
    if (ih.biCompression != 0) {
        fprintf(stderr, "imgview: compressed BMPs are not supported\n");
        fclose(f);
        return -1;
    }

    int64_t w_signed = ih.biWidth;
    int64_t h_signed = ih.biHeight;
    if (w_signed <= 0 || h_signed == 0) {
        fprintf(stderr, "imgview: invalid BMP dimensions\n");
        fclose(f);
        return -1;
    }
    uint32_t width = (uint32_t)w_signed;
    uint32_t height = (uint32_t)((h_signed < 0) ? -h_signed : h_signed);
    int top_down = h_signed < 0;

    size_t pixel_count = (size_t)width * (size_t)height;
    if (pixel_count == 0 || pixel_count > (SIZE_MAX / sizeof(uint32_t))) {
        fprintf(stderr, "imgview: BMP image too large\n");
        fclose(f);
        return -1;
    }

    if (fseek(f, (long)fh.bfOffBits, SEEK_SET) != 0) {
        perror("fseek");
        fclose(f);
        return -1;
    }

    uint32_t bpp = ih.biBitCount;
    size_t bytes_per_pixel = bpp / 8;
    size_t row_stride = ((size_t)width * bpp + 31) / 32 * 4;

    uint8_t *row_buf = malloc(row_stride);
    if (!row_buf) {
        fclose(f);
        return -1;
    }

    uint32_t *pixels = malloc(pixel_count * sizeof(uint32_t));
    if (!pixels) {
        free(row_buf);
        fclose(f);
        return -1;
    }

    for (uint32_t row = 0; row < height; ++row) {
        if (fread(row_buf, 1, row_stride, f) != row_stride) {
            fprintf(stderr, "imgview: unexpected EOF while reading BMP pixel data\n");
            free(row_buf);
            free(pixels);
            fclose(f);
            return -1;
        }

        uint32_t dest_row = top_down ? row : (height - 1 - row);
        uint32_t *dest = pixels + dest_row * width;

        for (uint32_t col = 0; col < width; ++col) {
            size_t idx = (size_t)col * bytes_per_pixel;
            uint8_t b = row_buf[idx + 0];
            uint8_t g = row_buf[idx + 1];
            uint8_t r = row_buf[idx + 2];
            uint8_t a = (bpp == 32) ? row_buf[idx + 3] : 0xFF;
            dest[col] = ((uint32_t)a << 24) | ((uint32_t)r << 16) |
                        ((uint32_t)g << 8) | (uint32_t)b;
        }
    }

    free(row_buf);
    fclose(f);

    out->width = width;
    out->height = height;
    out->pixels = pixels;
    return 0;
}

struct jpeg_error_state {
    struct jpeg_error_mgr pub;
    jmp_buf jmp;
};

static void jpeg_error_exit(j_common_ptr cinfo) {
    struct jpeg_error_state *st = (struct jpeg_error_state *)cinfo->err;
    longjmp(st->jmp, 1);
}

static int load_jpeg(const char *path, Image *out) {
    memset(out, 0, sizeof(*out));

    FILE *f = fopen(path, "rb");
    if (!f) {
        perror("open image");
        return -1;
    }

    struct jpeg_decompress_struct cinfo;
    struct jpeg_error_state jerr;

    cinfo.err = jpeg_std_error(&jerr.pub);
    jerr.pub.error_exit = jpeg_error_exit;

    if (setjmp(jerr.jmp)) {
        jpeg_destroy_decompress(&cinfo);
        fclose(f);
        free_image(out);
        fprintf(stderr, "imgview: failed to decode JPEG\n");
        return -1;
    }

    jpeg_create_decompress(&cinfo);
    jpeg_stdio_src(&cinfo, f);
    (void)jpeg_read_header(&cinfo, TRUE);
    (void)jpeg_start_decompress(&cinfo);

    uint32_t width = cinfo.output_width;
    uint32_t height = cinfo.output_height;
    int comps = cinfo.output_components;

    if (width == 0 || height == 0 || (comps != 1 && comps != 3 && comps != 4)) {
        jpeg_destroy_decompress(&cinfo);
        fclose(f);
        fprintf(stderr, "imgview: unsupported JPEG output format\n");
        return -1;
    }

    size_t pixel_count = (size_t)width * (size_t)height;
    if (pixel_count == 0 || pixel_count > (SIZE_MAX / sizeof(uint32_t))) {
        jpeg_destroy_decompress(&cinfo);
        fclose(f);
        fprintf(stderr, "imgview: JPEG image too large\n");
        return -1;
    }

    uint32_t *pixels = malloc(pixel_count * sizeof(uint32_t));
    if (!pixels) {
        jpeg_destroy_decompress(&cinfo);
        fclose(f);
        return -1;
    }

    size_t row_stride = (size_t)width * (size_t)comps;
    JSAMPARRAY row = (*cinfo.mem->alloc_sarray)((j_common_ptr)&cinfo, JPOOL_IMAGE,
                                                 (JDIMENSION)row_stride, 1);

    uint32_t y = 0;
    while (cinfo.output_scanline < cinfo.output_height) {
        (void)jpeg_read_scanlines(&cinfo, row, 1);
        uint8_t *src = row[0];
        uint32_t *dst = pixels + (size_t)y * width;

        for (uint32_t x = 0; x < width; ++x) {
            uint8_t r, g, b;
            if (comps == 1) {
                r = g = b = src[x];
            } else {
                size_t idx = (size_t)x * (size_t)comps;
                r = src[idx + 0];
                g = src[idx + 1];
                b = src[idx + 2];
            }
            dst[x] = 0xFF000000u | ((uint32_t)r << 16) | ((uint32_t)g << 8) |
                     (uint32_t)b;
        }
        y++;
    }

    (void)jpeg_finish_decompress(&cinfo);
    jpeg_destroy_decompress(&cinfo);
    fclose(f);

    out->width = width;
    out->height = height;
    out->pixels = pixels;
    return 0;
}

enum image_format {
    IMAGE_FMT_UNKNOWN = 0,
    IMAGE_FMT_BMP,
    IMAGE_FMT_JPEG,
};

static int ends_with_ci(const char *s, const char *suffix) {
    size_t n = strlen(s);
    size_t m = strlen(suffix);
    if (m > n) return 0;
    s += (n - m);
    for (size_t i = 0; i < m; i++) {
        if (tolower((unsigned char)s[i]) != tolower((unsigned char)suffix[i])) {
            return 0;
        }
    }
    return 1;
}

static enum image_format detect_image_format(const char *path) {
    FILE *f = fopen(path, "rb");
    if (!f) {
        return IMAGE_FMT_UNKNOWN;
    }

    unsigned char sig[3] = {0};
    size_t n = fread(sig, 1, sizeof(sig), f);
    fclose(f);

    if (n >= 2 && sig[0] == 'B' && sig[1] == 'M') {
        return IMAGE_FMT_BMP;
    }
    if (n >= 3 && sig[0] == 0xFF && sig[1] == 0xD8 && sig[2] == 0xFF) {
        return IMAGE_FMT_JPEG;
    }

    if (ends_with_ci(path, ".bmp")) {
        return IMAGE_FMT_BMP;
    }
    if (ends_with_ci(path, ".jpg") || ends_with_ci(path, ".jpeg")) {
        return IMAGE_FMT_JPEG;
    }
    return IMAGE_FMT_UNKNOWN;
}

static int load_image(const char *path, Image *out) {
    enum image_format fmt = detect_image_format(path);
    if (fmt == IMAGE_FMT_BMP) {
        return load_bmp(path, out);
    }
    if (fmt == IMAGE_FMT_JPEG) {
        return load_jpeg(path, out);
    }
    fprintf(stderr, "imgview: unsupported image format (expected BMP/JPG/JPEG)\n");
    return -1;
}

static int is_supported_image_name(const char *name) {
    return ends_with_ci(name, ".bmp") || ends_with_ci(name, ".jpg") ||
           ends_with_ci(name, ".jpeg");
}

static int cmp_name_ci(const void *a, const void *b) {
    const char *const *pa = (const char *const *)a;
    const char *const *pb = (const char *const *)b;
    return strcasecmp(*pa, *pb);
}

static void free_image_list(char **items, size_t count) {
    if (!items) return;
    for (size_t i = 0; i < count; ++i) {
        free(items[i]);
    }
    free(items);
}

// Returns:
//   0 on success with at least one image
//   1 if directory scan succeeded but no images were found
//  -1 on error
static int collect_images_from_cwd(char ***out_items, size_t *out_count) {
    *out_items = NULL;
    *out_count = 0;

    DIR *dir = opendir(".");
    if (!dir) {
        perror("imgview: opendir");
        return -1;
    }

    size_t cap = 0;
    size_t len = 0;
    char **items = NULL;

    struct dirent *ent;
    while ((ent = readdir(dir)) != NULL) {
        const char *name = ent->d_name;
        if (name[0] == '.') {
            continue;
        }
        if (!is_supported_image_name(name)) {
            continue;
        }

        struct stat st;
        if (stat(name, &st) != 0 || !S_ISREG(st.st_mode)) {
            continue;
        }

        if (len == cap) {
            size_t next_cap = cap ? cap * 2 : 8;
            char **next = realloc(items, next_cap * sizeof(*next));
            if (!next) {
                closedir(dir);
                free_image_list(items, len);
                return -1;
            }
            items = next;
            cap = next_cap;
        }

        items[len] = strdup(name);
        if (!items[len]) {
            closedir(dir);
            free_image_list(items, len);
            return -1;
        }
        len++;
    }

    closedir(dir);

    if (len == 0) {
        free(items);
        return 1;
    }

    qsort(items, len, sizeof(*items), cmp_name_ci);
    *out_items = items;
    *out_count = len;
    return 0;
}

static void clear_frame(uint32_t *fb, size_t count, uint32_t color) {
    for (size_t i = 0; i < count; ++i) {
        fb[i] = color;
    }
}

static void draw_rect(uint32_t *fb, uint32_t fb_w, uint32_t fb_h,
                      uint32_t x, uint32_t y, uint32_t w, uint32_t h, uint32_t color) {
    if (x >= fb_w || y >= fb_h || w == 0 || h == 0) {
        return;
    }
    uint32_t x2 = x + w;
    uint32_t y2 = y + h;
    if (x2 > fb_w) x2 = fb_w;
    if (y2 > fb_h) y2 = fb_h;

    for (uint32_t yy = y; yy < y2; ++yy) {
        uint32_t *row = fb + (size_t)yy * fb_w;
        for (uint32_t xx = x; xx < x2; ++xx) {
            row[xx] = color;
        }
    }
}

static void draw_hud_background(uint32_t *fb, uint32_t fb_w, uint32_t fb_h) {
    draw_rect(fb, fb_w, fb_h, 0, 0, fb_w, HUD_HEIGHT, HUD_BG_COLOR);
}

static void print_status_line(const char *fmt, ...) {
    char line[640];
    va_list ap;
    va_start(ap, fmt);
    (void)vsnprintf(line, sizeof(line), fmt, ap);
    va_end(ap);

    char out[700];
    int n = snprintf(out, sizeof(out), "\x1b[H\x1b[2K%s", line);
    if (n > 0) {
        (void)write(STDOUT_FILENO, out, (size_t)n);
    }
}

static void draw_loading_state(uint32_t *fb, uint32_t fb_w, uint32_t fb_h,
                               size_t index, size_t total) {
    clear_frame(fb, (size_t)fb_w * fb_h, 0xFF111723);
    draw_hud_background(fb, fb_w, fb_h);

    uint32_t bar_w = fb_w / 2;
    if (bar_w < 64) bar_w = fb_w > 8 ? fb_w - 8 : fb_w;
    uint32_t bar_h = 18;
    uint32_t bar_x = (fb_w - bar_w) / 2;
    uint32_t view_h = (fb_h > (HUD_HEIGHT + VIEW_MARGIN_BOTTOM))
                          ? (fb_h - HUD_HEIGHT - VIEW_MARGIN_BOTTOM)
                          : 1;
    uint32_t bar_y = HUD_HEIGHT + ((view_h > bar_h) ? (view_h - bar_h) / 2 : 0);

    draw_rect(fb, fb_w, fb_h, bar_x, bar_y, bar_w, bar_h, 0xFF2B3548);
    if (total > 0) {
        uint32_t fill_w = (uint32_t)(((index + 1) * (size_t)bar_w) / total);
        if (fill_w > bar_w) fill_w = bar_w;
        draw_rect(fb, fb_w, fb_h, bar_x, bar_y, fill_w, bar_h, 0xFF63D2FF);
    }
}

static void draw_error_state(uint32_t *fb, uint32_t fb_w, uint32_t fb_h) {
    clear_frame(fb, (size_t)fb_w * fb_h, 0xFF2B1111);
    draw_hud_background(fb, fb_w, fb_h);
    uint32_t box_w = fb_w / 3;
    uint32_t box_h = fb_h / 12;
    if (box_w < 40) box_w = fb_w > 8 ? fb_w - 8 : fb_w;
    if (box_h < 8) box_h = 8;
    uint32_t view_h = (fb_h > (HUD_HEIGHT + VIEW_MARGIN_BOTTOM))
                          ? (fb_h - HUD_HEIGHT - VIEW_MARGIN_BOTTOM)
                          : 1;
    uint32_t box_y = HUD_HEIGHT + ((view_h > box_h) ? (view_h - box_h) / 2 : 0);
    draw_rect(fb, fb_w, fb_h, (fb_w - box_w) / 2, box_y, box_w, box_h, 0xFFB03030);
}

// Nearest-neighbor scale + center
static void blit_image(uint32_t *fb, uint32_t fb_w, uint32_t fb_h,
                       const Image *img, uint32_t bg) {
    size_t total = (size_t)fb_w * (size_t)fb_h;
    clear_frame(fb, total, bg);
    draw_hud_background(fb, fb_w, fb_h);

    if (img->width == 0 || img->height == 0) {
        return;
    }

    uint32_t view_y = HUD_HEIGHT;
    uint32_t view_h =
        (fb_h > (HUD_HEIGHT + VIEW_MARGIN_BOTTOM)) ? (fb_h - HUD_HEIGHT - VIEW_MARGIN_BOTTOM) : 1;

    float sx = (float)fb_w / (float)img->width;
    float sy = (float)view_h / (float)img->height;
    float scale = sx < sy ? sx : sy;
    if (scale <= 0.0f) {
        scale = 1.0f;
    }

    uint32_t disp_w = (uint32_t)((float)img->width * scale);
    uint32_t disp_h = (uint32_t)((float)img->height * scale);
    if (disp_w == 0) disp_w = 1;
    if (disp_h == 0) disp_h = 1;
    if (disp_w > fb_w) disp_w = fb_w;
    if (disp_h > view_h) disp_h = view_h;

    uint32_t off_x = (fb_w - disp_w) / 2;
    uint32_t off_y = view_y + ((view_h - disp_h) / 2);

    for (uint32_t dy = 0; dy < disp_h; ++dy) {
        uint32_t src_y = (uint32_t)(((uint64_t)dy * img->height) / disp_h);
        uint32_t *dst_row = fb + (off_y + dy) * fb_w + off_x;
        const uint32_t *src_row = img->pixels + src_y * img->width;

        for (uint32_t dx = 0; dx < disp_w; ++dx) {
            uint32_t src_x = (uint32_t)(((uint64_t)dx * img->width) / disp_w);
            dst_row[dx] = src_row[src_x];
        }
    }
}

int main(int argc, char **argv) {
    const char **image_paths = NULL;
    size_t image_count = 0;
    char **owned_paths = NULL;

    if (argc < 2) {
        size_t found_count = 0;
        int rc = collect_images_from_cwd(&owned_paths, &found_count);
        if (rc < 0) {
            return 1;
        }
        if (rc > 0 || found_count == 0) {
            fprintf(stderr, "imgview: no .bmp/.jpg/.jpeg images found in current directory\n");
            return 1;
        }
        image_paths = (const char **)owned_paths;
        image_count = found_count;
    } else {
        image_paths = (const char **)&argv[1];
        image_count = (size_t)(argc - 1);
    }

    int fb = open(FB_PATH, O_RDWR);
    if (fb < 0) {
        perror("open /dev/fb0");
        return 1;
    }

    struct fb_var_screeninfo var = {0};
    struct fb_fix_screeninfo fix = {0};
    if (ioctl(fb, FBIOGET_VSCREENINFO, &var) < 0 ||
        ioctl(fb, FBIOGET_FSCREENINFO, &fix) < 0) {
        perror("ioctl framebuffer");
        close(fb);
        return 1;
    }

    size_t frame_bytes = (size_t)fix.smem_len;
    size_t expected = (size_t)var.xres * (size_t)var.yres *
                      (size_t)(var.bits_per_pixel / 8);
    if (expected > frame_bytes) {
        fprintf(stderr, "fb: reported buffer smaller than expected (%zu < %zu)\n",
                frame_bytes, expected);
        close(fb);
        return 1;
    }

    uint32_t *frame = mmap(NULL, frame_bytes, PROT_READ | PROT_WRITE,
                           MAP_SHARED, fb, 0);
    if (frame == MAP_FAILED) {
        perror("mmap framebuffer");
        close(fb);
        return 1;
    }

    if (enable_raw_mode() != 0) {
        perror("termios");
        munmap(frame, frame_bytes);
        close(fb);
        return 1;
    }

    size_t current = 0;
    ssize_t loaded_index = -1;
    int running = 1;
    Image img = {0};

    print_status_line("imgview: %zu image(s). Use <- / -> to switch, q or Esc to quit.",
                      image_count);

    while (running) {
        if ((ssize_t)current != loaded_index) {
            draw_loading_state(frame, var.xres, var.yres, current, image_count);
            (void)ioctl(fb, FBIOPAN_DISPLAY, NULL);

            const char *path = image_paths[current];
            print_status_line("imgview: loading [%zu/%zu] %s ...",
                              current + 1, image_count, path);

            free_image(&img);
            if (load_image(path, &img) == 0) {
                blit_image(frame, var.xres, var.yres, &img, 0xFF0F1116);
                loaded_index = (ssize_t)current;
                print_status_line("imgview: showing [%zu/%zu] %s",
                                  current + 1, image_count, path);
            } else {
                draw_error_state(frame, var.xres, var.yres);
                loaded_index = -1;
                print_status_line("imgview: failed to load [%zu/%zu] %s",
                                  current + 1, image_count, path);
            }
            (void)ioctl(fb, FBIOPAN_DISPLAY, NULL);
        }

        int key = read_key();
        if (key < 0) {
            continue;
        }
        switch (key) {
            case 'q':
            case 'Q':
            case '\x1b':
                running = 0;
                break;
            case 'l':
            case 'L':
            case 'n':
            case 'N':
            case VK_ARROW_RIGHT:
                current = (current + 1) % image_count;
                break;
            case 'h':
            case 'H':
            case 'p':
            case 'P':
            case VK_ARROW_LEFT:
                current = (current + image_count - 1) % image_count;
                break;
            default:
                break;
        }
    }

    (void)write(STDOUT_FILENO, "\x1b[H\x1b[2K\x1b[2;1H", 13);
    disable_raw_mode();
    munmap(frame, frame_bytes);
    close(fb);
    free_image(&img);
    free_image_list(owned_paths, image_count);
    return 0;
}
