#include "builtins.h"
#include "executor.h"
#include "parser.h"
#include "utils.h"
#include <errno.h>
#include <stdio.h>
#include <string.h>

int main(void) {
  char prompt_buf[4096];
  char line[4096];

  for (;;) {
    build_prompt(prompt_buf, sizeof prompt_buf);
    printf("\x1b[92m%s\x1b[0m", prompt_buf);
    fflush(stdout);

    if (!fgets(line, sizeof line, stdin))
      break;
    rstrip_newline(line);
    char *cmdline = lstrip(line);
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
        run_command(argv);
      }
      
      free_argv(argv);
      free_strv(segs);
      continue;
    }

    // Pipeline
    // Note: pipelines don't support builtins effectively here (they run in subshell if we fork, or complex logic)
    // We treat all pipeline segments as external commands or subshells.
    run_pipeline(segs, nseg);

    free_strv(segs);
  }
  return 0;
}
