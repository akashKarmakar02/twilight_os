#include "unistd.h"
#include <fcntl.h>
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

static int out_fd = 1;

static void remove_two(char **argv, int *argc, int i) {
  for (int j = i; j + 2 <= *argc; ++j)
    argv[j] = argv[j + 2];
  *argc -= 2;
}

char *join_strings(char *result, size_t size, char *const arr[], size_t count) {
  result[0] = '\0';
  int first = 1;

  for (size_t i = 3; i < count; i++) {
    if (!first) {
      size_t rem = (size > 0) ? size - strlen(result) - 1 : 0;
      strncat(result, " ", rem);
    }
    {
      size_t rem = (size > 0) ? size - strlen(result) - 1 : 0;
      strncat(result, arr[i], rem);
    }
    first = 0;
  }

  return result;
}

int main(int argc, char **argv) {
  if (argc < 2) {
    printf("usage: cat [file]...\n       cat > file [text...]\n");
    return 1;
  }

  char *file_path;

  if (strcmp(argv[1], ">") == 0 && argc >= 3) {
    file_path = argv[2];
  } else if (strcmp(argv[1], ">") == 0 && argc == 2) {
    printf("usage: cat > file [text...]\n");
    return 1;
  } else {
    file_path = argv[1];
  }

  if (strcmp(argv[1], ">") == 0) {
    int fd = open(file_path, O_WRONLY | O_CREAT | O_TRUNC, 0644);  // add mode
    if (fd < 0) {
      err2(file_path, ": cannot open");
      return 1;
    }

    if (argc == 3) {
      close(fd);
      return 0;
    }

    char buf[512];
    join_strings(buf, sizeof(buf), argv, (size_t)argc);
    if (write_all(fd, buf, strlen(buf)) < 0) {
      err2("write error", 0);
      close(fd);
      return 1;
    }
    close(fd);
    return 0;
  } else {
    int fd = open(file_path, O_RDONLY);
    if (fd < 0) {
      err2(file_path, ": cannot open");
      return 1;
    }

    unsigned char buf[512];
    for (;;) {
      ssize_t r = read(fd, buf, sizeof buf);
      if (r == 0) {
        close(fd);
        return 0;
      }
      if (r < 0) {
          printf("error: %d\n", r);
        err2("read error", 0);
        close(fd);
        return 1;
      }
      if (write_all(out_fd, buf, (size_t)r) < 0) {
        err2("write error", 0);
        close(fd);
        return 1;
      }
    }
  }
}
