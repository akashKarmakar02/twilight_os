/* tsh.c - tiny shell prompt showing username@hostname:cwd$ (or # for root)
 *
 * Compile:
 *   gcc -Wall -Wextra -o tsh tsh.c
 *
 * Notes:
 * - Prompt mimics common bash/sh PS1 of the form: user@host:cwd$ (root: #)
 * - Uses getpwuid(geteuid()) -> getenv("USER") -> "unknown" fallback to
 * determine username.
 * - Shows hostname (gethostname), and current working directory (getcwd).
 * - Very simple tokenization: splits by first space; argv[1] contains rest of
 * line.
 * - Uses execve syscall; on success the process image is replaced. On failure,
 * prints errno.
 */

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
    /* skip whitespace */
    while (*p && isspace((unsigned char)*p))
      p++;
    if (!*p)
      break;

    /* collect one argument into a temp buffer */
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
        /* Backslash escapes: active outside quotes and inside double quotes */
        const char *next = p + 1;
        if (*next) {
          char e = *next;
          if (!in_single) {
            /* Common escapes */
            if (e == 'n')
              c = '\n';
            else if (e == 't')
              c = '\t';
            else
              c = e; /* \", \\, \ , etc. */
            p += 2;
          } else {
            /* In single quotes, backslash is literal */
            c = '\\';
            p++;
          }
        } else { /* trailing backslash → literal */
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

    /* close any unclosed quotes (like sh does) — we just end token */
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
    /* argv[argc] left for NULL later */
  }

  argv[argc] = NULL;
  *argv_out = argv;
  return (int)argc;
}

static void rstrip_newline(char *s) {
  size_t n = strlen(s);
  if (n && s[n - 1] == '\n')
    s[n - 1] = '\0';
}

/* replace: #include <ctype.h> */
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

  /* First try geteuid + getpwuid (most reliable) */
  struct passwd *pw = getpwuid(geteuid());
  if (pw && pw->pw_name && pw->pw_name[0]) {
    strncpy(buf, pw->pw_name, bufsz - 1);
    buf[bufsz - 1] = '\0';
    return buf;
  }

  /* Next try environment variable */
  const char *env_user = getenv("USER");
  if (env_user && env_user[0]) {
    strncpy(buf, env_user, bufsz - 1);
    buf[bufsz - 1] = '\0';
    return buf;
  }

  /* Next try getlogin() */
  char *lg = getlogin();
  if (lg && lg[0]) {
    strncpy(buf, lg, bufsz - 1);
    buf[bufsz - 1] = '\0';
    return buf;
  }

  /* final fallback */
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

  // Trim leading whitespace
  while (isspace((unsigned char)*s))
    s++;

  // Trim trailing whitespace
  if (*s == '\0')
    return s; // all spaces
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

  /* choose prompt char: root -> '#', else '$' */
  char prompt_char = (geteuid() == 0) ? '#' : '$';

  /* Format: user@host:cwd<prompt_char> */
  /* Make sure we don't overflow 'out' */
  int n = snprintf(out, outsz, "%s@%s:%s%c ", user, host, cwd, prompt_char);
  if (n < 0 || (size_t)n >= outsz) {
    /* truncated - fallback */
    strncpy(out, "shell> ", outsz - 1);
    out[outsz - 1] = '\0';
  }
}

int main(void) {
  char prompt_buf[4096];
  char line[4096];

  for (;;) {
    build_prompt(prompt_buf, sizeof prompt_buf);
    /* Print prompt and flush so it appears even if stdout is line-buffered */
    printf("\x1b[92m%s\x1b[0m", prompt_buf);
    fflush(stdout);

    if (!fgets(line, sizeof line, stdin))
      break;
    rstrip_newline(line);
    char *cmdline = lstrip(line);
    if (*cmdline == '\0')
      continue;

    char **argv = NULL;
    int argc = parse_argv(cmdline, &argv);
    if (argc <= 0) {
      free_argv(argv);
      continue;
    }

    if (strcmp(argv[0], "exit") == 0)
      break;
    if (strcmp(argv[0], "cd") == 0) {
      if (argc < 2) {
        printf("cd: usage cd <dir>\n");
        continue;
      } else {
        int res = chdir(trim(argv[1]));
        if (res == -1) {
          fprintf(stderr, "tsh: cd: %s\n", strerror(errno));
        }
        continue;
      }
    }
    char *space = strchr(cmdline, ' ');
    if (space)
      *space = '\0';

    /* Copy argv[0] into cmdline safely: cmdline points into 'line', so compute
       remaining space in 'line' and use snprintf to avoid the strncpy-sizeof
       mismatch. */
    {
      size_t cmdcap = sizeof(line) - (size_t)(cmdline - line);
      if (cmdcap == 0)
        cmdcap = 1;
      snprintf(cmdline, cmdcap, "%s", argv[0]);
    }

    /* If cmdline doesn't start with '/', prefix it with "/bin/" */
    char fullpath[512];
    const char *path;
    if (cmdline[0] == '/')
      path = cmdline;
    else {
      snprintf(fullpath, sizeof(fullpath), "/bin/%s", cmdline);
      path = fullpath;
    }

    /* Provide an empty environment vector (safer than NULL on custom kernels)
     */
    char *const envp[] = {NULL};

    /* Use syscall directly as in your original code */
    long rc = syscall(SYS_execve, path, argv, envp);

    /* On success execve does not return. On failure, syscall returns -1 and
     * errno is set. */
    if (rc == -1) {
      /* Use errno for error text */
      printf("tsh: %s: %s\n", cmdline, strerror(errno));
      continue;
    }
  }
  return 0;
}
