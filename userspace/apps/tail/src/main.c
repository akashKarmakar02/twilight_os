#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define DEFAULT_LINES 10
#define CHUNK 512

struct Buffer {
  char *data;
  size_t len;
  size_t cap;
};

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
  const char *msg = "usage: tail [-n lines] [file...]\n";
  write_all(2, msg, strlen(msg));
}

static int parse_lines(const char *arg, int *out_lines) {
  if (!arg || !out_lines) {
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

static int ensure_capacity(struct Buffer *buf, size_t add) {
  if (buf->len + add <= buf->cap) {
    return 0;
  }
  size_t new_cap = buf->cap ? buf->cap : 1024;
  while (new_cap < buf->len + add) {
    new_cap *= 2;
  }
  char *new_data = realloc(buf->data, new_cap);
  if (!new_data) {
    return -1;
  }
  buf->data = new_data;
  buf->cap = new_cap;
  return 0;
}

static int tail_stream(int fd, int lines, const char *label) {
  if (lines <= 0) {
    return 0;
  }

  struct Buffer buf = {0};
  for (;;) {
    if (ensure_capacity(&buf, CHUNK) != 0) {
      dprintf(2, "tail: %s: allocation failure\n", label ? label : "");
      free(buf.data);
      return -1;
    }
    ssize_t r = read(fd, buf.data + buf.len, CHUNK);
    if (r < 0) {
      dprintf(2, "tail: %s: read error (%d)\n", label ? label : "", errno);
      free(buf.data);
      return -1;
    }
    if (r == 0) {
      break;
    }
    buf.len += (size_t)r;
  }

  size_t start = 0;
  int remaining = lines;
  if (buf.len > 0) {
    size_t pos = buf.len;
    while (pos > 0 && remaining > 0) {
      pos--;
      if (buf.data[pos] == '\n') {
        remaining--;
        if (remaining == 0) {
          start = pos + 1;
          break;
        }
      }
    }
    if (remaining > 0) {
      start = 0;
    }
  }

  if (buf.len > start) {
    if (write_all(1, buf.data + start, buf.len - start) < 0) {
      dprintf(2, "tail: write error\n");
      free(buf.data);
      return -1;
    }
  }
  free(buf.data);
  return 0;
}

static int tail_file(const char *path, int lines) {
  int fd = open(path, O_RDONLY);
  if (fd < 0) {
    dprintf(2, "tail: %s: cannot open (%d)\n", path, errno);
    return -1;
  }
  int rc = tail_stream(fd, lines, path);
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
      dprintf(2, "tail: invalid line count -- %s\n", argv[argi + 1]);
      return 1;
    }
    argi += 2;
  } else if (argi < argc && strncmp(argv[argi], "-n", 2) == 0) {
    if (parse_lines(argv[argi] + 2, &lines) != 0) {
      dprintf(2, "tail: invalid line count -- %s\n", argv[argi]);
      return 1;
    }
    argi += 1;
  }

  if (lines < 0) {
    lines = 0;
  }

  if (argi >= argc) {
    return tail_stream(0, lines, NULL) == 0 ? 0 : 1;
  }

  int status = 0;
  for (; argi < argc; ++argi) {
    if (tail_file(argv[argi], lines) != 0) {
      status = 1;
    }
  }

  return status;
}
