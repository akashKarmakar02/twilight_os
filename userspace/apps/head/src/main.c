#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define DEFAULT_LINES 10
#define BUF_SIZE 512

static int write_all(int fd, const void *buf, size_t len) {
  const unsigned char *p = (const unsigned char *)buf;
  while (len > 0) {
    ssize_t w = write(fd, p, len);
    if (w <= 0) {
      return -1;
    }
    p += (size_t)w;
    len -= (size_t)w;
  }
  return 0;
}

static void print_usage(void) {
  const char *msg = "usage: head [-n lines] [file...]\n";
  write_all(2, msg, strlen(msg));
}

static int parse_lines(const char *arg, int *out_lines) {
  if (arg == NULL || out_lines == NULL) {
    return -1;
  }
  char *end = NULL;
  long v = strtol(arg, &end, 10);
  if (end == arg || *end != '\0' || v < 0) {
    return -1;
  }
  if (v > 0x7fffffffL) {
    v = 0x7fffffffL;
  }
  *out_lines = (int)v;
  return 0;
}

static int head_stream(int fd, int lines, const char *label) {
  if (lines <= 0) {
    return 0;
  }
  unsigned char buf[BUF_SIZE];
  int remaining = lines;

  while (remaining > 0) {
    ssize_t r = read(fd, buf, sizeof buf);
    if (r < 0) {
      if (label) {
        dprintf(2, "head: %s: read error (%d)\n", label, errno);
      } else {
        dprintf(2, "head: read error (%d)\n", errno);
      }
      return -1;
    }
    if (r == 0) {
      break;
    }

    size_t stop = (size_t)r;
    int done = 0;
    for (size_t i = 0; i < (size_t)r; ++i) {
      if (buf[i] == '\n') {
        remaining--;
        if (remaining == 0) {
          stop = i + 1;
          done = 1;
          break;
        }
      }
    }

    size_t to_write = done ? stop : (size_t)r;
    if (write_all(1, buf, to_write) < 0) {
      dprintf(2, "head: write error\n");
      return -1;
    }

    if (done) {
      break;
    }
  }

  return 0;
}

static int head_file(const char *path, int lines) {
  int fd = open(path, O_RDONLY);
  if (fd < 0) {
    dprintf(2, "head: %s: cannot open (%d)\n", path, errno);
    return -1;
  }
  int rc = head_stream(fd, lines, path);
  close(fd);
  return rc;
}

int main(int argc, char **argv) {
  int lines = DEFAULT_LINES;
  int argi = 1;

  if (argi < argc && strcmp(argv[argi], "-n") == 0) {
    if (argi + 1 >= argc) {
      print_usage();
      return 1;
    }
    if (parse_lines(argv[argi + 1], &lines) != 0) {
      dprintf(2, "head: invalid line count -- %s\n", argv[argi + 1]);
      return 1;
    }
    argi += 2;
  } else if (argi < argc && strncmp(argv[argi], "-n", 2) == 0) {
    if (parse_lines(argv[argi] + 2, &lines) != 0) {
      dprintf(2, "head: invalid line count -- %s\n", argv[argi]);
      return 1;
    }
    argi += 1;
  }

  if (lines < 0) {
    lines = 0;
  }

  if (argi >= argc) {
    return head_stream(0, lines, NULL) == 0 ? 0 : 1;
  }

  int status = 0;
  for (; argi < argc; ++argi) {
    if (head_file(argv[argi], lines) != 0) {
      status = 1;
    }
  }
  return status;
}
