#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#ifndef BLKGETSIZE
#define BLKGETSIZE 0x1260
#endif
#ifndef BLKSSZGET
#define BLKSSZGET 0x1268
#endif
#ifndef BLKGETSIZE64
#define BLKGETSIZE64 0x80081272
#endif

static uint64_t now_ns(void) {
  struct timespec ts;
  if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0)
    return 0;
  return (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
}

static void fill_pattern(unsigned char *buf, size_t len, uint64_t seed) {
  uint64_t x = seed ? seed : 0x9e3779b97f4a7c15ull;
  for (size_t i = 0; i < len; i++) {
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    buf[i] = (unsigned char)(x & 0xff);
  }
}

static int seek_abs(int fd, uint64_t off) {
  if (lseek(fd, (off_t)off, SEEK_SET) < 0)
    return -1;
  return 0;
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

static void print_rate(const char *label, size_t bytes, uint64_t dt_ns) {
  if (dt_ns == 0) {
    printf("%s: time unavailable\n", label);
    return;
  }
  double sec = (double)dt_ns / 1e9;
  double mib = (double)bytes / (1024.0 * 1024.0);
  printf("%s: %.2f MiB in %.3f s = %.2f MiB/s\n", label, mib, sec, mib / sec);
}

static void progress_line(const char *phase, size_t done, size_t total) {
  unsigned pct = total ? (unsigned)((done * 100ull) / total) : 0;
  fprintf(stderr, "\r%s: %3u%% (%zu/%zu KiB)", phase, pct, done / 1024, total / 1024);
  fflush(stderr);
}

static uint64_t align_down_u64(uint64_t x, uint64_t a) {
  if (a == 0)
    return x;
  return x - (x % a);
}

static uint64_t align_up_u64(uint64_t x, uint64_t a) {
  if (a == 0)
    return x;
  uint64_t r = x % a;
  if (r == 0)
    return x;
  return x + (a - r);
}

int main(void) {
  const char *path = "/dev/disk0";
  int fd = open(path, O_RDWR);
  if (fd < 0) {
    fprintf(stderr, "diskbench: open(%s): %s\n", path, strerror(errno));
    return 1;
  }

  uint64_t disk_bytes = 0;
  int sector = 0;
  if (ioctl(fd, BLKSSZGET, &sector) != 0 || sector <= 0)
    sector = 512;

  if (ioctl(fd, BLKGETSIZE64, &disk_bytes) != 0 || disk_bytes == 0) {
    uint64_t sectors_512 = 0;
    if (ioctl(fd, BLKGETSIZE, &sectors_512) == 0 && sectors_512 != 0) {
      disk_bytes = sectors_512 * 512ull;
    } else {
      struct stat st;
      if (fstat(fd, &st) == 0 && st.st_size > 0)
        disk_bytes = (uint64_t)st.st_size;
    }
  }

  if (disk_bytes < (uint64_t)sector * 1024ull * 1024ull) {
    fprintf(stderr, "diskbench: disk too small or size unknown\n");
    close(fd);
    return 1;
  }

  const uint64_t margin = (uint64_t)sector * 4096ull;
  uint64_t bench_bytes = 8ull * 1024ull * 1024ull;
  if (disk_bytes < bench_bytes + margin)
    bench_bytes = align_down_u64(disk_bytes / 8, (uint64_t)sector);
  bench_bytes = align_down_u64(bench_bytes, (uint64_t)sector);
  if (bench_bytes < (uint64_t)sector * 1024ull) {
    fprintf(stderr, "diskbench: bench size too small\n");
    close(fd);
    return 1;
  }

  uint64_t start = align_down_u64(disk_bytes - bench_bytes - margin, (uint64_t)sector);

  size_t total = (size_t)bench_bytes;
  unsigned char *backup = (unsigned char *)malloc(total);
  while (!backup && total > (size_t)(2 * 1024 * 1024)) {
    total /= 2;
    total = (size_t)align_down_u64((uint64_t)total, (uint64_t)sector);
    backup = (unsigned char *)malloc(total);
  }
  if (!backup) {
    fprintf(stderr, "diskbench: malloc backup failed\n");
    close(fd);
    return 1;
  }
  bench_bytes = (uint64_t)total;
  start = align_down_u64(disk_bytes - bench_bytes - margin, (uint64_t)sector);

  size_t io_block = 256 * 1024;
  io_block = (size_t)align_up_u64((uint64_t)io_block, (uint64_t)sector);
  unsigned char *buf = (unsigned char *)malloc(io_block);
  if (!buf) {
    fprintf(stderr, "diskbench: malloc io buffer failed\n");
    free(backup);
    close(fd);
    return 1;
  }

  fprintf(stderr, "diskbench: %s size=%" PRIu64 " MiB sector=%d bench=%" PRIu64
                  " MiB offset=0x%" PRIx64 "\n",
          path, (uint64_t)(disk_bytes / (1024ull * 1024ull)), sector,
          (uint64_t)(bench_bytes / (1024ull * 1024ull)), (uint64_t)start);

  // Backup region.
  if (seek_abs(fd, start) != 0 || read_full(fd, backup, (size_t)bench_bytes) != 0) {
    fprintf(stderr, "diskbench: backup read failed: %s\n", strerror(errno));
    free(buf);
    free(backup);
    close(fd);
    return 1;
  }

  // Write benchmark.
  if (seek_abs(fd, start) != 0) {
    fprintf(stderr, "diskbench: seek: %s\n", strerror(errno));
    free(buf);
    free(backup);
    close(fd);
    return 1;
  }

  uint64_t t0 = now_ns();
  size_t done = 0;
  uint64_t seed = 1;
  while (done < (size_t)bench_bytes) {
    size_t chunk = io_block;
    if (chunk > (size_t)bench_bytes - done)
      chunk = (size_t)bench_bytes - done;
    fill_pattern(buf, chunk, seed++);
    if (write_full(fd, buf, chunk) != 0) {
      fprintf(stderr, "\ndiskbench: write failed: %s\n", strerror(errno));
      goto restore;
    }
    done += chunk;
    progress_line("write", done, (size_t)bench_bytes);
  }
  fprintf(stderr, "\n");
  uint64_t t1 = now_ns();

  // Read benchmark.
  if (seek_abs(fd, start) != 0) {
    fprintf(stderr, "diskbench: seek: %s\n", strerror(errno));
    goto restore;
  }
  uint64_t t2 = now_ns();
  done = 0;
  while (done < (size_t)bench_bytes) {
    size_t chunk = io_block;
    if (chunk > (size_t)bench_bytes - done)
      chunk = (size_t)bench_bytes - done;
    if (read_full(fd, buf, chunk) != 0) {
      fprintf(stderr, "\ndiskbench: read failed: %s\n", strerror(errno));
      goto restore;
    }
    done += chunk;
    progress_line("read ", done, (size_t)bench_bytes);
  }
  fprintf(stderr, "\n");
  uint64_t t3 = now_ns();

  print_rate("write", (size_t)bench_bytes, t1 - t0);
  print_rate("read ", (size_t)bench_bytes, t3 - t2);

restore:
  // Restore original region (not timed).
  if (seek_abs(fd, start) != 0 ||
      write_full(fd, backup, (size_t)bench_bytes) != 0) {
    fprintf(stderr, "diskbench: restore failed: %s\n", strerror(errno));
  }

  free(buf);
  free(backup);
  close(fd);
  return 0;
}
