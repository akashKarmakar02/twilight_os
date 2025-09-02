#include "../include/unistd.h"
#include <stdarg.h>

static int putc1(char c) { write(1, &c, 1); return 1; }
static int puts1(const char *s) {
    int n = 0; if (!s) s = "(null)";
    while (*s) { write(1, s, 1); ++s; ++n; }
    return n;
}
static int putu(unsigned long x, int base, int sign) {
    char buf[32]; int i = 0, n = 0;
    if (sign && (long)x < 0) { n += putc1('-'); x = (unsigned long)(-(long)x); }
    do {
        unsigned d = x % (unsigned)base; buf[i++] = (d < 10) ? '0' + d : 'a' + (d - 10);
        x /= (unsigned)base;
    } while (x);
    while (i--) { n += putc1(buf[i]); }
    return n;
}

/* very small printf: %s %d %u %x %c %% (no width/precision/floats) */
int printf(const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int out = 0;
    for (; *fmt; ++fmt) {
        if (*fmt != '%') { out += putc1(*fmt); continue; }
        ++fmt;
        switch (*fmt) {
            case '%': out += putc1('%'); break;
            case 'c': out += putc1((char)va_arg(ap, int)); break;
            case 's': out += puts1(va_arg(ap, const char*)); break;
            case 'd': case 'i': out += putu((unsigned long)va_arg(ap, int), 10, 1); break;
            case 'u': out += putu((unsigned long)va_arg(ap, unsigned), 10, 0); break;
            case 'x': out += putu((unsigned long)va_arg(ap, unsigned), 16, 0); break;
            default:  out += putc1('%'); out += putc1(*fmt); break;
        }
    }
    va_end(ap);
    return out;
}
