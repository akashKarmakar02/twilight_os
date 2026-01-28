#include <ctype.h>
#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#include <pwd.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static void free_argv(char **argv) {
  if (!argv)
    return;
  for (size_t i = 0; argv[i]; i++)
    free(argv[i]);
  free(argv);
}

static int parse_argv(const char *line, char ***argv_out) {
  *argv_out = NULL;
  if (!line)
    return 0;

  size_t cap = 8, argc = 0;
  char **argv = malloc(cap * sizeof *argv);
  if (!argv)
    return 0;

  const char *p = line;
  while (*p) {
    while (*p && isspace((unsigned char)*p))
      p++;
    if (!*p)
      break;

    size_t outcap = 64, outlen = 0;
    char *out = malloc(outcap);
    if (!out) {
      free_argv(argv);
      return 0;
    }

    int in_single = 0, in_double = 0;
    while (*p) {
      unsigned char c = (unsigned char)*p;

      if (!in_single && !in_double && isspace(c))
        break;

      if (!in_double && c == '\'') {
        in_single = !in_single;
        p++;
        continue;
      }
      if (!in_single && c == '"') {
        in_double = !in_double;
        p++;
        continue;
      }

      if (c == '\\') {
        const char *next = p + 1;
        if (*next) {
          char e = *next;
          if (!in_single) {
            if (e == 'n')
              c = '\n';
            else if (e == 't')
              c = '\t';
            else
              c = e;
            p += 2;
          } else {
            c = '\\';
            p++;
          }
        } else {
          p++;
          c = '\\';
        }
      } else {
        p++;
      }

      if (outlen + 1 >= outcap) {
        outcap *= 2;
        char *tmp = realloc(out, outcap);
        if (!tmp) {
          free(out);
          free_argv(argv);
          return 0;
        }
        out = tmp;
      }
      out[outlen++] = (char)c;
    }

    out[outlen] = '\0';

    if (argc + 2 > cap) {
      cap *= 2;
      char **tmp = realloc(argv, cap * sizeof *argv);
      if (!tmp) {
        free(out);
        free_argv(argv);
        return 0;
      }
      argv = tmp;
    }
    argv[argc++] = out;
  }

  argv[argc] = NULL;
  *argv_out = argv;
  return (int)argc;
}

static void free_strv(char **v) {
  if (!v)
    return;
  for (size_t i = 0; v[i]; i++)
    free(v[i]);
  free(v);
}

static int split_pipeline(const char *line, char ***out_segs) {
  *out_segs = NULL;
  if (!line)
    return 0;

  size_t cap = 4, n = 0;
  char **segs = malloc(cap * sizeof *segs);
  if (!segs)
    return 0;

  int in_single = 0, in_double = 0;
  const char *start = line;
  const char *p = line;
  while (*p) {
    char c = *p;
    if (c == '\\') {
      if (p[1])
        p += 2;
      else
        p++;
      continue;
    }
    if (!in_double && c == '\'') {
      in_single = !in_single;
      p++;
      continue;
    }
    if (!in_single && c == '"') {
      in_double = !in_double;
      p++;
      continue;
    }
    if (!in_single && !in_double && c == '|') {
      size_t len = (size_t)(p - start);
      while (len > 0 && isspace((unsigned char)start[0])) {
        start++;
        len--;
      }
      while (len > 0 && isspace((unsigned char)start[len - 1])) {
        len--;
      }
      char *seg = malloc(len + 1);
      if (!seg) {
        free_strv(segs);
        return 0;
      }
      memcpy(seg, start, len);
      seg[len] = '\0';

      if (n + 2 > cap) {
        cap *= 2;
        char **tmp = realloc(segs, cap * sizeof *segs);
        if (!tmp) {
          free(seg);
          free_strv(segs);
          return 0;
        }
        segs = tmp;
      }
      segs[n++] = seg;
      start = p + 1;
    }
    p++;
  }

  // last segment
  size_t len = (size_t)(p - start);
  while (len > 0 && isspace((unsigned char)start[0])) {
    start++;
    len--;
  }
  while (len > 0 && isspace((unsigned char)start[len - 1])) {
    len--;
  }
  char *seg = malloc(len + 1);
  if (!seg) {
    free_strv(segs);
    return 0;
  }
  memcpy(seg, start, len);
  seg[len] = '\0';
  if (n + 2 > cap) {
    cap *= 2;
    char **tmp = realloc(segs, cap * sizeof *segs);
    if (!tmp) {
      free(seg);
      free_strv(segs);
      return 0;
    }
    segs = tmp;
  }
  segs[n++] = seg;
  segs[n] = NULL;

  *out_segs = segs;
  return (int)n;
}

static void rstrip_newline(char *s) {
  size_t n = strlen(s);
  if (n && s[n - 1] == '\n')
    s[n - 1] = '\0';
}

static inline int ascii_isspace(unsigned char c) {
  return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\v' ||
         c == '\f';
}

static char *lstrip(char *s) {
  while (*s && ascii_isspace((unsigned char)*s))
    s++;
  return s;
}

static const char *get_username(char *buf, size_t bufsz) {
  if (!buf || bufsz == 0)
    return "unknown";

  struct passwd *pw = getpwuid(geteuid());
  if (pw && pw->pw_name && pw->pw_name[0]) {
    strncpy(buf, pw->pw_name, bufsz - 1);
    buf[bufsz - 1] = '\0';
    return buf;
  }

  const char *env_user = getenv("USER");
  if (env_user && env_user[0]) {
    strncpy(buf, env_user, bufsz - 1);
    buf[bufsz - 1] = '\0';
    return buf;
  }

  char *lg = getlogin();
  if (lg && lg[0]) {
    strncpy(buf, lg, bufsz - 1);
    buf[bufsz - 1] = '\0';
    return buf;
  }

  strncpy(buf, "unknown", bufsz - 1);
  buf[bufsz - 1] = '\0';
  return buf;
}

static const char *get_hostname(char *buf, size_t bufsz) {
  if (!buf || bufsz == 0)
    return "unknown";
  long hnmax = sysconf(_SC_HOST_NAME_MAX);
  if (hnmax < 0 || hnmax > (long)bufsz - 1)
    hnmax = bufsz - 1;
  if (gethostname(buf, (size_t)hnmax) == 0) {
    buf[(size_t)hnmax] = '\0';
    return buf;
  }
  strncpy(buf, "unknown", bufsz - 1);
  buf[bufsz - 1] = '\0';
  return buf;
}

static const char *get_cwd_short(char *buf, size_t bufsz) {
  if (!buf || bufsz == 0)
    return "/";
  char dbuf[bufsz];

  getcwd(dbuf, sizeof(dbuf));
  strncpy(buf, dbuf, bufsz - 1);
  buf[bufsz - 1] = '\0';

  return buf;
}

static char *trim(char *s) {
  if (!s)
    return s;

  while (isspace((unsigned char)*s))
    s++;

  if (*s == '\0')
    return s;
  char *end = s + strlen(s) - 1;
  while (end > s && isspace((unsigned char)*end))
    end--;
  *(end + 1) = '\0';

  return s;
}

static void build_prompt(char *out, size_t outsz) {
  char user[128];
  char host[128];
  char cwd[PATH_MAX];

  get_username(user, sizeof user);
  get_hostname(host, sizeof host);
  get_cwd_short(cwd, sizeof cwd);

  char prompt_char = (geteuid() == 0) ? '#' : '$';

  int n = snprintf(out, outsz, "%s@%s:%s%c ", user, host, cwd, prompt_char);
  if (n < 0 || (size_t)n >= outsz) {
    strncpy(out, "shell> ", outsz - 1);
    out[outsz - 1] = '\0';
  }
}

static const char *resolve_path(const char *cmd, char out[512]) {
  if (!cmd || cmd[0] == '\0')
    return cmd;
  if (cmd[0] == '/')
    return cmd;
  snprintf(out, 512, "/bin/%s", cmd);
  return out;
}

static int run_command_spawn(char *const argv[]) {
  char fullpath[512];
  const char *path = resolve_path(argv[0], fullpath);
  char *const envp[] = {NULL};

  // Linux shell behavior: fork + exec + wait
  pid_t pid = fork();
  if (pid == 0) {
    syscall(SYS_execve, path, argv, envp);
    _exit(127);
  }
  if (pid > 0) {
    int st = 0;
    (void)waitpid(pid, &st, 0);
    return 0;
  }

  return -1;
}

static int run_pipeline_fork(char **segs, int nseg) {
  if (nseg <= 0)
    return 0;

  int (*pipes)[2] = NULL;
  if (nseg > 1) {
    pipes = calloc((size_t)(nseg - 1), sizeof *pipes);
    if (!pipes)
      return -1;
    for (int i = 0; i < nseg - 1; i++) {
      if (pipe(pipes[i]) != 0) {
        free(pipes);
        return -1;
      }
    }
  }

  pid_t *pids = calloc((size_t)nseg, sizeof *pids);
  if (!pids) {
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
      free_argv(argv);
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

    if (pid == 0) {
      if (pipes) {
        if (i > 0) {
          dup2(pipes[i - 1][0], 0);
        }
        if (i < nseg - 1) {
          dup2(pipes[i][1], 1);
        }
        for (int k = 0; k < nseg - 1; k++) {
          close(pipes[k][0]);
          close(pipes[k][1]);
        }
      }

      char fullpath[512];
      const char *path = resolve_path(argv[0], fullpath);
      char *const envp[] = {NULL};
      syscall(SYS_execve, path, argv, envp);
      _exit(127);
    }

    pids[i] = pid;
    free_argv(argv);
  }

  if (pipes) {
    for (int k = 0; k < nseg - 1; k++) {
      close(pipes[k][0]);
      close(pipes[k][1]);
    }
    free(pipes);
  }

  for (int i = 0; i < nseg; i++) {
    int st = 0;
    (void)waitpid(pids[i], &st, 0);
  }
  free(pids);
  return 0;
}

static int run_pipeline_twilight(char **segs, int nseg) {
  int prev_read = -1;

  // keep stderr on tty
  syscall(SYS_dup2, 0, 2);

  for (int si = 0; si < nseg; si++) {
    char **stage_argv = NULL;
    int stage_argc = parse_argv(segs[si], &stage_argv);
    if (stage_argc <= 0) {
      free_argv(stage_argv);
      return -1;
    }

    if (prev_read >= 0) {
      syscall(SYS_dup2, prev_read, 0);
    } else {
      // stdin to tty
      syscall(SYS_dup2, 1, 0);
    }

    int pipefd[2] = {-1, -1};
    int write_end = -1;
    if (si < nseg - 1) {
      if (syscall(SYS_pipe2, pipefd, 0) != 0) {
        free_argv(stage_argv);
        return -1;
      }
      // stdout to pipe write end
      write_end = pipefd[1];
      syscall(SYS_dup2, write_end, 1);

      // Ensure this stage doesn't inherit the read end.
      close(pipefd[0]);
    } else {
      syscall(SYS_dup2, 0, 1); // stdout to tty
    }

    char fullpath[512];
    const char *path = resolve_path(stage_argv[0], fullpath);
    char *const envp[] = {NULL};
    long rc = syscall(SYS_execve, path, stage_argv, envp);
    free_argv(stage_argv);
    if (rc == -1) {
      return -1;
    }

    if (prev_read >= 0) {
      close(prev_read);
      prev_read = -1;
    }
    if (write_end >= 0) {
      close(write_end);
    }
    if (si < nseg - 1) {
      prev_read = pipefd[0];
    }
  }

  if (prev_read >= 0)
    close(prev_read);

  // restore stdio to tty
  syscall(SYS_dup2, 1, 0);
  syscall(SYS_dup2, 0, 1);
  syscall(SYS_dup2, 0, 2);

  return 0;
}

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
      char **argv = NULL;
      int argc = parse_argv(segs[0], &argv);
      if (argc <= 0) {
        free_argv(argv);
        free_strv(segs);
        continue;
      }

      if (strcmp(argv[0], "exit") == 0) {
        free_argv(argv);
        free_strv(segs);
        break;
      }
      if (strcmp(argv[0], "cd") == 0) {
        if (argc < 2) {
          printf("cd: usage cd <dir>\n");
        } else {
          int res = chdir(trim(argv[1]));
          if (res == -1) {
            fprintf(stderr, "tsh: cd: %s\n", strerror(errno));
          }
        }
        free_argv(argv);
        free_strv(segs);
        continue;
      }

      if (run_command_spawn(argv) != 0) {
        printf("tsh: %s: %s\n", argv[0], strerror(errno));
      }
      free_argv(argv);
      free_strv(segs);
      continue;
    }

    // If fork works (Linux), use real concurrent pipelines.
    if (run_pipeline_fork(segs, nseg) == 0) {
      free_strv(segs);
      continue;
    }
    if (errno == ENOSYS) {
      // Twilight fallback: execve returns -> sequential pipeline.
      (void)run_pipeline_twilight(segs, nseg);
      free_strv(segs);
      continue;
    }

    free_strv(segs);
  }
  return 0;
}
