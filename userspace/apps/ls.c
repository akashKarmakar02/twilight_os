#include "stdio.h"

int main(int argc, char **argv) {
  if (argc < 2) {
    printf("usage: ls <dir name>");
    return 1;
  }

  printf("filename/path: %s", argv[1]);

  return 0;
}