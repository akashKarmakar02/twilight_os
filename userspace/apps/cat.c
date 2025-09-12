// cat.c — for your OS; handles ">", ">>" inside cat
// Syscalls used: open, read, write, close

// ---- tiny libc shims (safe if your headers are not ready) ----
#include "unistd.h"
#include <stdio.h>
#include <string.h>

#ifndef O_RDONLY
#define O_RDONLY 0
#endif
#ifndef O_WRONLY
#define O_WRONLY 1
#endif
#ifndef O_CREAT
#define O_CREAT 0100
#endif
#ifndef O_TRUNC
#define O_TRUNC 01000
#endif
#ifndef O_APPEND
#define O_APPEND 02000
#endif

// ---- helpers (no libc) ----
static size_t z_strlen(const char *s) {
  const char *p = s;
  while (*p)
    p++;
  return (size_t)(p - s);
}
static int write_all(int fd, const void *buf, size_t n) {
  const unsigned char *p = (const unsigned char *)buf;
  while (n) {
    ssize_t w = write(fd, p, n);
    if (w <= 0)
      return -1;
    p += (size_t)w;
    n -= (size_t)w;
  }
  return 0;
}
static void putstr_fd(int fd, const char *s) {
  (void)write_all(fd, s, z_strlen(s));
}
static void err2(const char *a, const char *b) {
  putstr_fd(2, "cat: ");
  if (a)
    putstr_fd(2, a);
  if (b)
    putstr_fd(2, b);
  putstr_fd(2, "\n");
}

// ---- cat core ----
static int out_fd = 1;

static int cat_fd(int fd) {
  unsigned char buf[16 * 1024];
  for (;;) {
    ssize_t r = read(fd, buf, sizeof buf);
    if (r == 0)
      return 0;
    if (r < 0)
      return -1;
    if (write_all(out_fd, buf, (size_t)r) < 0)
      return -1;
  }
}

// Remove argv[i] and argv[i+1]
static void remove_two(char **argv, int *argc, int i) {
  for (int j = i; j + 2 <= *argc; ++j)
    argv[j] = argv[j + 2];
  *argc -= 2;
}

int main(int argc, char **argv) {
    printf("argc: %d\n", argc);

    for (int i = 0; i < argc; i++) {
        printf("argv[%d] @ %p = \"%s\"\n", i, (void*)argv[i], argv[i]);
    }

    // Also show the NULL terminator after argv
    printf("argv[%d] @ %p = %p (terminator)\n",
           argc, (void*)&argv[argc], (void*)argv[argc]);

    return 0;
}

