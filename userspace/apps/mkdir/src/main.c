#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int mkdir_p(const char *path, mode_t mode) {
  if (!path || !*path) {
    errno = ENOENT;
    return -1;
  }

  char tmp[1024];
  size_t n = strnlen(path, sizeof(tmp) - 1);
  if (n == 0 || n >= sizeof(tmp) - 1) {
    errno = ENAMETOOLONG;
    return -1;
  }
  memcpy(tmp, path, n);
  tmp[n] = '\0';

  // Strip trailing slashes (except "/").
  while (n > 1 && tmp[n - 1] == '/') {
    tmp[n - 1] = '\0';
    n--;
  }

  for (size_t i = 1; i < n; i++) {
    if (tmp[i] != '/')
      continue;
    tmp[i] = '\0';
    if (mkdir(tmp, mode) != 0 && errno != EEXIST) {
      return -1;
    }
    tmp[i] = '/';
  }
  if (mkdir(tmp, mode) != 0 && errno != EEXIST) {
    return -1;
  }
  return 0;
}

static void usage(const char *argv0) {
  fprintf(stderr, "Usage: %s [-p] <dir> [dir...]\n", argv0);
}

int main(int argc, char *argv[]) {
  int pflag = 0;
  int i = 1;
  if (i < argc && strcmp(argv[i], "-p") == 0) {
    pflag = 1;
    i++;
  }
  if (i >= argc) {
    usage(argv[0]);
    return 1;
  }

  int rc = 0;
  for (; i < argc; i++) {
    const char *dir = argv[i];
    int ok = 0;
    if (pflag) {
      ok = (mkdir_p(dir, 0777) == 0);
    } else {
      ok = (mkdir(dir, 0777) == 0);
    }
    if (!ok) {
      fprintf(stderr, "mkdir: %s: %s\n", dir, strerror(errno));
      rc = 1;
    }
  }
  return rc;
}

