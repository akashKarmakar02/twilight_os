#include "builtins.h"
#include "parser.h" // for trim? or we implement helpers
#include "utils.h" // if needed
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Helpers */
static char *trim_locally(char *s) {
    // We can rely on parser.h's trim if we include it and link it.
    // But since trim is in parser.c, let's include parser.h
    extern char *trim(char *s);
    return trim(s);
}

int handle_builtin(int argc, char **argv) {
  if (argc < 1)
    return 0;

  if (strcmp(argv[0], "exit") == 0) {
    // We should signal the main loop to exit.
    // Returning a specific code?
    // Let's just exit the process here for simplicity,
    // or return a special code. Use 1 for handled.
    // The caller needs to know to break the loop. 
    // Usually builtins are: exit(0). 
    exit(0);
    return 1;
  }

  if (strcmp(argv[0], "cd") == 0) {
    if (argc < 2) {
      printf("cd: usage cd <dir>\n");
    } else {
      // trim argument? Parse argv usually handles quotes/spaces.
      int res = chdir(argv[1]);
      if (res == -1) {
        fprintf(stderr, "tsh: cd: %s\n", strerror(errno));
      }
    }
    return 1;
  }

  return 0; // Not a builtin
}
