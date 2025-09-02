#include "../include/unistd.h"
#include <stdarg.h>
#include <stdint.h>
#include <stddef.h>
#include <stdarg.h>

static int putc1(char c) { write(1, &c, 1); return 1; }
static int puts1(const char *s) {
    int n = 0; if (!s) s = "(null)";
    while (*s) { write(1, s, 1); ++s; ++n; }
    return n;
}


static int emit_repeat(char ch, int n) {
    int out = 0;
    while (n-- > 0) out += putc1(ch);
    return out;
}

static int emit_u64(uint64_t v, unsigned base, int upper) {
    char buf[32];
    int i = 0, out = 0;
    if (base < 2) base = 10;
    do {
        unsigned d = (unsigned)(v % base);
        v /= base;
        if (d < 10) buf[i++] = '0' + d;
        else        buf[i++] = (upper ? 'A' : 'a') + (d - 10);
    } while (v);
    while (i--) out += putc1(buf[i]);
    return out;
}

static int emit_i64(int64_t x, unsigned base, int upper) {
    if (base < 2) base = 10;
    if (x < 0) {
        int out = putc1('-');
        uint64_t ux = (uint64_t)(-x);
        return out + emit_u64(ux, base, upper);
    }
    return emit_u64((uint64_t)x, base, upper);
}

/* width + zero pad around an already-known numeric value */
static int emit_num_padded_u64(uint64_t v, unsigned base, int upper, int width, int zero) {
    /* measure */
    char tmp[32]; int n = 0;
    {
        uint64_t t = v;
        do { unsigned d = (unsigned)(t % base); t /= base; tmp[n++] = (d<10)? '0'+d : (upper?'A':'a')+(d-10); } while (t);
    }
    int pad = (width > n) ? (width - n) : 0;
    int out = 0;
    out += emit_repeat(zero ? '0' : ' ', pad);
    while (n--) out += putc1(tmp[n]);
    return out;
}

static int emit_num_padded_i64(int64_t v, unsigned base, int upper, int width, int zero) {
    /* account for minus sign in width */
    int neg = (v < 0);
    uint64_t uv = neg ? (uint64_t)(-v) : (uint64_t)v;

    char tmp[32]; int n = 0;
    {
        uint64_t t = uv;
        do { unsigned d = (unsigned)(t % base); t /= base; tmp[n++] = (d<10)? '0'+d : (upper?'A':'a')+(d-10); } while (t);
    }
    int total = n + (neg ? 1 : 0);
    int pad = (width > total) ? (width - total) : 0;

    int out = 0;
    if (!zero) out += emit_repeat(' ', pad);
    if (neg) out += putc1('-');
    if (zero) out += emit_repeat('0', pad);
    while (n--) out += putc1(tmp[n]);
    return out;
}

/* --- the printf itself --- */
int printf(const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int out = 0;

    for (; *fmt; ++fmt) {
        if (*fmt != '%') { out += putc1(*fmt); continue; }

        /* parse: % [0] [width] [length] conv */
        ++fmt;

        /* flags */
        int zero = 0;
        if (*fmt == '0') { zero = 1; ++fmt; }

        /* width (decimal) */
        int width = 0;
        while (*fmt >= '0' && *fmt <= '9') {
            width = width * 10 + (*fmt - '0');
            ++fmt;
        }

        /* length */
        enum { LEN_NONE, LEN_HH, LEN_H, LEN_L, LEN_LL, LEN_Z } len = LEN_NONE;
        if (*fmt == 'h') {
            if (fmt[1] == 'h') { len = LEN_HH; fmt += 2; }
            else { len = LEN_H; ++fmt; }
        } else if (*fmt == 'l') {
            if (fmt[1] == 'l') { len = LEN_LL; fmt += 2; }
            else { len = LEN_L; ++fmt; }
        } else if (*fmt == 'z') {
            len = LEN_Z; ++fmt;
        }

        char c = *fmt ? *fmt : '%';
        if (!*fmt) { out += putc1('%'); break; }

        switch (c) {
            case '%':
                out += putc1('%');
                break;

            case 'c': {
                int ch = va_arg(ap, int);
                out += putc1((char)ch);
                break;
            }

            case 's': {
                const char *s = va_arg(ap, const char *);
                if (!s) s = "(null)";
                /* width handling for strings (right align) */
                int n = 0; const char *t = s; while (*t++) n++;
                if (width > n) out += emit_repeat(' ', width - n);
                out += puts1(s) - 0; /* puts1 returns count; it doesn’t append newline per your impl */
                break;
            }

            /* signed decimal */
            case 'd':
            case 'i': {
                int64_t v;
                switch (len) {
                    case LEN_HH: v = (signed char)va_arg(ap, int); break;
                    case LEN_H:  v = (short)va_arg(ap, int); break;
                    case LEN_L:  v = va_arg(ap, long); break;
                    case LEN_LL: v = va_arg(ap, long long); break;
                    case LEN_Z:  /* ssize_t */
                    default:     v = va_arg(ap, int);
                                 if (len == LEN_Z) v = (ssize_t)v; /* compile-time noop on LP64 */
                                 break;
                }
                if (width) out += emit_num_padded_i64(v, 10, 0, width, zero);
                else       out += emit_i64(v, 10, 0);
                break;
            }

            /* unsigned: u / o / x / X */
            case 'u': case 'o': case 'x': case 'X': {
                unsigned base = (c=='o') ? 8u : (c=='u' ? 10u : 16u);
                int upper = (c=='X');
                uint64_t v;
                switch (len) {
                    case LEN_HH: v = (unsigned char)va_arg(ap, unsigned); break;
                    case LEN_H:  v = (unsigned short)va_arg(ap, unsigned); break;
                    case LEN_L:  v = va_arg(ap, unsigned long); break;
                    case LEN_LL: v = va_arg(ap, unsigned long long); break;
                    case LEN_Z:  v = (size_t)va_arg(ap, size_t); break;
                    default:     v = va_arg(ap, unsigned); break;
                }
                if (width) out += emit_num_padded_u64(v, base, upper, width, zero);
                else       out += emit_u64(v, base, upper);
                break;
            }

            /* pointer */
            case 'p': {
                uintptr_t v = (uintptr_t)va_arg(ap, void *);
                out += puts1("0x");
                /* pointers typically zero-padded to pointer width; keep minimal */
                out += emit_u64((uint64_t)v, 16, 0);
                break;
            }

            default:
                /* unknown: print literally */
                out += putc1('%');
                out += putc1(c);
                break;
        }
    }

    va_end(ap);
    return out;
}
