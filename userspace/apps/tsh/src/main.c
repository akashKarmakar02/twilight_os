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
  if (getcwd(buf, bufsz) == NULL) {
    /* fallback */
    strncpy(buf, "/", bufsz - 1);
    buf[bufsz - 1] = '\0';
    return buf;
  }
  /* Optionally, shorten home to ~ if you want:
     const char *home = getenv("HOME");
     if (home && strncmp(buf, home, strlen(home)) == 0) { ... }
     For now we'll show full cwd like /home/user/projects
  */
  return buf;
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
      continue; /* empty line */

    if (strcmp(cmdline, "exit") == 0)
      break;
    char *space = strchr(cmdline, ' ');
    if (space)
      *space = '\0';

    /* If cmdline doesn't start with '/', prefix it with "/bin/" */
    char fullpath[512];
    const char *path;
    if (cmdline[0] == '/')
      path = cmdline;
    else {
      snprintf(fullpath, sizeof(fullpath), "/bin/%s", cmdline);
      path = fullpath;
    }

    /* Build argv */
    char *const argv[] = {(char *)path, space ? space + 1 : NULL, NULL};

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
