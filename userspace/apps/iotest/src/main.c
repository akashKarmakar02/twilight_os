#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static uint64_t now_ns(void) {
  struct timespec ts;
  if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
    return 0;
  }
  return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

static int write_full(int fd, const void *buf, size_t len) {
  const unsigned char *p = (const unsigned char *)buf;
  size_t off = 0;
  while (off < len) {
    ssize_t n = write(fd, p + off, len - off);
    if (n < 0) {
      if (errno == EINTR)
        continue;
      return -1;
    }
    if (n == 0)
      return -1;
    off += (size_t)n;
  }
  return 0;
}

static int read_full(int fd, void *buf, size_t len) {
  unsigned char *p = (unsigned char *)buf;
  size_t off = 0;
  while (off < len) {
    ssize_t n = read(fd, p + off, len - off);
    if (n < 0) {
      if (errno == EINTR)
        continue;
      return -1;
    }
    if (n == 0)
      return -1;
    off += (size_t)n;
  }
  return 0;
}

static void fill_pattern(unsigned char *buf, size_t len, uint64_t seed) {
  // Simple deterministic pattern (fast enough, catches obvious corruption).
  uint64_t x = seed ? seed : 0x9e3779b97f4a7c15ull;
  for (size_t i = 0; i < len; i++) {
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    buf[i] = (unsigned char)(x & 0xff);
  }
}

static void print_rate(const char *label, size_t bytes, uint64_t dt_ns) {
  if (dt_ns == 0) {
    printf("%s: time unavailable\n", label);
    return;
  }
  double sec = (double)dt_ns / 1e9;
  double mib = (double)bytes / (1024.0 * 1024.0);
  printf("%s: %.2f MiB in %.3f s = %.2f MiB/s\n", label, mib, sec, mib / sec);
}

static void usage(const char *argv0) {
  printf("usage: %s <path> [mib=64] [block=65536] [mode=rw|r|w]\n", argv0);
}

int main(int argc, char **argv) {
  if (argc < 2) {
    usage(argv[0]);
    return 1;
  }

  const char *path = argv[1];
  size_t mib = 64;
  size_t block = 65536;
  const char *mode = "rw";

  if (argc >= 3)
    mib = (size_t)strtoull(argv[2], NULL, 10);
  if (argc >= 4)
    block = (size_t)strtoull(argv[3], NULL, 10);
  if (argc >= 5)
    mode = argv[4];

  if (mib == 0 || block == 0) {
    fprintf(stderr, "iotest: invalid size\n");
    return 1;
  }

  size_t total = mib * 1024ull * 1024ull;
  unsigned char *buf = (unsigned char *)malloc(block);
  if (!buf) {
    fprintf(stderr, "iotest: malloc failed\n");
    return 1;
  }

  if (strchr(mode, 'w')) {
    int fd = open(path, O_CREAT | O_TRUNC | O_WRONLY, 0644);
    if (fd < 0) {
      fprintf(stderr, "iotest: open(w): %s\n", strerror(errno));
      free(buf);
      return 1;
    }

    uint64_t t0 = now_ns();
    size_t remaining = total;
    uint64_t seed = 1;
    while (remaining) {
      size_t chunk = remaining < block ? remaining : block;
      fill_pattern(buf, chunk, seed++);
      if (write_full(fd, buf, chunk) != 0) {
        fprintf(stderr, "iotest: write: %s\n", strerror(errno));
        close(fd);
        free(buf);
        return 1;
      }
      remaining -= chunk;
    }
    // If you add fsync() in the kernel, call it here.
    if (close(fd) != 0) {
      fprintf(stderr, "iotest: close(w): %s\n", strerror(errno));
      free(buf);
      return 1;
    }
    uint64_t t1 = now_ns();
    print_rate("write", total, t1 - t0);
  }

  if (strchr(mode, 'r')) {
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
      fprintf(stderr, "iotest: open(r): %s\n", strerror(errno));
      free(buf);
      return 1;
    }

    uint64_t t0 = now_ns();
    size_t remaining = total;
    while (remaining) {
      size_t chunk = remaining < block ? remaining : block;
      if (read_full(fd, buf, chunk) != 0) {
        fprintf(stderr, "iotest: read: %s\n", strerror(errno));
        close(fd);
        free(buf);
        return 1;
      }
      remaining -= chunk;
    }
    if (close(fd) != 0) {
      fprintf(stderr, "iotest: close(r): %s\n", strerror(errno));
      free(buf);
      return 1;
    }
    uint64_t t1 = now_ns();
    print_rate("read ", total, t1 - t0);
  }

  free(buf);
  return 0;
}

