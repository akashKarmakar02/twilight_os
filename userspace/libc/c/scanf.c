#include "../include/unistd.h"
#include <stdarg.h>
#include <stddef.h>

// Provide glibc/musl compatibility symbols
int __isoc99_scanf(const char *fmt, ...) __attribute__((alias("scanf")));
int __isoc23_scanf(const char *fmt, ...) __attribute__((alias("scanf")));


#ifndef EOF
#define EOF (-1)
#endif

// ---- tiny buffered stdin ----
static char ibuf[256];
static int  ipos = 0;
static int  ilen = 0;
static int  pushed = -1; // one-char pushback

static int refill(void) {
    ipos = 0;
    ilen = (int)read(0, ibuf, sizeof ibuf);
    return ilen;
}

static int getch(void) {
    if (pushed != -1) { int c = pushed; pushed = -1; return c; }
    if (ipos >= ilen) {
        if (refill() <= 0) return EOF;
    }
    return (unsigned char)ibuf[ipos++];
}

static void ungetch(int c) {
    pushed = c;
}

// ---- helpers ----
static int isspace_c(int c) { return c==' '||c=='\t'||c=='\n'||c=='\r'||c=='\f'||c=='\v'; }
static int isdigit_c(int c) { return c>='0'&&c<='9'; }
static int isxdigit_c(int c){ return isdigit_c(c) || (c>='a'&&c<='f') || (c>='A'&&c<='F'); }
static int tolower_c(int c){ return (c>='A'&&c<='Z') ? (c+('a'-'A')) : c; }
static int hexval(int c){
    if (c>='0'&&c<='9') return c-'0';
    c = tolower_c(c);
    if (c>='a'&&c<='f') return 10 + (c-'a');
    return -1;
}

// Skip any whitespace; return first nonspace or EOF (and leaves it consumed)
static int skip_ws(void){
    int c;
    do { c = getch(); } while (c != EOF && isspace_c(c));
    return c;
}

// ---- core conversions ----
static int read_signed(int *out) {
    long val = 0;
    int c = skip_ws();
    if (c == EOF) return 0;

    int neg = 0;
    if (c=='+' || c=='-') { neg = (c=='-'); c = getch(); }

    if (!isdigit_c(c)) { ungetch(c); return 0; }

    do {
        val = val*10 + (c - '0');
        c = getch();
    } while (isdigit_c(c));

    if (c != EOF) ungetch(c);
    if (neg) val = -val;
    *out = (int)val;
    return 1;
}

static int read_unsigned(unsigned *out) {
    unsigned long val = 0;
    int c = skip_ws();
    if (c == EOF) return 0;

    if (!isdigit_c(c)) { ungetch(c); return 0; }

    do {
        val = val*10u + (unsigned)(c - '0');
        c = getch();
    } while (isdigit_c(c));

    if (c != EOF) ungetch(c);
    *out = (unsigned)val;
    return 1;
}

static int read_hex(unsigned *out) {
    unsigned long val = 0;
    int c = skip_ws();
    if (c == EOF) return 0;

    // optional 0x / 0X
    if (c=='0') {
        int c2 = getch();
        if (c2=='x' || c2=='X') c = getch();
        else { if (c2!=EOF) ungetch(c2); }
    }

    if (!isxdigit_c(c)) { ungetch(c); return 0; }

    do {
        int d = hexval(c);
        val = (val<<4) | (unsigned)d;
        c = getch();
    } while (c != EOF && isxdigit_c(c));

    if (c != EOF) ungetch(c);
    *out = (unsigned)val;
    return 1;
}

static int read_word(char *dst) {
    int c = skip_ws();
    if (c == EOF) return 0;
    int n = 0;
    while (c != EOF && !isspace_c(c)) {
        dst[n++] = (char)c;
        c = getch();
    }
    dst[n] = '\0';
    if (c != EOF) ungetch(c);
    return 1;
}

static int read_char(int *outc) {
    int c = getch();
    if (c == EOF) return 0;
    *outc = c;
    return 1;
}

/* very small scanf: %s %d %u %x %c %%  (no width/precision/floats) */
int scanf(const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int assigned = 0;
    int c;

    while ((c = *fmt++) != '\0') {
        if (c == ' ') {
            // absorb any amount of whitespace in input
            int ch;
            do { ch = getch(); } while (isspace_c(ch));
            if (ch != EOF) ungetch(ch);
            continue;
        }
        if (c != '%') {
            // literal match
            int ch = getch();
            if (ch != c) { // mismatch
                if (ch != EOF) ungetch(ch);
                goto done;
            }
            continue;
        }

        // conversion
        c = *fmt++;
        if (c == '\0') break;

        switch (c) {
        case '%': {
            int ch = getch();
            if (ch != '%') { if (ch != EOF) ungetch(ch); goto done; }
        } break;

        case 'c': {
            int ch;
            if (!read_char(&ch)) goto done; // EOF before assignment
            char *out = va_arg(ap, char*);
            *out = (char)ch;
            assigned++;
        } break;

        case 's': {
            char *out = va_arg(ap, char*);
            if (!read_word(out)) goto done;
            assigned++;
        } break;

        case 'd': case 'i': {
            int *out = va_arg(ap, int*);
            if (!read_signed(out)) goto done;
            assigned++;
        } break;

        case 'u': {
            unsigned *out = va_arg(ap, unsigned*);
            if (!read_unsigned(out)) goto done;
            assigned++;
        } break;

        case 'x': {
            unsigned *out = va_arg(ap, unsigned*);
            if (!read_hex(out)) goto done;
            assigned++;
        } break;

        default:
            // unsupported specifier → stop parsing like real scanf would
            goto done;
        }
    }

done:
    va_end(ap);
    return assigned;
}
