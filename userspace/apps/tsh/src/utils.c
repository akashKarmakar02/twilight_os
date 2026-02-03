#include "utils.h"
#include <limits.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

const char *resolve_path(const char *cmd, char out[512]) {
  if (!cmd || cmd[0] == '\0')
    return cmd;

  // 1. If absolute path, return as-is
  if (cmd[0] == '/')
    return cmd;

  // 2. If it contains '/', it's a relative path (e.g. ./foo, ../bar, foo/bar)
  if (strchr(cmd, '/') != NULL) {
    if (getcwd(out, 512) == NULL) {
      // Fallback or error, just return cmd and hope for best?
      return cmd;
    }
    // Append / and cmd
    size_t len = strlen(out);
    if (len + 1 + strlen(cmd) + 1 > 512) {
      return cmd; // Too long, return original
    }
    // Ensure we don't double slash if root is "/"
    if (len > 0 && out[len - 1] != '/') {
      strcat(out, "/");
    }
    strcat(out, cmd);
    return out;
  }

  // 3. Otherwise, search in /bin/
  snprintf(out, 512, "/bin/%s", cmd);
  return out;
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

void build_prompt(char *out, size_t outsz) {
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
