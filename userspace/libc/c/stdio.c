// userspace/libc/c/stdio.c

#include "../include/unistd.h"
#include "../include/stdio.h"
#include <stdarg.h>
#include <stddef.h>

// Backing FILE objects and exported pointers
static struct __twlite_FILE __stdin  = { .fd = 0 };
static struct __twlite_FILE __stdout = { .fd = 1 };
static struct __twlite_FILE __stderr = { .fd = 2 };

FILE *stdin  = &__stdin;
FILE *stdout = &__stdout;
FILE *stderr = &__stderr;

int getchar(void){
    unsigned char c; long r = read(0, &c, 1);
    return r <= 0 ? -1 : c;
}

int putchar(int c){
    unsigned char ch = (unsigned char)c;
    return (int)write(1, &ch, 1);
}

int puts(const char *s){
    long n = 0;
    while (*s) { n += write(1, s, 1); s++; }
    n += write(1, "\n", 1);
    return (int)n;
}

// very small printf that only supports %s and %d/%ld/%x/%p is fine too,
// but you said you already have printf implemented elsewhere.

// Flush is a no-op for now
int fflush(FILE *stream) { (void)stream; return 0; }

// Minimal fgets: reads up to size-1 or newline, stores '\0'. Returns s or NULL on EOF/error.
char *fgets(char *s, int size, FILE *stream) {
    if (!s || size <= 0) return NULL;
    int fd = stream ? stream->fd : 0;
    int i = 0;
    while (i < size - 1) {
        unsigned char c;
        long r = read(fd, &c, 1);
        if (r <= 0) break;       // EOF or error
        s[i++] = (char)c;
        if (c == '\n') break;
    }
    if (i == 0) return NULL;     // nothing read
    s[i] = '\0';
    return s;
}
