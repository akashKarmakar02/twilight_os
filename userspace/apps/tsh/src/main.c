#include "builtins.h"
#include "executor.h"
#include "parser.h"
#include "utils.h"
#include <ctype.h>
#include <stdio.h>
#include <string.h>

static int strip_trailing_background(char *cmdline) {
  size_t end = strlen(cmdline);
  while (end > 0 && isspace((unsigned char)cmdline[end - 1])) {
    end--;
  }

  if (end == 0 || cmdline[end - 1] != '&') {
    return 0;
  }

  size_t amp = end - 1;
  int in_single = 0;
  int in_double = 0;
  int escaped = 0;

  for (size_t i = 0; i < amp; i++) {
    unsigned char c = (unsigned char)cmdline[i];
    if (escaped) {
      escaped = 0;
      continue;
    }
    if (c == '\\' && !in_single) {
      escaped = 1;
      continue;
    }
    if (c == '\'' && !in_double) {
      in_single = !in_single;
      continue;
    }
    if (c == '"' && !in_single) {
      in_double = !in_double;
      continue;
    }
  }

  if (in_single || in_double || escaped) {
    return 0;
  }

  cmdline[amp] = '\0';
  while (amp > 0 && isspace((unsigned char)cmdline[amp - 1])) {
    cmdline[--amp] = '\0';
  }
  return 1;
}

int main(void) {
  char prompt_buf[4096];
  char line[4096];

  for (;;) {
    reap_background_jobs();

    build_prompt(prompt_buf, sizeof prompt_buf);
    printf("\x1b[92m%s\x1b[0m", prompt_buf);
    fflush(stdout);

    if (!fgets(line, sizeof line, stdin))
      break;
    rstrip_newline(line);
    char *cmdline = lstrip(line);
    if (*cmdline == '\0')
      continue;

    int background = strip_trailing_background(cmdline);
    if (*cmdline == '\0')
      continue;

    // Pipeline support: cmd1 | cmd2 | ...
    char **segs = NULL;
    int nseg = split_pipeline(cmdline, &segs);
    if (nseg <= 0) {
      free_strv(segs);
      continue;
    }

    if (nseg == 1) {
      // Single command
      char **argv = NULL;
      int argc = parse_argv(segs[0], &argv);
      if (argc <= 0) {
        free_argv(argv);
        free_strv(segs);
        continue;
      }

      // Check builtins
      int is_builtin = handle_builtin(argc, argv);
      if (is_builtin == 0) {
        // External command
        run_command(argv, background);
      }

      free_argv(argv);
      free_strv(segs);
      continue;
    }

    // Pipeline
    // Note: pipelines don't support builtins effectively here (they run in
    // subshell if we fork, or complex logic) We treat all pipeline segments as
    // external commands or subshells.
    run_pipeline(segs, nseg, background);

    free_strv(segs);
  }
  return 0;
}
