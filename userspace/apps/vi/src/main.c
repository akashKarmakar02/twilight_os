#define _DEFAULT_SOURCE
#define _BSD_SOURCE
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#define CTRL_KEY(k) ((k) & 0x1f)

struct termios orig_termios;

/* ----- terminal raw mode ----- */
static void die(const char *msg) {
  // best-effort restore
  tcsetattr(STDIN_FILENO, TCSAFLUSH, &orig_termios);
  // show cursor
  write(STDOUT_FILENO, "\x1b[?25h\x1b[0m\x1b[H\x1b[2J", 17);
  perror(msg);
  exit(1);
}

static void disable_raw(void) {
  tcsetattr(STDIN_FILENO, TCSAFLUSH, &orig_termios);
  write(STDOUT_FILENO, "\x1b[?25h", 6); // show cursor
}

static void enable_raw(void) {
  if (tcgetattr(STDIN_FILENO, &orig_termios) == -1) die("tcgetattr");
  atexit(disable_raw);

  struct termios raw = orig_termios;
  raw.c_iflag &= ~(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
  raw.c_oflag &= ~(OPOST);
  raw.c_cflag |= (CS8);
  raw.c_lflag &= ~(ECHO | ICANON | IEXTEN | ISIG);
  raw.c_cc[VMIN] = 1;
  raw.c_cc[VTIME] = 0;
  if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw) == -1) die("tcsetattr");

  // hide cursor, clear
  write(STDOUT_FILENO, "\x1b[?25l\x1b[2J\x1b[H", 12);
}

/* ----- window size ----- */
static int term_rows = 49, term_cols = 160;

static void update_winsize(void) {
  struct winsize ws;
  if (ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == -1 || ws.ws_col == 0) {
    // fallback: move to bottom-right to query; keep defaults if it fails
    write(STDOUT_FILENO, "\x1b[999C\x1b[999B", 12);
    return;
  } else {
    term_rows = ws.ws_row;
    term_cols = ws.ws_col;
  }
}

static void on_sigwinch(int sig) {
  (void)sig;
  update_winsize();
}

/* ----- buffer (just a vector of lines) ----- */
typedef struct {
  char **lines;
  size_t line_count;
  char  *filename;
  bool dirty;
} Buffer;

static void buf_init(Buffer *b, const char *fname) {
  b->lines = malloc(sizeof(char*));
  b->line_count = 1;
  b->lines[0] = strdup("");
  b->filename = strdup(fname ? fname : "untitled.txt");
  b->dirty = false;
}

static void buf_free(Buffer *b) {
  if (!b) return;
  for (size_t i = 0; i < b->line_count; ++i) free(b->lines[i]);
  free(b->lines);
  free(b->filename);
}

static void buf_load(Buffer *b, const char *path) {
  buf_init(b, path ? path : "untitled.txt");
  if (!path) return;

  FILE *f = fopen(path, "rb");
  if (!f) return;

  // read whole file
  fseek(f, 0, SEEK_END);
  long n = ftell(f);
  fseek(f, 0, SEEK_SET);
  if (n < 0) { fclose(f); return; }
  char *data = malloc((size_t)n + 1);
  if (!data) { fclose(f); return; }
  size_t rn = fread(data, 1, (size_t)n, f);
  data[rn] = '\0';
  fclose(f);

  // split into lines
  // reset current empty buffer
  for (size_t i = 0; i < b->line_count; ++i) free(b->lines[i]);
  free(b->lines);

  b->lines = NULL;
  b->line_count = 0;

  char *start = data;
  for (size_t i = 0; i <= rn; ++i) {
    if (data[i] == '\n' || data[i] == '\0') {
      size_t len = &data[i] - start;
      char *line = malloc(len + 1);
      memcpy(line, start, len);
      line[len] = '\0';
      b->lines = realloc(b->lines, sizeof(char*) * (b->line_count + 1));
      b->lines[b->line_count++] = line;
      start = &data[i + 1];
    }
  }
  if (b->line_count == 0) {
    b->lines = malloc(sizeof(char*));
    b->lines[0] = strdup("");
    b->line_count = 1;
  }
  free(data);
  b->dirty = false;
}

static size_t line_len(Buffer *b, size_t y) {
  if (y >= b->line_count) return 0;
  return strlen(b->lines[y]);
}

static void buf_insert_char(Buffer *b, size_t y, size_t x, char c) {
  if (y >= b->line_count) return;
  char *ln = b->lines[y];
  size_t n = strlen(ln);
  if (x > n) x = n;
  char *nl = malloc(n + 2);
  memcpy(nl, ln, x);
  nl[x] = c;
  memcpy(nl + x + 1, ln + x, n - x + 1);
  free(ln);
  b->lines[y] = nl;
  b->dirty = true;
}

static void buf_insert_newline(Buffer *b, size_t y, size_t x) {
  if (y >= b->line_count) return;
  char *ln = b->lines[y];
  size_t n = strlen(ln);
  if (x > n) x = n;

  char *left  = malloc(x + 1);
  char *right = strdup(ln + x);
  memcpy(left, ln, x);
  left[x] = '\0';

  b->lines[y] = left;
  b->lines = realloc(b->lines, sizeof(char*) * (b->line_count + 1));
  memmove(&b->lines[y + 2], &b->lines[y + 1], sizeof(char*) * (b->line_count - (y + 1)));
  b->lines[y + 1] = right;
  b->line_count++;
  free(ln);
  b->dirty = true;
}

static void buf_backspace(Buffer *b, size_t *y, size_t *x) {
  if (*y >= b->line_count) return;
  if (*x > 0) {
    char *ln = b->lines[*y];
    size_t n = strlen(ln);
    if (*x > n) *x = n;
    memmove(&ln[*x - 1], &ln[*x], n - *x + 1);
    (*x)--;
    b->dirty = true;
  } else if (*y > 0) {
    size_t prev_len = line_len(b, *y - 1);
    // merge current line into previous
    char *prev = b->lines[*y - 1];
    char *cur  = b->lines[*y];
    size_t pn = strlen(prev), cn = strlen(cur);
    prev = realloc(prev, pn + cn + 1);
    memcpy(prev + pn, cur, cn + 1);
    b->lines[*y - 1] = prev;

    // remove current line
    free(cur);
    memmove(&b->lines[*y], &b->lines[*y + 1], sizeof(char*) * (b->line_count - (*y + 1)));
    b->line_count--;
    (*y)--;
    *x = prev_len;
    b->dirty = true;
  }
}

static int buf_save(Buffer *b) {
  int fd = open(b->filename, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (fd < 0) return -1;
  for (size_t i = 0; i < b->line_count; ++i) {
    size_t n = strlen(b->lines[i]);
    if (write(fd, b->lines[i], n) != (ssize_t)n) { close(fd); return -1; }
    if (i + 1 < b->line_count) {
      if (write(fd, "\n", 1) != 1) { close(fd); return -1; }
    }
  }
  close(fd);
  b->dirty = false;
  return 0;
}

/* ----- editor state ----- */
typedef struct {
  Buffer buf;
  size_t cx, cy;         // cursor in file coords
  size_t row_off, col_off; // scroll offsets
  char status_msg[256];
  time_t status_at;
} Editor;

static Editor E;

static void set_status(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  vsnprintf(E.status_msg, sizeof(E.status_msg), fmt, ap);
  va_end(ap);
  E.status_at = time(NULL);
}

static void clamp_cursor(void) {
  if (E.cy >= E.buf.line_count) E.cy = E.buf.line_count ? E.buf.line_count - 1 : 0;
  size_t len = line_len(&E.buf, E.cy);
  if (E.cx > len) E.cx = len;
}

static void scroll(void) {
  int text_rows = term_rows - 2; // one for status, one for msg
  if (text_rows < 1) text_rows = 1;

  if (E.cy < E.row_off) E.row_off = E.cy;
  if (E.cy >= E.row_off + (size_t)text_rows) E.row_off = E.cy - (size_t)text_rows + 1;

  if (E.cx < E.col_off) E.col_off = E.cx;
  if (E.cx >= E.col_off + (size_t)term_cols) {
    E.col_off = E.cx - (size_t)term_cols + 1;
  }
}

/* ----- drawing ----- */

static void draw_rows(void) {
  int text_rows = term_rows - 2;
  if (text_rows < 1) text_rows = 1;

  for (int y = 0; y < text_rows; ++y) {
    write(STDOUT_FILENO, "\x1b[K", 3); // clear line
    size_t file_y = E.row_off + (size_t)y;
    if (file_y < E.buf.line_count) {
      char *ln = E.buf.lines[file_y];
      size_t len = strlen(ln);
      size_t start = (E.col_off < len) ? E.col_off : len;
      size_t end = len;
      if (end > start + (size_t)term_cols) end = start + (size_t)term_cols;
      write(STDOUT_FILENO, ln + start, end - start);
    } else {
      write(STDOUT_FILENO, "~", 1);
    }
    if (y < text_rows - 1) write(STDOUT_FILENO, "\r\n", 2);
  }
}

static void draw_status(void) {
  // status (reverse video)
  char buf[512];
  int n = snprintf(buf, sizeof(buf), "\x1b[7m %s%s | %zu:%zu \x1b[m",
                   E.buf.filename, E.buf.dirty ? " +" : "",
                   E.cy + 1, E.cx + 1);
  write(STDOUT_FILENO, "\r\n", 2);
  write(STDOUT_FILENO, "\x1b[K", 3);
  write(STDOUT_FILENO, buf, (size_t)n);

  // message line
  write(STDOUT_FILENO, "\r\n", 2);
  write(STDOUT_FILENO, "\x1b[K", 3);
  if (E.status_msg[0] && time(NULL) - E.status_at < 4) {
    write(STDOUT_FILENO, E.status_msg, strlen(E.status_msg));
  }
}

static void refresh_screen(void) {
  clamp_cursor();
  scroll();

  // move cursor home
  write(STDOUT_FILENO, "\x1b[H", 3);

  draw_rows();
  draw_status();

  // position cursor
  size_t rx = E.cx - (E.cx >= E.col_off ? E.col_off : E.cx);
  size_t ry = E.cy - (E.cy >= E.row_off ? E.row_off : E.cy);
  char cmdbuf[64];
  int m = snprintf(cmdbuf, sizeof(cmdbuf), "\x1b[%zu;%zuH", ry + 1, rx + 1);
  write(STDOUT_FILENO, cmdbuf, (size_t)m);
}

/* ----- input ----- */
enum Keys {
  KEY_ARROW_LEFT = 1000,
  KEY_ARROW_RIGHT,
  KEY_ARROW_UP,
  KEY_ARROW_DOWN,
  KEY_HOME,
  KEY_END,
  KEY_PAGE_UP,
  KEY_PAGE_DOWN
};

static int read_key(void) {
  char c;
  ssize_t nread;
  while ((nread = read(STDIN_FILENO, &c, 1)) != 1) {
    if (nread == -1 && errno != EAGAIN) die("read");
  }

  if (c == '\x1b') {
    char seq[3];
    if (read(STDIN_FILENO, &seq[0], 1) != 1) return '\x1b';
    if (read(STDIN_FILENO, &seq[1], 1) != 1) return '\x1b';

    if (seq[0] == '[') {
      if (seq[1] >= '0' && seq[1] <= '9') {
        if (read(STDIN_FILENO, &seq[2], 1) != 1) return '\x1b';
        if (seq[2] == '~') {
          switch (seq[1]) {
            case '1': return KEY_HOME;
            case '4': return KEY_END;
            case '5': return KEY_PAGE_UP;
            case '6': return KEY_PAGE_DOWN;
            case '7': return KEY_HOME;
            case '8': return KEY_END;
          }
        }
      } else {
        switch (seq[1]) {
          case 'A': return KEY_ARROW_UP;
          case 'B': return KEY_ARROW_DOWN;
          case 'C': return KEY_ARROW_RIGHT;
          case 'D': return KEY_ARROW_LEFT;
          case 'H': return KEY_HOME;
          case 'F': return KEY_END;
        }
      }
    }
    return '\x1b';
  }
  return (int)(unsigned char)c;
}

/* ----- movement & edit ----- */

static void move_cursor(int key) {
  switch (key) {
    case KEY_ARROW_LEFT:
      if (E.cx > 0) {
        E.cx--;
      } else if (E.cy > 0) {
        E.cy--;
        E.cx = line_len(&E.buf, E.cy);
      }
      break;
    case KEY_ARROW_RIGHT: {
      size_t len = line_len(&E.buf, E.cy);
      if (E.cx < len) {
        E.cx++;
      } else if (E.cy + 1 < E.buf.line_count) {
        E.cy++;
        E.cx = 0;
      }
    } break;
    case KEY_ARROW_UP:
      if (E.cy > 0) {
        E.cy--;
        size_t len = line_len(&E.buf, E.cy);
        if (E.cx > len) E.cx = len;
      }
      break;
    case KEY_ARROW_DOWN:
      if (E.cy + 1 < E.buf.line_count) {
        E.cy++;
        size_t len = line_len(&E.buf, E.cy);
        if (E.cx > len) E.cx = len;
      }
      break;
    case KEY_HOME:
      E.cx = 0; break;
    case KEY_END:
      E.cx = line_len(&E.buf, E.cy); break;
    case KEY_PAGE_UP:
    case KEY_PAGE_DOWN: {
      int rows = term_rows - 2;
      if (rows < 1) rows = 1;
      if (key == KEY_PAGE_UP) {
        if ((int)E.cy - rows < 0) E.cy = 0;
        else E.cy -= (size_t)rows;
      } else {
        size_t maxy = E.buf.line_count ? E.buf.line_count - 1 : 0;
        E.cy += (size_t)rows;
        if (E.cy > maxy) E.cy = maxy;
      }
      size_t len = line_len(&E.buf, E.cy);
      if (E.cx > len) E.cx = len;
    } break;
  }
}

static void insert_char(int c) {
  if (c == '\r') c = '\n';
  if (c == '\n') {
    buf_insert_newline(&E.buf, E.cy, E.cx);
    E.cy++; E.cx = 0;
  } else if (c == '\t') {
    buf_insert_char(&E.buf, E.cy, E.cx, '\t');
    E.cx++;
  } else if (c >= 32 && c <= 126) {
    buf_insert_char(&E.buf, E.cy, E.cx, (char)c);
    E.cx++;
  }
}

/* ----- main ----- */

int main(int argc, char **argv) {
  signal(SIGWINCH, on_sigwinch);

  const char *path = argc > 1 ? argv[1] : NULL;
  buf_load(&E.buf, path ? path : NULL);

  update_winsize();
  enable_raw();
  set_status("insert-only | arrows/Home/End | Enter/Backspace | Ctrl+S save | Ctrl+C quit");

  while (1) {
    refresh_screen();

    int c = read_key();

    if (c == CTRL_KEY('c')) {
      // quit
      write(STDOUT_FILENO, "\x1b[2J\x1b[H", 7);
      break;
    }
    if (c == CTRL_KEY('s')) {
      if (buf_save(&E.buf) == 0) set_status("saved.");
      else set_status("save error: %s", strerror(errno));
      continue;
    }

    switch (c) {
      case KEY_ARROW_LEFT:
      case KEY_ARROW_RIGHT:
      case KEY_ARROW_UP:
      case KEY_ARROW_DOWN:
      case KEY_HOME:
      case KEY_END:
      case KEY_PAGE_UP:
      case KEY_PAGE_DOWN:
        move_cursor(c);
        break;
      case 127: // Backspace
      case CTRL_KEY('h'):
        buf_backspace(&E.buf, &E.cy, &E.cx);
        break;
      case '\r':
      case '\n':
      case '\t':
      default:
        if (c == '\r' || c == '\n' || c == '\t' || (c >= 32 && c <= 126)) {
          insert_char(c);
        }
        // ignore other control chars
        break;
    }
  }

  buf_free(&E.buf);
  return 0;
}
