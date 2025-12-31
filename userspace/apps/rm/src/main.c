#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char *argv[]) {
  if (argc < 2) {
    fprintf(stderr, "Usage: %s <file> [file...]\n", argv[0]);
    return 1;
  }

  int exit_status = 0;

  for (int i = 1; i < argc; i++) {
    const char *path = argv[i];
    if (unlink(path) == -1) {
      fprintf(stderr, "rm: cannot remove '%s': %s\n", path, strerror(errno));
      exit_status = 1;
    }
  }

  return exit_status;
}
