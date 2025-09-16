#include "../include/unistd.h"
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

/* ===== Buffered printf: minimize syscalls ===== */

#ifndef OUTBUF_SZ
#define OUTBUF_SZ 4096
#endif

typedef struct {
  int fd;    /* target fd, use 1 for stdout */
  char *buf; /* buffer base */
  int cap;   /* buffer capacity */
  int len;   /* current length in buffer */
  int total; /* total bytes "printed" */
  int err;   /* first write() error (negative errno) */
} out_t;

static int out_flush(out_t *o) {
  if (o->err || o->len == 0)
    return 0;
  int w = (int)write(o->fd, o->buf, (size_t)o->len);
  if (w < 0) {
    o->err = w;
    o->len = 0;
    return w;
  }
  o->total += w;
  o->len = 0;
  return w;
}

static void out_putc(out_t *o, char c) {
  if (o->err)
    return;
  if (o->len >= o->cap)
    out_flush(o);
  o->buf[o->len++] = c;
}

static void out_repeat(out_t *o, char ch, int n) {
  if (o->err || n <= 0)
    return;
  while (n--) {
    if (o->len >= o->cap) {
      if (out_flush(o) < 0)
        return;
    }
    o->buf[o->len++] = ch;
  }
}

/* Append raw bytes (does NOT add newlines) */
static void out_write(out_t *o, const char *p, int n) {
  if (o->err || n <= 0)
    return;
  /* If chunk bigger than buffer, flush buffer and write in big slices. */
  if (n >= o->cap) {
    out_flush(o);
    int w = (int)write(o->fd, p, (size_t)n);
    if (w < 0) {
      o->err = w;
      return;
    }
    o->total += w;
    return;
  }
  /* Otherwise, copy into buffer (may require one flush). */
  int rem = o->cap - o->len;
  if (n > rem)
    out_flush(o);
  /* now enough room */
  for (int i = 0; i < n; ++i)
    o->buf[o->len++] = p[i];
}

/* Convert unsigned into tmp buffer (reversed), then append */
static void out_emit_u64(out_t *o, uint64_t v, unsigned base, int upper) {
  char tmp[32];
  int i = 0;
  if (base < 2)
    base = 10;
  do {
    unsigned d = (unsigned)(v % base);
    v /= base;
    tmp[i++] = (d < 10) ? ('0' + d) : (upper ? 'A' : 'a') + (d - 10);
  } while (v);
  while (i--)
    out_putc(o, tmp[i]);
}

static void out_emit_i64(out_t *o, int64_t x, unsigned base, int upper) {
  if (base < 2)
    base = 10;
  if (x < 0) {
    out_putc(o, '-');
    uint64_t ux = (uint64_t)(-x);
    out_emit_u64(o, ux, base, upper);
  } else {
    out_emit_u64(o, (uint64_t)x, base, upper);
  }
}

/* Padded unsigned */
static void out_emit_num_padded_u64(out_t *o, uint64_t v, unsigned base,
                                    int upper, int width, int zero) {
  char tmp[32];
  int n = 0;
  if (base < 2)
    base = 10;
  {
    uint64_t t = v;
    do {
      unsigned d = (unsigned)(t % base);
      t /= base;
      tmp[n++] = (d < 10) ? '0' + d : (upper ? 'A' : 'a') + (d - 10);
    } while (t);
  }
  int pad = (width > n) ? (width - n) : 0;
  out_repeat(o, zero ? '0' : ' ', pad);
  while (n--)
    out_putc(o, tmp[n]);
}

/* Padded signed */
static void out_emit_num_padded_i64(out_t *o, int64_t v, unsigned base,
                                    int upper, int width, int zero) {
  int neg = (v < 0);
  uint64_t uv = neg ? (uint64_t)(-v) : (uint64_t)v;

  char tmp[32];
  int n = 0;
  if (base < 2)
    base = 10;
  {
    uint64_t t = uv;
    do {
      unsigned d = (unsigned)(t % base);
      t /= base;
      tmp[n++] = (d < 10) ? '0' + d : (upper ? 'A' : 'a') + (d - 10);
    } while (t);
  }
  int total = n + (neg ? 1 : 0);
  int pad = (width > total) ? (width - total) : 0;

  if (!zero)
    out_repeat(o, ' ', pad);
  if (neg)
    out_putc(o, '-');
  if (zero)
    out_repeat(o, '0', pad);
  while (n--)
    out_putc(o, tmp[n]);
}

/* Core vprintf using our buffered emitter */
static int vprintf_internal(const char *fmt, va_list ap) {
  char stackbuf[OUTBUF_SZ];
  out_t o = {.fd = 1,
             .buf = stackbuf,
             .cap = OUTBUF_SZ,
             .len = 0,
             .total = 0,
             .err = 0};

  for (; *fmt; ++fmt) {
    if (*fmt != '%') {
      out_putc(&o, *fmt);
      continue;
    }

    ++fmt;

    /* flags */
    int zero = 0;
    if (*fmt == '0') {
      zero = 1;
      ++fmt;
    }

    /* width */
    int width = 0;
    while (*fmt >= '0' && *fmt <= '9') {
      width = width * 10 + (*fmt - '0');
      ++fmt;
    }

    /* length */
    enum { LEN_NONE, LEN_HH, LEN_H, LEN_L, LEN_LL, LEN_Z } len = LEN_NONE;
    if (*fmt == 'h') {
      if (fmt[1] == 'h') {
        len = LEN_HH;
        fmt += 2;
      } else {
        len = LEN_H;
        ++fmt;
      }
    } else if (*fmt == 'l') {
      if (fmt[1] == 'l') {
        len = LEN_LL;
        fmt += 2;
      } else {
        len = LEN_L;
        ++fmt;
      }
    } else if (*fmt == 'z') {
      len = LEN_Z;
      ++fmt;
    }

    char c = *fmt ? *fmt : '%';
    if (!*fmt) {
      out_putc(&o, '%');
      break;
    }

    switch (c) {
    case '%':
      out_putc(&o, '%');
      break;

    case 'c': {
      int ch = va_arg(ap, int);
      out_putc(&o, (char)ch);
      break;
    }

    case 's': {
      const char *s = va_arg(ap, const char *);
      if (!s)
        s = "(null)";
      /* measure length once */
      int n = 0;
      const char *t = s;
      while (*t++)
        n++;
      if (width > n)
        out_repeat(&o, ' ', width - n);
      out_write(&o, s, n);
      break;
    }

    case 'd':
    case 'i': {
      int64_t v;
      switch (len) {
      case LEN_HH:
        v = (signed char)va_arg(ap, int);
        break;
      case LEN_H:
        v = (short)va_arg(ap, int);
        break;
      case LEN_L:
        v = va_arg(ap, long);
        break;
      case LEN_LL:
        v = va_arg(ap, long long);
        break;
      case LEN_Z: /* ssize_t */
        v = (ssize_t)va_arg(ap, ssize_t);
        break;
      default:
        v = va_arg(ap, int);
        break;
      }
      if (width)
        out_emit_num_padded_i64(&o, v, 10, 0, width, zero);
      else
        out_emit_i64(&o, v, 10, 0);
      break;
    }

    case 'u':
    case 'o':
    case 'x':
    case 'X': {
      unsigned base = (c == 'o') ? 8u : (c == 'u' ? 10u : 16u);
      int upper = (c == 'X');
      uint64_t v;
      switch (len) {
      case LEN_HH:
        v = (unsigned char)va_arg(ap, unsigned);
        break;
      case LEN_H:
        v = (unsigned short)va_arg(ap, unsigned);
        break;
      case LEN_L:
        v = va_arg(ap, unsigned long);
        break;
      case LEN_LL:
        v = va_arg(ap, unsigned long long);
        break;
      case LEN_Z:
        v = (size_t)va_arg(ap, size_t);
        break;
      default:
        v = va_arg(ap, unsigned);
        break;
      }
      if (width)
        out_emit_num_padded_u64(&o, v, base, upper, width, zero);
      else
        out_emit_u64(&o, v, base, upper);
      break;
    }

    case 'p': {
      uintptr_t v = (uintptr_t)va_arg(ap, void *);
      out_write(&o, "0x", 2);
      out_emit_u64(&o, (uint64_t)v, 16, 0);
      break;
    }

    default:
      out_putc(&o, '%');
      out_putc(&o, c);
      break;
    }
  }

  /* final flush (one syscall in the common case) */
  out_flush(&o);

  /* on error, POSIX printf returns a negative value; we mirror that */
  return o.err ? o.err : o.total;
}

int printf(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  int r = vprintf_internal(fmt, ap);
  va_end(ap);
  return r;
}
