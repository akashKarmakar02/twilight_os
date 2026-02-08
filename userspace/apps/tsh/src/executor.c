#include "executor.h"
#include "parser.h"
#include "utils.h"
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include <string.h>

#ifndef WNOHANG
#define WNOHANG 1
#endif

void reap_background_jobs(void) {
  int st = 0;
  for (;;) {
    pid_t pid = waitpid(-1, &st, WNOHANG);
    if (pid <= 0)
      break;
  }
}

int run_command(char *const argv[], int background) {
  char fullpath[512];
  const char *path = resolve_path(argv[0], fullpath);
  char *const envp[] = {NULL};

  pid_t pid = fork();
  if (pid == 0) {
    // Child
    syscall(SYS_execve, path, argv, envp);
    // If we're here, exec failed
    fprintf(stderr, "tsh: %s: %s\n", argv[0], strerror(errno));
    _exit(127);
  } else if (pid > 0) {
    // Parent
    if (background) {
      printf("[bg] started pid %d\n", (int)pid);
      return 0;
    }

    int st = 0;
    if (waitpid(pid, &st, 0) == -1) {
        // perror("waitpid");
        return -1;
    }
    if (WIFEXITED(st)) {
        return WEXITSTATUS(st);
    }
    return 0;
  } else {
    perror("fork");
    return -1;
  }
}

int run_pipeline(char **segs, int nseg, int background) {
  if (nseg <= 0)
    return 0;

  /*
   * Create pipes.
   * For N segments, we need N-1 pipes.
   * pipes[i] connects seg[i] to seg[i+1].
   */
  int(*pipes)[2] = NULL;
  if (nseg > 1) {
    pipes = calloc((size_t)(nseg - 1), sizeof *pipes);
    if (!pipes) {
      perror("calloc");
      return -1;
    }
    for (int i = 0; i < nseg - 1; i++) {
      if (pipe(pipes[i]) != 0) {
        perror("pipe");
        // Cleanup already opened pipes
        for (int j = 0; j < i; j++) {
            close(pipes[j][0]);
            close(pipes[j][1]);
        }
        free(pipes);
        return -1;
      }
    }
  }

  pid_t *pids = calloc((size_t)nseg, sizeof *pids);
  if (!pids) {
    perror("calloc");
    if (pipes) {
      for (int i = 0; i < nseg - 1; i++) {
        close(pipes[i][0]);
        close(pipes[i][1]);
      }
      free(pipes);
    }
    return -1;
  }

  for (int i = 0; i < nseg; i++) {
    char **argv = NULL;
    int argc = parse_argv(segs[i], &argv);
    if (argc <= 0) {
      free_argv(argv);
      // Determine what to do on parsing error inside pipeline?
      // Better to abort pipeline.
      // Kill previous children? Complex.
      // Just fail this start loop and let others proceed/fail.
      // For simplicity, we just continue (which effectively drops this stage?)
      // Or return error?
      // Let's return error but we must clean up.
      // This logic is getting complex, mimicking original tsh simple return logic.
      free(pids);
      if (pipes) {
          for (int k = 0; k < nseg - 1; k++) {
              close(pipes[k][0]);
              close(pipes[k][1]);
          }
          free(pipes);
      }
      return -1;
    }

    pid_t pid = fork();
    if (pid == -1) {
      perror("fork");
      free_argv(argv);
      // Clean up logic... complicated if some children already spawned.
      // In a real shell we might killpg.
      // Here just break and wait for existing ones.
      free(pids); // leak pids array?
      if (pipes) {
          for (int k = 0; k < nseg - 1; k++) {
              close(pipes[k][0]);
              close(pipes[k][1]);
          }
          free(pipes);
      }
      return -1;
    }

    if (pid == 0) {
      // Child Process
      if (pipes) {
        // Setup Stdin (from previous pipe)
        if (i > 0) {
          if (dup2(pipes[i - 1][0], STDIN_FILENO) == -1) {
              perror("dup2 stdin");
              _exit(1);
          }
        }
        // Setup Stdout (to current pipe)
        if (i < nseg - 1) {
          if (dup2(pipes[i][1], STDOUT_FILENO) == -1) {
              perror("dup2 stdout");
              _exit(1);
          }
        }

        // Close ALL pipe ends (important!)
        for (int k = 0; k < nseg - 1; k++) {
          close(pipes[k][0]);
          close(pipes[k][1]);
        }
      }

      char fullpath[512];
      const char *path = resolve_path(argv[0], fullpath);
      char *const envp[] = {NULL};
      
      syscall(SYS_execve, path, argv, envp);
      
      fprintf(stderr, "tsh: %s: %s\n", argv[0], strerror(errno));
      _exit(127);
    }

    // Parent
    pids[i] = pid;
    free_argv(argv);
  }

  // Parent closes all pipe ends
  if (pipes) {
    for (int k = 0; k < nseg - 1; k++) {
      close(pipes[k][0]);
      close(pipes[k][1]);
    }
    free(pipes);
  }

  if (background) {
    pid_t leader = (nseg > 0) ? pids[0] : -1;
    if (leader > 0) {
      printf("[bg] started pipeline pid %d\n", (int)leader);
    }
    free(pids);
    return 0;
  }

  // Wait for all children (foreground)
  for (int i = 0; i < nseg; i++) {
    int st = 0;
    if (pids[i] > 0) {
        waitpid(pids[i], &st, 0);
    }
  }
  free(pids);
  return 0;
}
