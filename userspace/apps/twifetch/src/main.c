#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/utsname.h>
#include <unistd.h>

static int read_file_all(const char *path, char **out, size_t *out_len) {
  *out = NULL;
  *out_len = 0;
  int fd = open(path, O_RDONLY);
  if (fd < 0)
    return -1;

  size_t cap = 4096;
  char *buf = (char *)malloc(cap);
  if (!buf) {
    close(fd);
    errno = ENOMEM;
    return -1;
  }

  size_t len = 0;
  for (;;) {
    if (len == cap) {
      cap *= 2;
      char *tmp = (char *)realloc(buf, cap);
      if (!tmp) {
        free(buf);
        close(fd);
        errno = ENOMEM;
        return -1;
      }
      buf = tmp;
    }
    ssize_t n = read(fd, buf + len, cap - len);
    if (n < 0) {
      if (errno == EINTR)
        continue;
      free(buf);
      close(fd);
      return -1;
    }
    if (n == 0)
      break;
    len += (size_t)n;
  }

  close(fd);
  buf = (char *)realloc(buf, len + 1);
  if (!buf)
    return -1;
  buf[len] = '\0';
  *out = buf;
  *out_len = len;
  return 0;
}

static char *dup_trim_n(const char *s, size_t n) {
  if (!s)
    return NULL;
  while (n && (*s == ' ' || *s == '\t')) {
    s++;
    n--;
  }
  while (n && (s[n - 1] == '\n' || s[n - 1] == '\r' || s[n - 1] == ' ' ||
               s[n - 1] == '\t')) {
    n--;
  }
  char *out = (char *)malloc(n + 1);
  if (!out)
    return NULL;
  memcpy(out, s, n);
  out[n] = '\0';
  return out;
}

static char *find_kv_value(const char *text, const char *key) {
  // Find lines like "Key:\tvalue" or "key\t: value".
  size_t klen = strlen(key);
  const char *p = text;
  while (*p) {
    const char *line = p;
    const char *nl = strchr(p, '\n');
    size_t linelen = nl ? (size_t)(nl - line) : strlen(line);

    if (linelen >= klen && strncmp(line, key, klen) == 0) {
      const char *q = line + klen;
      while (*q == ' ' || *q == '\t')
        q++;
      if (*q == ':') {
        q++;
        if (*q == ' ')
          q++;
        size_t remain = linelen - (size_t)(q - line);
        return dup_trim_n(q, remain);
      }
    }

    p = nl ? nl + 1 : p + linelen;
  }
  return NULL;
}

static int parse_meminfo_kb(const char *text, const char *key, uint64_t *out_kb) {
  *out_kb = 0;
  char *val = find_kv_value(text, key);
  if (!val)
    return -1;
  // Avoid strto*() because some libc/header combos (notably glibc C23) redirect
  // to __isoc23_* symbols which musl doesn't provide when cross-linking.
  unsigned long long n = 0;
  int seen = 0;
  for (const char *p = val; *p; p++) {
    if (*p >= '0' && *p <= '9') {
      seen = 1;
      n = n * 10ull + (unsigned long long)(*p - '0');
    } else if (seen) {
      break;
    }
  }
  free(val);
  if (!seen)
    return -1;
  *out_kb = (uint64_t)n;
  return 0;
}

static void print_rows_with_ascii(const char **art, size_t art_lines, const char **rows,
                                  size_t row_lines) {
  size_t n = art_lines > row_lines ? art_lines : row_lines;
  for (size_t i = 0; i < n; i++) {
    const char *a = i < art_lines ? art[i] : "        ";
    const char *r = i < row_lines ? rows[i] : "";
    printf("%s  %s\n", a, r);
  }
}

int main(void) {
  struct utsname u;
  if (uname(&u) != 0) {
    fprintf(stderr, "twifetch: uname: %s\n", strerror(errno));
    return 1;
  }

  int is_linux = (strcmp(u.sysname, "Linux") == 0);

  // CPU
  char *cpuinfo = NULL;
  size_t cpuinfo_len = 0;
  char *cpu_model = NULL;
  uint64_t cpu_mhz = 0;
  if (read_file_all("/proc/cpuinfo", &cpuinfo, &cpuinfo_len) == 0) {
    cpu_model = find_kv_value(cpuinfo, "model name");
    if (!cpu_model)
      cpu_model = find_kv_value(cpuinfo, "Model name");
    char *mhz_s = find_kv_value(cpuinfo, "cpu MHz");
    if (mhz_s) {
      unsigned long long n = 0;
      int seen = 0;
      for (const char *p = mhz_s; *p; p++) {
        if (*p >= '0' && *p <= '9') {
          seen = 1;
          n = n * 10ull + (unsigned long long)(*p - '0');
        } else if (seen) {
          break;
        }
      }
      if (seen)
        cpu_mhz = (uint64_t)n;
      free(mhz_s);
    }
  }

  // Memory
  char *meminfo = NULL;
  size_t meminfo_len = 0;
  uint64_t mem_total_kb = 0, mem_avail_kb = 0;
  if (read_file_all("/proc/meminfo", &meminfo, &meminfo_len) == 0) {
    (void)parse_meminfo_kb(meminfo, "MemTotal", &mem_total_kb);
    if (parse_meminfo_kb(meminfo, "MemAvailable", &mem_avail_kb) != 0) {
      (void)parse_meminfo_kb(meminfo, "MemFree", &mem_avail_kb);
    }
  }

  uint64_t mem_used_kb = 0;
  if (mem_total_kb && mem_avail_kb && mem_total_kb >= mem_avail_kb) {
    mem_used_kb = mem_total_kb - mem_avail_kb;
  }

  // Host/user
  const char *user = getenv("USER");
  if (!user)
    user = "unknown";
  const char *host = u.nodename[0] ? u.nodename : "twilight";

  char line0[256];
  snprintf(line0, sizeof(line0), "%s@%s", user, host);

  char os_line[256];
  snprintf(os_line, sizeof(os_line), "OS: %s", u.sysname);

  char kernel_line[256];
  snprintf(kernel_line, sizeof(kernel_line), "Kernel: %s %s", u.release, u.version);

  char cpu_line[512];
  if (cpu_model) {
    if (cpu_mhz) {
      snprintf(cpu_line, sizeof(cpu_line), "CPU: %s (%" PRIu64 " MHz)", cpu_model, cpu_mhz);
    } else {
      snprintf(cpu_line, sizeof(cpu_line), "CPU: %s", cpu_model);
    }
  } else {
    snprintf(cpu_line, sizeof(cpu_line), "CPU: Unknown");
  }

  char mem_line[256];
  if (mem_total_kb) {
    double total_mib = (double)mem_total_kb / 1024.0;
    double used_mib = (double)mem_used_kb / 1024.0;
    snprintf(mem_line, sizeof(mem_line), "Mem: %.0f MiB / %.0f MiB", used_mib, total_mib);
  } else {
    snprintf(mem_line, sizeof(mem_line), "Mem: Unknown");
  }

  const char *linux_art[] = {
      "   .--.  ",
      "  |o_o | ",
      "  |:_/ | ",
      " //   \\\\ ",
      "(|     |)",
      "/'\\_   _/`\\",
      "\\___)=(___/",
  };

  const char *twilight_art[] = {
    "████████╗██╗    ██╗██╗██╗     ██╗ ██████╗ ██╗  ██╗████████╗",
    "╚══██╔══╝██║    ██║██║██║     ██║██╔════╝ ██║  ██║╚══██╔══╝",
    "   ██║   ██║ █╗ ██║██║██║     ██║██║  ███╗███████║   ██║   ",
    "   ██║   ██║███╗██║██║██║     ██║██║   ██║██╔══██║   ██║   ",
    "   ██║   ╚███╔███╔╝██║███████╗██║╚██████╔╝██║  ██║   ██║   ",
    "   ╚═╝    ╚══╝╚══╝ ╚═╝╚══════╝╚═╝ ╚═════╝ ╚═╝  ╚═╝   ╚═╝   ",
    "TWILIGHT OS — build the night 🌙",
  };

  const char *rows[] = {line0, os_line, kernel_line, cpu_line, mem_line};

  if (is_linux) {
    print_rows_with_ascii(linux_art, sizeof(linux_art) / sizeof(linux_art[0]), rows,
                          sizeof(rows) / sizeof(rows[0]));
  } else {
    print_rows_with_ascii(twilight_art, sizeof(twilight_art) / sizeof(twilight_art[0]), rows,
                          sizeof(rows) / sizeof(rows[0]));
  }

  free(cpu_model);
  free(cpuinfo);
  free(meminfo);
  return 0;
}
