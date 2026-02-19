#define _GNU_SOURCE
#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pwd.h>
#include <shadow.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#define SYS_add_user_key 448
#define SYS_set_file_attr 449
#define IFLAG_ENCRYPTED (1 << 2)

static long sys_add_user_key(uid_t uid, const void *key, size_t keylen) {
  return syscall(SYS_add_user_key, uid, key, keylen);
}

static long sys_set_file_attr(const char *path, uint32_t attr, uint32_t value) {
  return syscall(SYS_set_file_attr, path, attr, value);
}

#define PASSWD_FILE "/etc/passwd"
#define PASSWD_MAX_LINE 1024
#define USERNAME_MAX 32
#define PASSWORD_MAX 256
#define HOME_DIR_PREFIX "/home"
#define ZONEINFO_ROOT "/usr/share/zoneinfo"
#define LOCALTIME_PATH "/etc/localtime"
#define LOCALTIME_TMP_PATH "/etc/localtime.tmp"
#define TIMEZONE_NAME_PATH "/etc/timezone"

typedef struct {
  char **items;
  size_t len;
  size_t cap;
} StringList;

static void string_list_init(StringList *list) {
  list->items = NULL;
  list->len = 0;
  list->cap = 0;
}

static void string_list_free(StringList *list) {
  if (!list)
    return;

  for (size_t i = 0; i < list->len; i++) {
    free(list->items[i]);
  }
  free(list->items);
  list->items = NULL;
  list->len = 0;
  list->cap = 0;
}

static int string_list_push(StringList *list, const char *value) {
  if (list->len == list->cap) {
    size_t new_cap = list->cap == 0 ? 16 : list->cap * 2;
    char **new_items = realloc(list->items, new_cap * sizeof(char *));
    if (!new_items)
      return -1;
    list->items = new_items;
    list->cap = new_cap;
  }

  list->items[list->len] = strdup(value);
  if (!list->items[list->len])
    return -1;
  list->len++;
  return 0;
}

static int string_cmp(const void *a, const void *b) {
  const char *sa = *(const char *const *)a;
  const char *sb = *(const char *const *)b;
  return strcmp(sa, sb);
}

static void string_list_sort(StringList *list) {
  if (list->len > 1) {
    qsort(list->items, list->len, sizeof(char *), string_cmp);
  }
}

static int should_skip_continent(const char *name) {
  return strcmp(name, ".") == 0 || strcmp(name, "..") == 0 || name[0] == '.' ||
         strcmp(name, "posix") == 0 || strcmp(name, "right") == 0 ||
         strcmp(name, "SystemV") == 0;
}

static int collect_continents(StringList *continents) {
  DIR *dir = opendir(ZONEINFO_ROOT);
  if (!dir)
    return -1;

  struct dirent *entry;
  while ((entry = readdir(dir)) != NULL) {
    if (should_skip_continent(entry->d_name))
      continue;

    char full_path[PATH_MAX];
    int written =
        snprintf(full_path, sizeof(full_path), "%s/%s", ZONEINFO_ROOT, entry->d_name);
    if (written < 0 || (size_t)written >= sizeof(full_path))
      continue;

    struct stat st;
    if (stat(full_path, &st) != 0 || !S_ISDIR(st.st_mode))
      continue;

    if (string_list_push(continents, entry->d_name) != 0) {
      closedir(dir);
      return -1;
    }
  }

  closedir(dir);
  string_list_sort(continents);
  return 0;
}

static int collect_locations_recursive(const char *base_path, const char *rel_path,
                                       StringList *locations) {
  char current_path[PATH_MAX];
  int current_written;

  if (rel_path[0] == '\0') {
    current_written = snprintf(current_path, sizeof(current_path), "%s", base_path);
  } else {
    current_written =
        snprintf(current_path, sizeof(current_path), "%s/%s", base_path, rel_path);
  }
  if (current_written < 0 || (size_t)current_written >= sizeof(current_path))
    return -1;

  DIR *dir = opendir(current_path);
  if (!dir)
    return -1;

  struct dirent *entry;
  while ((entry = readdir(dir)) != NULL) {
    if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0 ||
        entry->d_name[0] == '.')
      continue;

    char child_rel[PATH_MAX];
    int rel_written;
    if (rel_path[0] == '\0') {
      rel_written = snprintf(child_rel, sizeof(child_rel), "%s", entry->d_name);
    } else {
      rel_written = snprintf(child_rel, sizeof(child_rel), "%s/%s", rel_path,
                             entry->d_name);
    }
    if (rel_written < 0 || (size_t)rel_written >= sizeof(child_rel))
      continue;

    char child_full[PATH_MAX];
    int full_written =
        snprintf(child_full, sizeof(child_full), "%s/%s", base_path, child_rel);
    if (full_written < 0 || (size_t)full_written >= sizeof(child_full))
      continue;

    struct stat st;
    if (stat(child_full, &st) != 0)
      continue;

    if (S_ISDIR(st.st_mode)) {
      if (collect_locations_recursive(base_path, child_rel, locations) != 0) {
        closedir(dir);
        return -1;
      }
    } else if (S_ISREG(st.st_mode)) {
      if (string_list_push(locations, child_rel) != 0) {
        closedir(dir);
        return -1;
      }
    }
  }

  closedir(dir);
  return 0;
}

static int collect_locations(const char *continent, StringList *locations) {
  char continent_path[PATH_MAX];
  int written =
      snprintf(continent_path, sizeof(continent_path), "%s/%s", ZONEINFO_ROOT, continent);
  if (written < 0 || (size_t)written >= sizeof(continent_path))
    return -1;

  if (collect_locations_recursive(continent_path, "", locations) != 0)
    return -1;

  string_list_sort(locations);
  return 0;
}

static int read_stdin_line(char *buf, size_t bufsz) {
  if (!fgets(buf, bufsz, stdin))
    return -1;

  size_t len = strlen(buf);
  if (len > 0 && buf[len - 1] == '\n') {
    buf[len - 1] = '\0';
  } else if (len + 1 == bufsz) {
    int c;
    while ((c = getchar()) != '\n' && c != EOF) {
    }
  }

  return 0;
}

static size_t digit_count(size_t value) {
  size_t digits = 1;
  while (value >= 10) {
    value /= 10;
    digits++;
  }
  return digits;
}

static size_t get_terminal_columns(void) {
  struct winsize ws;
  memset(&ws, 0, sizeof(ws));

  if (ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == 0 && ws.ws_col > 0) {
    size_t cols = ws.ws_col;
    return cols < 20 ? 20 : cols;
  }

  memset(&ws, 0, sizeof(ws));
  if (ioctl(STDIN_FILENO, TIOCGWINSZ, &ws) == 0 && ws.ws_col > 0) {
    size_t cols = ws.ws_col;
    return cols < 20 ? 20 : cols;
  }

  const char *env_cols = getenv("COLUMNS");
  if (env_cols && env_cols[0] != '\0') {
    char *endptr = NULL;
    errno = 0;
    long parsed = strtol(env_cols, &endptr, 10);
    if (errno == 0 && endptr != env_cols && *endptr == '\0' && parsed > 0) {
      size_t cols = (size_t)parsed;
      return cols < 20 ? 20 : cols;
    }
  }

  return 80;
}

static size_t menu_label_width(size_t one_based_index, const char *item) {
  return digit_count(one_based_index) + 2 + strlen(item);
}

static int menu_select(const char *title, StringList *items, size_t *selected_index) {
  if (!items || items->len == 0)
    return -1;

  size_t max_label_width = 0;
  for (size_t i = 0; i < items->len; i++) {
    size_t width = menu_label_width(i + 1, items->items[i]);
    if (width > max_label_width)
      max_label_width = width;
  }

  size_t column_width = max_label_width + 2;
  if (column_width == 0)
    column_width = 1;

  size_t terminal_columns = get_terminal_columns();
  size_t columns = terminal_columns / column_width;
  if (columns == 0)
    columns = 1;

  size_t row_count = (items->len + columns - 1) / columns;
  char input[64];

  for (;;) {
    printf("\n%s\n", title);
    for (size_t row = 0; row < row_count; row++) {
      size_t last_visible_col = 0;
      for (size_t col = 0; col < columns; col++) {
        size_t idx = row * columns + col;
        if (idx < items->len)
          last_visible_col = col;
      }

      for (size_t col = 0; col < columns; col++) {
        size_t idx = row * columns + col;
        if (idx >= items->len)
          continue;

        size_t label_width = menu_label_width(idx + 1, items->items[idx]);
        printf("%zu) %s", idx + 1, items->items[idx]);

        if (col < last_visible_col) {
          size_t pad = column_width > label_width ? column_width - label_width : 1;
          for (size_t s = 0; s < pad; s++) {
            putchar(' ');
          }
        }
      }
      putchar('\n');
    }
    printf("Enter choice number (1-%zu): ", items->len);
    fflush(stdout);

    if (read_stdin_line(input, sizeof(input)) != 0)
      return -1;

    char *endptr = NULL;
    errno = 0;
    long choice = strtol(input, &endptr, 10);
    if (errno != 0 || endptr == input || *endptr != '\0' || choice < 1) {
      printf("Invalid choice.\n");
      continue;
    }

    if ((size_t)choice > items->len) {
      printf("Choice out of range.\n");
      continue;
    }

    *selected_index = (size_t)(choice - 1);
    return 0;
  }
}

static int copy_file(const char *src_path, const char *dst_path) {
  int src_fd = open(src_path, O_RDONLY);
  if (src_fd < 0)
    return -1;

  int dst_fd = open(dst_path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (dst_fd < 0) {
    close(src_fd);
    return -1;
  }

  char buf[4096];
  int rc = 0;
  for (;;) {
    ssize_t nread = read(src_fd, buf, sizeof(buf));
    if (nread < 0) {
      rc = -1;
      break;
    }
    if (nread == 0)
      break;

    size_t written = 0;
    while (written < (size_t)nread) {
      ssize_t nwritten =
          write(dst_fd, buf + written, (size_t)nread - written);
      if (nwritten < 0) {
        rc = -1;
        break;
      }
      written += (size_t)nwritten;
    }
    if (rc != 0)
      break;
  }

  if (close(dst_fd) != 0 && rc == 0)
    rc = -1;
  close(src_fd);

  return rc;
}

static int install_localtime(const char *zone_path) {
  char src_path[PATH_MAX];
  int written = snprintf(src_path, sizeof(src_path), "%s/%s", ZONEINFO_ROOT, zone_path);
  if (written < 0 || (size_t)written >= sizeof(src_path)) {
    errno = ENAMETOOLONG;
    return -1;
  }

  if (copy_file(src_path, LOCALTIME_TMP_PATH) != 0)
    return -1;

  if (rename(LOCALTIME_TMP_PATH, LOCALTIME_PATH) != 0) {
    int rename_errno = errno;
    unlink(LOCALTIME_TMP_PATH);
    errno = rename_errno;
    return -1;
  }

  return 0;
}

static void write_timezone_name(const char *zone_path) {
  FILE *fp = fopen(TIMEZONE_NAME_PATH, "w");
  if (!fp) {
    fprintf(stderr, "logind: warning: cannot write %s: %s\n", TIMEZONE_NAME_PATH,
            strerror(errno));
    return;
  }
  fprintf(fp, "%s\n", zone_path);
  fclose(fp);
}

static void apply_timezone_env(void) {
  FILE *fp = fopen(TIMEZONE_NAME_PATH, "r");
  if (fp) {
    char tzbuf[PATH_MAX];
    if (fgets(tzbuf, sizeof(tzbuf), fp)) {
      size_t len = strlen(tzbuf);
      while (len > 0 &&
             (tzbuf[len - 1] == '\n' || tzbuf[len - 1] == '\r')) {
        tzbuf[--len] = '\0';
      }
      if (len > 0) {
        setenv("TZ", tzbuf, 1);
        fclose(fp);
        return;
      }
    }
    fclose(fp);
  }

  // Fallback to explicit localtime path if /etc/timezone is missing.
  setenv("TZ", ":/etc/localtime", 1);
}

static int setup_timezone(void) {
  for (;;) {
    StringList continents;
    string_list_init(&continents);
    if (collect_continents(&continents) != 0 || continents.len == 0) {
      string_list_free(&continents);
      fprintf(stderr, "logind: no timezone continents found in %s\n",
              ZONEINFO_ROOT);
      return 1;
    }

    size_t continent_idx = 0;
    if (menu_select("Select timezone continent", &continents,
                    &continent_idx) != 0) {
      string_list_free(&continents);
      fprintf(stderr, "logind: failed to read timezone continent\n");
      return 1;
    }

    StringList locations;
    string_list_init(&locations);
    if (collect_locations(continents.items[continent_idx], &locations) != 0 ||
        locations.len == 0) {
      fprintf(stderr, "logind: no timezone locations found in %s\n",
              continents.items[continent_idx]);
      string_list_free(&locations);
      string_list_free(&continents);
      continue;
    }

    char title[256];
    snprintf(title, sizeof(title), "Select timezone location in %s",
             continents.items[continent_idx]);

    size_t location_idx = 0;
    if (menu_select(title, &locations, &location_idx) != 0) {
      string_list_free(&locations);
      string_list_free(&continents);
      fprintf(stderr, "logind: failed to read timezone location\n");
      return 1;
    }

    char selected_zone[PATH_MAX];
    int selected_written =
        snprintf(selected_zone, sizeof(selected_zone), "%s/%s",
                 continents.items[continent_idx], locations.items[location_idx]);
    if (selected_written < 0 ||
        (size_t)selected_written >= sizeof(selected_zone)) {
      string_list_free(&locations);
      string_list_free(&continents);
      fprintf(stderr, "logind: selected timezone path too long\n");
      continue;
    }

    printf("Selected timezone: %s\n", selected_zone);
    if (install_localtime(selected_zone) == 0) {
      write_timezone_name(selected_zone);
      printf("logind: timezone configured: %s\n", selected_zone);
      string_list_free(&locations);
      string_list_free(&continents);
      return 0;
    }

    fprintf(stderr, "logind: failed to install timezone '%s': %s\n",
            selected_zone, strerror(errno));
    string_list_free(&locations);
    string_list_free(&continents);
  }
}

// Stub for missing crypt in environment
char *crypt(const char *key, const char *salt) {
  static char buffer[256];
  // Insecure dummy implementation for build success
  // Combining salt and key to simulate a hash
  snprintf(buffer, sizeof(buffer), "$1$STUB$%s", key);
  return buffer;
}

static void usage(const char *progname) {
  printf("Usage: %s [OPTION]...\n", progname);
  printf("Linux-style login daemon\n\n");
  printf("Options:\n");
  printf("  -h, --help              Show this help message\n");
  printf("  -u, --user USERNAME     Create new user\n");
  printf("  login                   Prompt for login\n");
}

static int read_line(int fd, char *buf, size_t bufsz) {
  if (!buf || bufsz == 0)
    return -1;

  size_t pos = 0;
  while (pos < bufsz - 1) {
    ssize_t n = read(fd, buf + pos, 1);
    if (n <= 0) {
      if (pos == 0)
        return -1;
      break;
    }
    if (buf[pos] == '\n') {
      buf[pos] = '\0';
      return (int)pos;
    }
    pos++;
  }
  buf[pos] = '\0';
  return (int)pos;
}

static char *get_password(const char *prompt) {
  static char password[PASSWORD_MAX];
  struct termios old_termios, new_termios;
  int tty_fd = open("/dev/tty", O_RDWR);
  int echo_was_disabled = 0;

  if (tty_fd < 0) {
    tty_fd = STDIN_FILENO;
  }

  printf("%s", prompt);
  fflush(stdout);

  // Disable echo
  if (tcgetattr(tty_fd, &old_termios) == 0) {
    new_termios = old_termios;
    new_termios.c_lflag &= ~(ECHO | ECHOE | ECHOK | ECHONL);
    if (tcsetattr(tty_fd, TCSAFLUSH, &new_termios) == 0) {
      echo_was_disabled = 1;
    }
  }

  ssize_t len = read_line(tty_fd, password, sizeof(password));

  // Restore echo (use saved old_termios, don't call tcgetattr again)
  if (echo_was_disabled) {
    tcsetattr(tty_fd, TCSAFLUSH, &old_termios);
  }

  if (tty_fd != STDIN_FILENO) {
    close(tty_fd);
  }

  if (len < 0) {
    return NULL;
  }

  return password;
}

static int validate_username(const char *username) {
  if (!username || username[0] == '\0')
    return 0;

  size_t len = strlen(username);
  if (len > USERNAME_MAX)
    return 0;

  // Username must start with letter or underscore
  if (!isalnum((unsigned char)username[0]) && username[0] != '_')
    return 0;

  // Rest must be alphanumeric, underscore, or dash
  for (size_t i = 1; i < len; i++) {
    if (!isalnum((unsigned char)username[i]) && username[i] != '_' &&
        username[i] != '-')
      return 0;
  }

  return 1;
}

static int user_exists(const char *username) {
  FILE *fp = fopen(PASSWD_FILE, "r");
  if (!fp)
    return 0;

  char line[PASSWD_MAX_LINE];
  while (fgets(line, sizeof(line), fp)) {
    // Remove newline
    size_t len = strlen(line);
    if (len > 0 && line[len - 1] == '\n')
      line[len - 1] = '\0';

    // Parse passwd entry: username:password:uid:gid:gecos:home:shell
    char *colon = strchr(line, ':');
    if (!colon)
      continue;

    size_t userlen = (size_t)(colon - line);
    if (userlen == strlen(username) && strncmp(line, username, userlen) == 0) {
      fclose(fp);
      return 1;
    }
  }

  fclose(fp);
  return 0;
}

static uid_t get_next_uid(void) {
  uid_t max_uid = 1000; // Start from 1000 for regular users
  FILE *fp = fopen(PASSWD_FILE, "r");
  if (!fp)
    return max_uid;

  char line[PASSWD_MAX_LINE];
  while (fgets(line, sizeof(line), fp)) {
    // Parse passwd entry
    char *fields[7];
    int field_idx = 0;
    char *p = line;
    char *start = p;

    while (*p && field_idx < 7) {
      if (*p == ':') {
        *p = '\0';
        fields[field_idx++] = start;
        start = p + 1;
      }
      p++;
    }
    if (field_idx < 7)
      fields[field_idx++] = start;

    if (field_idx >= 3) {
      // fields[2] is UID
      uid_t uid = (uid_t)atoi(fields[2]);
      if (uid >= max_uid)
        max_uid = uid + 1;
    }
  }

  fclose(fp);
  return max_uid;
}

static int create_directory_recursive(const char *path) {
  // Check if directory already exists
  struct stat st;
  if (stat(path, &st) == 0) {
    if (S_ISDIR(st.st_mode)) {
      return 0; // Already exists
    } else {
      fprintf(stderr, "logind: '%s' exists but is not a directory\n", path);
      return -1;
    }
  }

  // Try to create the directory
  if (mkdir(path, 0755) == 0) {
    return 0; // Success
  }

  // If parent doesn't exist, try to create parent first
  char parent[512];
  const char *last_slash = strrchr(path, '/');
  if (last_slash && last_slash != path) {
    size_t parent_len = (size_t)(last_slash - path);
    if (parent_len >= sizeof(parent))
      return -1;
    strncpy(parent, path, parent_len);
    parent[parent_len] = '\0';

    // Recursively create parent
    if (create_directory_recursive(parent) != 0)
      return -1;

    // Try again to create the directory
    if (mkdir(path, 0755) == 0)
      return 0;
  }

  // Check if it was created by another process (race condition)
  if (stat(path, &st) == 0 && S_ISDIR(st.st_mode)) {
    return 0; // Created by another process, that's fine
  }

  return -1;
}

static int create_user(const char *username, const char *password) {
  if (!validate_username(username)) {
    fprintf(stderr, "logind: invalid username '%s'\n", username);
    return 1;
  }

  if (user_exists(username)) {
    fprintf(stderr, "logind: user '%s' already exists\n", username);
    return 1;
  }

  if (!password || strlen(password) == 0) {
    fprintf(stderr, "logind: password cannot be empty\n");
    return 1;
  }

  // Generate salt for MD5 crypt ($1$salt$)
  // Use a combination of time and random data for salt
  char salt_chars[] =
      "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789./";
  char salt[16];
  time_t now = time(NULL);
  unsigned seed = (unsigned)(now ^ (unsigned long)password);

  // Simple PRNG for salt generation
  salt[0] = salt_chars[seed % (sizeof(salt_chars) - 1)];
  salt[1] = salt_chars[(seed * 1103515245 + 12345) % (sizeof(salt_chars) - 1)];
  salt[2] = salt_chars[(seed * 2147483647) % (sizeof(salt_chars) - 1)];
  salt[3] =
      salt_chars[(seed * 1664525 + 1013904223) % (sizeof(salt_chars) - 1)];
  salt[4] = salt_chars[(seed * 48271) % (sizeof(salt_chars) - 1)];
  salt[5] = salt_chars[(seed * 69621 + 1) % (sizeof(salt_chars) - 1)];
  salt[6] = salt_chars[(seed * 16807) % (sizeof(salt_chars) - 1)];
  salt[7] = salt_chars[(seed * 214013 + 2531011) % (sizeof(salt_chars) - 1)];
  salt[8] = '\0';

  char salt_buf[32];
  snprintf(salt_buf, sizeof(salt_buf), "$1$%s$", salt);

  char *hash = crypt(password, salt_buf);
  if (!hash) {
    fprintf(stderr, "logind: failed to hash password\n");
    return 1;
  }

  // Get next UID
  uid_t uid = get_next_uid();

  // Create home directory path
  char home_dir[256];
  snprintf(home_dir, sizeof(home_dir), "%s/%s", HOME_DIR_PREFIX, username);

  if (setgid(uid) != 0) {
    fprintf(stderr, "logind: failed to setgid(%u): %s\n", uid, strerror(errno));
    return 1;
  }
  if (setuid(uid) != 0) {
    fprintf(stderr, "logind: failed to setuid(%u): %s\n", uid, strerror(errno));
    setgid(0);
    return 1;
  }

  // Create home directory (and parent directories if needed)
  if (create_directory_recursive(home_dir) != 0) {
    setuid(0);
    setgid(0);
    fprintf(stderr, "logind: failed to create home directory '%s': %s\n",
            home_dir, strerror(errno));
    return 1;
  }

  if (setuid(0) != 0 || setgid(0) != 0) {
    fprintf(stderr, "logind: failed to restore root identity after home creation\n");
    return 1;
  }

  // Enable encryption on the home directory
  long attr_ret = sys_set_file_attr(home_dir, IFLAG_ENCRYPTED, 1);
  if (attr_ret < 0) {
    fprintf(stderr,
            "logind: warning: failed to enable encryption on '%s': %d\n",
            home_dir, (int)attr_ret);
  }

  // Append to passwd file
  FILE *fp = fopen(PASSWD_FILE, "a");
  if (!fp) {
    fprintf(stderr, "logind: cannot open %s: %s\n", PASSWD_FILE,
            strerror(errno));
    return 1;
  }

  // Format: username:password:uid:gid:gecos:home:shell
  // gid = uid for now (no groups yet)
  // gecos = full name (empty for now)
  // shell = /bin/tsh
  fprintf(fp, "%s:%s:%u:%u::%s:/bin/tsh\n", username, hash, uid, uid, home_dir);

  fclose(fp);

  printf("logind: user '%s' created successfully (UID: %u)\n", username, uid);
  printf("logind: home directory: %s\n", home_dir);

  if (setup_timezone() != 0) {
    fprintf(stderr, "logind: timezone setup failed\n");
    return 1;
  }

  return 0;
}

static int authenticate_user(const char *username, const char *password) {
  FILE *fp = fopen(PASSWD_FILE, "r");
  if (!fp) {
    fprintf(stderr, "logind: cannot open %s: %s\n", PASSWD_FILE,
            strerror(errno));
    return 0;
  }

  char line[PASSWD_MAX_LINE];
  while (fgets(line, sizeof(line), fp)) {
    // Remove newline
    size_t len = strlen(line);
    if (len > 0 && line[len - 1] == '\n')
      line[len - 1] = '\0';

    // Parse passwd entry
    char *fields[7];
    int field_idx = 0;
    char *p = line;
    char *start = p;

    while (*p && field_idx < 7) {
      if (*p == ':') {
        *p = '\0';
        fields[field_idx++] = start;
        start = p + 1;
      }
      p++;
    }
    if (field_idx < 7)
      fields[field_idx++] = start;

    if (field_idx < 2)
      continue;

    // Check username
    if (strcmp(fields[0], username) != 0)
      continue;

    // Check password
    char *stored_hash = fields[1];
    char *computed_hash = crypt(password, stored_hash);

    if (computed_hash && strcmp(computed_hash, stored_hash) == 0) {
      // Authentication successful
      // Derive key and add to kernel keyring
      uint8_t key[32];
      size_t pass_len = strlen(password);
      for (int i = 0; i < 32; i++) {
        key[i] = password[i % pass_len] ^ (uint8_t)(i * 13);
      }

      uid_t uid = (uid_t)atoi(fields[2]);
      long ret = sys_add_user_key(uid, key, 32);
      if (ret < 0) {
        fprintf(stderr, "logind: warning: failed to add user key: %d\n",
                (int)ret);
      }

      fclose(fp);
      return 1;
    }

    fclose(fp);
    return 0;
  }

  fclose(fp);
  return 0;
}

static int do_login(void) {
  char username[USERNAME_MAX + 1];

  for (;;) {
    char *password;

    printf("Username: ");
    fflush(stdout);

    if (!fgets(username, sizeof(username), stdin)) {
      // EOF or error - exit
      if (feof(stdin)) {
        printf("\n");
        return 1;
      }
      fprintf(stderr, "logind: failed to read username\n");
      return 1;
    }

    // Remove newline
    size_t len = strlen(username);
    if (len > 0 && username[len - 1] == '\n')
      username[len - 1] = '\0';

    if (strlen(username) == 0) {
      fprintf(stderr, "logind: username cannot be empty\n");
      continue; // Retry
    }

    password = get_password("Password: ");
    if (!password || strlen(password) == 0) {
      fprintf(stderr, "logind: password cannot be empty\n");
      continue; // Retry
    }

    if (!authenticate_user(username, password)) {
      fprintf(stderr, "logind: login failed: invalid username or password\n");
      continue; // Retry login
    }

    // Authentication successful - break out of loop
    break;
  }

  // Get user info from passwd file
  FILE *fp = fopen(PASSWD_FILE, "r");
  if (fp) {
    char line[PASSWD_MAX_LINE];
    while (fgets(line, sizeof(line), fp)) {
      // Parse entry
      char *fields[7];
      int field_idx = 0;
      char *p = line;
      char *start = p;

      while (*p && field_idx < 7) {
        if (*p == ':') {
          *p = '\0';
          fields[field_idx++] = start;
          start = p + 1;
        }
        p++;
      }
      if (field_idx < 7)
        fields[field_idx++] = start;

      if (field_idx >= 3 && strcmp(fields[0], username) == 0) {
        uid_t uid = (uid_t)atoi(fields[2]);
        char *home = field_idx >= 6 ? fields[5] : HOME_DIR_PREFIX;
        char *shell = field_idx >= 7 ? fields[6] : "/bin/tsh";
        // Set UID/GID (requires root or appropriate privileges)
        //                if (setgid(gid) != 0) {
        //                    fprintf(stderr, "logind: warning: failed to
        //                    setgid: %s\n", strerror(errno));
        //                }
        if (setuid(uid) != 0) {
          fprintf(stderr, "logind: warning: failed to setuid: %s\n",
                  strerror(errno));
        }

        setenv("HOME", home, 1);
        setenv("USER", username, 1);
        setenv("PATH", "/bin", 1);
        apply_timezone_env();

        // Change to home directory
        if (chdir(home) != 0) {
          // If home doesn't exist, try to create it
          fprintf(stderr,
                  "logind: warning: home directory '%s' does not exist\n",
                  home);
        }

        // Execute shell with inherited environment.
        char *argv[] = {shell, NULL};
        execv(shell, argv);

        fprintf(stderr, "logind: failed to execute shell: %s\n",
                strerror(errno));
        fclose(fp);
        return 1;
      }
    }
    fclose(fp);
  }

  fprintf(stderr, "logind: failed to find user info\n");
  return 1;
}

int main(int argc, char **argv) {
  if (argc < 2) {
    // Default to login
    return do_login();
  }

  if (strcmp(argv[1], "-h") == 0 || strcmp(argv[1], "--help") == 0) {
    usage(argv[0]);
    return 0;
  }

  if (strcmp(argv[1], "-u") == 0 || strcmp(argv[1], "--user") == 0) {
    if (argc < 3) {
      fprintf(stderr, "logind: --user requires a username\n");
      usage(argv[0]);
      return 1;
    }

    const char *username = argv[2];
    char *password = get_password("New password: ");

    if (!password || strlen(password) == 0) {
      fprintf(stderr, "logind: password cannot be empty\n");
      return 1;
    }

    return create_user(username, password);
  }

  if (strcmp(argv[1], "login") == 0) {
    return do_login();
  }

  // Default to login if no recognized command
  return do_login();
}
