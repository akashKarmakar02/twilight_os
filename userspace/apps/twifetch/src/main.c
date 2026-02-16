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

static int parse_meminfo_kb(const char *text, const char *key,
                            uint64_t *out_kb) {
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

static void print_rows_with_ascii(const char **art, size_t art_lines,
                                  const char **rows, size_t row_lines) {
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

#define COLOR_RESET "\033[0m"
#define COLOR_CYAN "\033[1;36m"
#define COLOR_PURPLE "\033[1;35m"
#define COLOR_WHITE "\033[1;37m"

  const char *linux_art[] = {
      "   .--.  ", "  |o_o | ",     "  |:_/ | ",    " //   \\\\ ",
      "(|     |)", "/'\\_   _/`\\", "\\___)=(___/",
  };

  const char *twilight_art[] = {
      " _________   ___       __   ___  ___       ___  ________  ___  ___  "
      "_________",
      "|\\___   ___\\|\\  \\     |\\  \\|\\  \\|\\  \\     |\\  \\|\\   "
      "____\\|\\  \\|\\  \\|\\___   ___\\",
      "\\|___ \\  \\_|\\ \\  \\    \\ \\  \\ \\  \\ \\  \\    \\ \\  \\ \\  "
      "\\___|\\ \\  \\\\\\  \\|___ \\  \\_|",
      "     \\ \\  \\  \\ \\  \\  __\\ \\  \\ \\  \\ \\  \\    \\ \\  \\ \\  "
      "\\  __\\ \\   __  \\   \\ \\  \\",
      "      \\ \\  \\  \\ \\  \\|   \\_\\  \\ \\  \\ \\  \\____\\ \\  \\ \\  "
      "\\|\\  \\ \\  \\ \\  \\   \\ \\  \\",
      "       \\ \\__\\  \\ \\____|\\______\\ \\__\\ \\_______\\ \\__\\ "
      "\\_______\\ \\__\\ \\__\\   \\ \\__\\",
      "        \\|__|   "
      "\\|____|\\______|\\|__|\\|_______|\\|__|\\|_______|\\|__|\\|__|    "
      "\\|__|",
      "",
      "                          [  T W I L I G H T   O S  ]"};

  // Host/user
  const char *user = getenv("USER");
  if (!user)
    user = "unknown";
  const char *host = u.nodename[0] ? u.nodename : "twilight";

  char line0[512];
  snprintf(line0, sizeof(line0), "%s%s@%s%s", COLOR_PURPLE, user, host,
           COLOR_RESET);

  char line1[512]; // Separator
  size_t user_host_len = strlen(user) + 1 + strlen(host);
  memset(line1, 0, sizeof(line1));
  strcat(line1, COLOR_WHITE);
  for (size_t i = 0; i < user_host_len; ++i)
    strcat(line1, "-");
  strcat(line1, COLOR_RESET);

  char os_line[512];
  snprintf(os_line, sizeof(os_line), "%sOS:      %s%s", COLOR_PURPLE,
           COLOR_RESET, u.sysname);

  char kernel_line[512];
  snprintf(kernel_line, sizeof(kernel_line), "%sKernel:  %s%s %s", COLOR_PURPLE,
           COLOR_RESET, u.release, u.version);

  char cpu_line[512];
  if (cpu_model) {
    if (cpu_mhz) {
      snprintf(cpu_line, sizeof(cpu_line), "%sCPU:     %s%s (%" PRIu64 " MHz)",
               COLOR_PURPLE, COLOR_RESET, cpu_model, cpu_mhz);
    } else {
      snprintf(cpu_line, sizeof(cpu_line), "%sCPU:     %s%s", COLOR_PURPLE,
               COLOR_RESET, cpu_model);
    }
  } else {
    snprintf(cpu_line, sizeof(cpu_line), "%sCPU:     %sUnknown", COLOR_PURPLE,
             COLOR_RESET);
  }

  char mem_line[512];
  if (mem_total_kb) {
    double total_mib = (double)mem_total_kb / 1024.0;
    double used_mib = (double)mem_used_kb / 1024.0;
    snprintf(mem_line, sizeof(mem_line), "%sMem:     %s%.0f MiB / %.0f MiB",
             COLOR_PURPLE, COLOR_RESET, used_mib, total_mib);
  } else {
    snprintf(mem_line, sizeof(mem_line), "%sMem:     %sUnknown", COLOR_PURPLE,
             COLOR_RESET);
  }

  const char *rows[] = {line0, line1, os_line, kernel_line, cpu_line, mem_line};

  // Custom print function to handle colors
  const char **art = is_linux ? linux_art : twilight_art;
  size_t art_lines = is_linux ? sizeof(linux_art) / sizeof(linux_art[0])
                              : sizeof(twilight_art) / sizeof(twilight_art[0]);
  size_t row_lines = sizeof(rows) / sizeof(rows[0]);
  size_t n = art_lines > row_lines ? art_lines : row_lines;

  printf("\n");
  for (size_t i = 0; i < n; i++) {
    const char *a = i < art_lines ? art[i] : "";
    // Pad ascii art to constant width only if we have more rows to print
    int padding = 0;
    if (i < row_lines) {
      // Calculate padding based on max width of art.
      // Since the new art is wide, let's just make it a fixed width large
      // enough. formatting with %-78s would work if no colors were in art (but
      // we will add colors). For now, simpler approach: print art, print
      // spaces, print row. But wait, the art lines have different lengths in
      // byte code but fixed visual length? Actually the new art is blocky so
      // mostly fixed width.
      padding = 80 - strlen(a);
      if (padding < 2)
        padding = 2; // Minimum spacing
    }

    if (!is_linux)
      printf("%s", COLOR_CYAN);
    printf("%s", a);
    if (!is_linux)
      printf("%s", COLOR_RESET);

    if (i < row_lines) {
      // Find how long the printed string was.
      // strlen(a) includes bytes. ASCII chars are 1 byte.
      // The escape sequences for backslashes were handled by compiler.
      // So strlen(a) should be the printable length.
      // Wait, I messed up the padding logic above.
      // Iterate to print padding spaces.
      // Let's just align to column 80.
      size_t len = strlen(a);
      if (len < 80) {
        for (size_t k = 0; k < 80 - len; k++)
          putchar(' ');
      } else {
        printf("  ");
      }
      printf("%s\n", rows[i]);
    } else {
      printf("\n");
    }
  }
  printf("\n");

  free(cpu_model);
  free(cpuinfo);
  free(meminfo);
  return 0;
}
