#define _POSIX_C_SOURCE 200809L

#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <setjmp.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include <jpeglib.h>

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

static void clear_frame(uint32_t *fb, size_t count, uint32_t color) {
    for (size_t i = 0; i < count; ++i) {
        fb[i] = color;
    }
}

// Nearest-neighbor scale + center
static void blit_image(uint32_t *fb, uint32_t fb_w, uint32_t fb_h,
                       const Image *img, uint32_t bg) {
    size_t total = (size_t)fb_w * (size_t)fb_h;
    clear_frame(fb, total, bg);

    if (img->width == 0 || img->height == 0) {
        return;
    }

    float sx = (float)fb_w / (float)img->width;
    float sy = (float)fb_h / (float)img->height;
    float scale = sx < sy ? sx : sy;
    if (scale <= 0.0f) {
        scale = 1.0f;
    }

    uint32_t disp_w = (uint32_t)((float)img->width * scale);
    uint32_t disp_h = (uint32_t)((float)img->height * scale);
    if (disp_w == 0) disp_w = 1;
    if (disp_h == 0) disp_h = 1;
    if (disp_w > fb_w) disp_w = fb_w;
    if (disp_h > fb_h) disp_h = fb_h;

    uint32_t off_x = (fb_w - disp_w) / 2;
    uint32_t off_y = (fb_h - disp_h) / 2;

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
    if (argc < 2) {
        fprintf(stderr, "Usage: %s <image.(bmp|jpg|jpeg)>\n", argv[0]);
        return 1;
    }

    Image img;
    if (load_image(argv[1], &img) != 0) {
        return 1;
    }

    int fb = open(FB_PATH, O_RDWR);
    if (fb < 0) {
        perror("open /dev/fb0");
        free_image(&img);
        return 1;
    }

    struct fb_var_screeninfo var = {0};
    struct fb_fix_screeninfo fix = {0};
    if (ioctl(fb, FBIOGET_VSCREENINFO, &var) < 0 ||
        ioctl(fb, FBIOGET_FSCREENINFO, &fix) < 0) {
        perror("ioctl framebuffer");
        close(fb);
        free_image(&img);
        return 1;
    }

    size_t frame_bytes = (size_t)fix.smem_len;
    size_t expected = (size_t)var.xres * (size_t)var.yres *
                      (size_t)(var.bits_per_pixel / 8);
    if (expected > frame_bytes) {
        fprintf(stderr, "fb: reported buffer smaller than expected (%zu < %zu)\n",
                frame_bytes, expected);
        close(fb);
        free_image(&img);
        return 1;
    }

    uint32_t *frame = mmap(NULL, frame_bytes, PROT_READ | PROT_WRITE,
                           MAP_SHARED, fb, 0);
    if (frame == MAP_FAILED) {
        perror("mmap framebuffer");
        close(fb);
        free_image(&img);
        return 1;
    }

    blit_image(frame, var.xres, var.yres, &img, 0xFF0F1116);
    if (ioctl(fb, FBIOPAN_DISPLAY, NULL) < 0) {
        perror("fb flush");
    }

    munmap(frame, frame_bytes);
    close(fb);
    free_image(&img);
    return 0;
}
