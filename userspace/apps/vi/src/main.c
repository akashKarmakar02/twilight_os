#include <ctype.h>
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

#define CLR_RESET "\x1b[0m"
#define CLR_TAG     "\x1b[38;5;33m"  /* blue (tag names) */
#define CLR_ATTR    "\x1b[38;5;37m"  /* light cyan (attr names) */
#define CLR_KEYWORD "\x1b[38;5;33m"  // blue
#define CLR_STRING "\x1b[38;5;166m"  // orange
#define CLR_COMMENT "\x1b[38;5;242m" // gray
#define CLR_NUMBER "\x1b[38;5;141m"  // purple

typedef enum { LANG_PLAIN = 0, LANG_C, LANG_HTML, LANG_PYTHON, LANG_LUA } Lang;

typedef struct {
  char **lines;
  size_t line_count;
  char *filename;
  bool dirty;
} Buffer;

typedef struct {
  Buffer buf;
  size_t cx, cy;
  size_t row_off, col_off;
  char status_msg[256];
  time_t status_at;
  Lang lang;
  bool cursor_visible;
} Editor;

static Editor E = {0};

static const char *keywords[] = {
    "int",      "char",   "void",   "if",       "else",    "for",
    "while",    "return", "static", "struct",   "typedef", "const",
    "unsigned", "signed", "long",   "short",    "float",   "double",
    "include",  "define", "break",  "continue", NULL};

static const char *python_keywords[] = {
    "and",      "as",       "assert",   "break",    "class",   "continue",
    "def",      "del",      "elif",     "else",     "except",  "exec",
    "finally",  "for",      "from",     "global",   "if",      "import",
    "in",       "is",       "lambda",   "not",      "or",      "pass",
    "print",    "raise",    "return",   "try",      "while",   "with",
    "yield",    "False",    "True",     "None",     NULL};

static const char *lua_keywords[] = { // NEW
    "and", "break", "do", "else", "elseif", "end", "false", "for",
    "function", "goto", "if", "in", "local", "nil", "not", "or",
    "repeat", "return", "then", "true", "until", "while",
    NULL
};

static Lang detect_lang(const char *fname) {
  const char *n = fname ? strrchr(fname, '.') : NULL;
  if (!n) return LANG_PLAIN;
  n++;
  if (!strcasecmp(n, "c") || !strcasecmp(n, "h") ||
      !strcasecmp(n, "cpp") || !strcasecmp(n, "hpp") ||
      !strcasecmp(n, "cc") || !strcasecmp(n, "hh"))
    return LANG_C;
  if (!strcasecmp(n, "html") || !strcasecmp(n, "htm"))
    return LANG_HTML;
  if (!strcasecmp(n, "py") || !strcasecmp(n, "pyw"))
    return LANG_PYTHON;
  if (!strcasecmp(n, "lua")) // NEW
    return LANG_LUA;
  return LANG_PLAIN;
}

static void cursor_set(bool vis) {
  static int cur = 1; // assume visible at process start
  if ((int)vis == cur) return;
  if (vis) write(STDOUT_FILENO, "\x1b[?25h", 6);
  else     write(STDOUT_FILENO, "\x1b[?25l", 6);
  cur = (int)vis;
}

static void draw_highlighted_c(const char *line) {
  const char *p = line;
  while (*p) {
    // comment
    if (*p == '/' && *(p + 1) == '/') {
      write(STDOUT_FILENO, CLR_COMMENT, strlen(CLR_COMMENT));
      write(STDOUT_FILENO, p, strlen(p));
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      return;
    }

    // string literal
    if (*p == '"' || *p == '\'') {
      char quote = *p++;
      write(STDOUT_FILENO, CLR_STRING, strlen(CLR_STRING));
      write(STDOUT_FILENO, &quote, 1);
      while (*p && *p != quote) {
        if (*p == '\\' && *(p + 1)) {
          write(STDOUT_FILENO, p, 2);
          p += 2;
          continue;
        }
        write(STDOUT_FILENO, p, 1);
        p++;
      }
      if (*p == quote) {
        write(STDOUT_FILENO, p, 1);
        p++;
      }
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    // number
    if ((*p >= '0' && *p <= '9') &&
        (p == line || !isalnum((unsigned char)*(p - 1)))) {
      const char *start = p;
      while (isdigit((unsigned char)*p) || *p == '.')
        p++;
      write(STDOUT_FILENO, CLR_NUMBER, strlen(CLR_NUMBER));
      write(STDOUT_FILENO, start, p - start);
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    // keyword
    if (isalpha((unsigned char)*p) || *p == '_') {
      const char *start = p;
      while (isalnum((unsigned char)*p) || *p == '_')
        p++;
      size_t len = p - start;
      bool matched = false;
      for (int i = 0; keywords[i]; i++) {
        if (strlen(keywords[i]) == len && !strncmp(start, keywords[i], len)) {
          matched = true;
          break;
        }
      }
      if (matched) {
        write(STDOUT_FILENO, CLR_KEYWORD, strlen(CLR_KEYWORD));
        write(STDOUT_FILENO, start, len);
        write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      } else {
        write(STDOUT_FILENO, start, len);
      }
      continue;
    }

    // default
    write(STDOUT_FILENO, p, 1);
    p++;
  }
}

static void draw_highlighted_python(const char *line) {
  const char *p = line;
  while (*p) {
    // Comment (starts with #)
    if (*p == '#') {
      write(STDOUT_FILENO, CLR_COMMENT, strlen(CLR_COMMENT));
      write(STDOUT_FILENO, p, strlen(p));
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      return;
    }

    // Triple-quoted strings (""" or ''')
    if ((p[0] == '"' && p[1] == '"' && p[2] == '"') ||
        (p[0] == '\'' && p[1] == '\'' && p[2] == '\'')) {
      char quote = p[0];
      write(STDOUT_FILENO, CLR_STRING, strlen(CLR_STRING));
      write(STDOUT_FILENO, p, 3);
      p += 3;
      // Find closing triple quote
      while (*p) {
        if (p[0] == quote && p[1] == quote && p[2] == quote) {
          write(STDOUT_FILENO, p, 3);
          p += 3;
          break;
        }
        write(STDOUT_FILENO, p, 1);
        p++;
      }
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    // String literal (single or double quote)
    if (*p == '"' || *p == '\'') {
      char quote = *p++;
      write(STDOUT_FILENO, CLR_STRING, strlen(CLR_STRING));
      write(STDOUT_FILENO, &quote, 1);
      while (*p && *p != quote) {
        if (*p == '\\' && *(p + 1)) {
          write(STDOUT_FILENO, p, 2);
          p += 2;
          continue;
        }
        write(STDOUT_FILENO, p, 1);
        p++;
      }
      if (*p == quote) {
        write(STDOUT_FILENO, p, 1);
        p++;
      }
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    // Number
    if ((*p >= '0' && *p <= '9') &&
        (p == line || (!isalnum((unsigned char)*(p - 1)) && *(p - 1) != '.'))) {
      const char *start = p;
      while (isdigit((unsigned char)*p) || *p == '.' || 
             *p == 'e' || *p == 'E' || *p == '+' || *p == '-')
        p++;
      write(STDOUT_FILENO, CLR_NUMBER, strlen(CLR_NUMBER));
      write(STDOUT_FILENO, start, p - start);
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    // Keyword
    if (isalpha((unsigned char)*p) || *p == '_') {
      const char *start = p;
      while (isalnum((unsigned char)*p) || *p == '_')
        p++;
      size_t len = p - start;
      bool matched = false;
      for (int i = 0; python_keywords[i]; i++) {
        if (strlen(python_keywords[i]) == len && !strncmp(start, python_keywords[i], len)) {
          matched = true;
          break;
        }
      }
      if (matched) {
        write(STDOUT_FILENO, CLR_KEYWORD, strlen(CLR_KEYWORD));
        write(STDOUT_FILENO, start, len);
        write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      } else {
        write(STDOUT_FILENO, start, len);
      }
      continue;
    }

    // Default
    write(STDOUT_FILENO, p, 1);
    p++;
  }
}

static void draw_highlighted_html(const char *line) {
  const char *p = line;

  while (*p) {
    /* HTML comment <!-- ... --> */
    if (p[0] == '<' && p[1] == '!' && p[2] == '-' && p[3] == '-') {
      const char *q = strstr(p + 4, "-->");
      write(STDOUT_FILENO, CLR_COMMENT, strlen(CLR_COMMENT));
      if (q) {
        write(STDOUT_FILENO, p, (q + 3) - p);
        p = q + 3;
      } else {
        write(STDOUT_FILENO, p, strlen(p));
        p += strlen(p);
      }
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    /* DOCTYPE */
    if (p[0] == '<' && (p[1] == '!' || p[1] == '?')) {
      const char *q = strchr(p, '>');
      write(STDOUT_FILENO, CLR_COMMENT, strlen(CLR_COMMENT));
      if (q) {
        write(STDOUT_FILENO, p, (q + 1) - p);
        p = q + 1;
      } else {
        write(STDOUT_FILENO, p, strlen(p));
        p += strlen(p);
      }
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    /* Entity: &amp; &lt; &#123; */
    if (*p == '&') {
      const char *q = strchr(p, ';');
      if (q && q - p <= 32) {
        write(STDOUT_FILENO, CLR_NUMBER, strlen(CLR_NUMBER));
        write(STDOUT_FILENO, p, (q + 1) - p);
        write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
        p = q + 1;
        continue;
      }
    }

    /* Tag: <tag ...> or </tag ...> */
    if (*p == '<') {
      const char *s = p;
      /* print '<' or '</' */
      write(STDOUT_FILENO, "<", 1);
      p++;
      if (*p == '/') { write(STDOUT_FILENO, "/", 1); p++; }

      /* tag name */
      const char *tn_start = p;
      while (*p && (isalnum((unsigned char)*p) || *p=='-' || *p==':' ))
        p++;
      size_t tn_len = p - tn_start;
      if (tn_len > 0) {
        write(STDOUT_FILENO, CLR_TAG, strlen(CLR_TAG));
        write(STDOUT_FILENO, tn_start, tn_len);
        write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      }

      /* attrs until '>' */
      while (*p && *p != '>') {
        if (isspace((unsigned char)*p)) {
          write(STDOUT_FILENO, p, 1);
          p++;
          continue;
        }

        /* '/>' self-close */
        if (p[0] == '/' && p[1] == '>') {
          write(STDOUT_FILENO, "/>", 2);
          p += 2;
          goto tag_done;
        }

        /* attr name */
        const char *an_start = p;
        if (isalpha((unsigned char)*p) || *p=='_' || *p==':' || *p=='-') {
          while (*p && (isalnum((unsigned char)*p) || *p=='_' || *p==':' || *p=='-' || *p=='.'))
            p++;
          size_t an_len = p - an_start;
          write(STDOUT_FILENO, CLR_ATTR, strlen(CLR_ATTR));
          write(STDOUT_FILENO, an_start, an_len);
          write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
        }

        /* optional spaces */
        while (isspace((unsigned char)*p)) { write(STDOUT_FILENO, p, 1); p++; }

        /* '=' and value */
        if (*p == '=') {
          write(STDOUT_FILENO, "=", 1); p++;
          while (isspace((unsigned char)*p)) { write(STDOUT_FILENO, p, 1); p++; }
          if (*p == '"' || *p == '\'') {
            char q = *p++;
            write(STDOUT_FILENO, &q, 1);
            write(STDOUT_FILENO, CLR_STRING, strlen(CLR_STRING));
            while (*p && *p != q) {
              if (*p == '\\' && p[1]) { write(STDOUT_FILENO, p, 2); p+=2; }
              else { write(STDOUT_FILENO, p, 1); p++; }
            }
            write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
            if (*p == q) { write(STDOUT_FILENO, &q, 1); p++; }
          } else {
            /* unquoted value */
            const char *vv = p;
            while (*p && !isspace((unsigned char)*p) && *p!='>')
              p++;
            write(STDOUT_FILENO, CLR_STRING, strlen(CLR_STRING));
            write(STDOUT_FILENO, vv, p - vv);
            write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
          }
        }
      }

      if (*p == '>') { write(STDOUT_FILENO, ">", 1); p++; }
tag_done:
      continue;
    }

    /* Outside tags: plain text */
    write(STDOUT_FILENO, p, 1);
    p++;
  }
}

static void draw_highlighted_lua(const char *line) { // NEW
  const char *p = line;
  while (*p) {
    // Single-line comment --
    if (p[0] == '-' && p[1] == '-') {
      // Long comment block --[[ ... ]]
      if (p[2] == '[' && p[3] == '[') {
        const char *q = strstr(p + 4, "]]");
        write(STDOUT_FILENO, CLR_COMMENT, strlen(CLR_COMMENT));
        if (q) {
          write(STDOUT_FILENO, p, (q + 2) - p);
          p = q + 2;
        } else {
          write(STDOUT_FILENO, p, strlen(p));
          p += strlen(p);
        }
        write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
        continue;
      }
      // Normal single-line comment
      write(STDOUT_FILENO, CLR_COMMENT, strlen(CLR_COMMENT));
      write(STDOUT_FILENO, p, strlen(p));
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      return;
    }

    // String literals: "..." or '...' or [[ ... ]]
    if (*p == '"' || *p == '\'') {
      char quote = *p++;
      write(STDOUT_FILENO, CLR_STRING, strlen(CLR_STRING));
      write(STDOUT_FILENO, &quote, 1);
      while (*p && *p != quote) {
        if (*p == '\\' && p[1]) {
          write(STDOUT_FILENO, p, 2);
          p += 2;
        } else {
          write(STDOUT_FILENO, p, 1);
          p++;
        }
      }
      if (*p == quote) {
        write(STDOUT_FILENO, p, 1);
        p++;
      }
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }
    if (p[0] == '[' && p[1] == '[') {
      const char *q = strstr(p + 2, "]]");
      write(STDOUT_FILENO, CLR_STRING, strlen(CLR_STRING));
      if (q) {
        write(STDOUT_FILENO, p, (q + 2) - p);
        p = q + 2;
      } else {
        write(STDOUT_FILENO, p, strlen(p));
        p += strlen(p);
      }
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    // Number literal
    if (isdigit((unsigned char)*p) &&
        (p == line || !isalnum((unsigned char)*(p - 1)))) {
      const char *start = p;
      while (isdigit((unsigned char)*p) || *p == '.' || *p == 'e' || *p == 'E' ||
             *p == '+' || *p == '-') {
        p++;
      }
      write(STDOUT_FILENO, CLR_NUMBER, strlen(CLR_NUMBER));
      write(STDOUT_FILENO, start, p - start);
      write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      continue;
    }

    // Keywords and identifiers
    if (isalpha((unsigned char)*p) || *p == '_') {
      const char *start = p;
      while (isalnum((unsigned char)*p) || *p == '_') p++;
      size_t len = p - start;
      bool matched = false;
      for (int i = 0; lua_keywords[i]; i++) {
        if (strlen(lua_keywords[i]) == len && !strncmp(start, lua_keywords[i], len)) {
          matched = true;
          break;
        }
      }
      if (matched) {
        write(STDOUT_FILENO, CLR_KEYWORD, strlen(CLR_KEYWORD));
        write(STDOUT_FILENO, start, len);
        write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
      } else {
        // function name highlight: foo = function
        if (strncmp(start, "function", len) == 0) {
          write(STDOUT_FILENO, CLR_KEYWORD, strlen(CLR_KEYWORD));
          write(STDOUT_FILENO, start, len);
          write(STDOUT_FILENO, CLR_RESET, strlen(CLR_RESET));
        } else {
          write(STDOUT_FILENO, start, len);
        }
      }
      continue;
    }

    // Default char
    write(STDOUT_FILENO, p, 1);
    p++;
  }
}

static void draw_highlighted(const char *line) {
  switch (E.lang) {
    case LANG_HTML: draw_highlighted_html(line); break;
    case LANG_C:    draw_highlighted_c(line);    break;
    case LANG_PYTHON: draw_highlighted_python(line); break;
    case LANG_LUA:    draw_highlighted_lua(line);     break;
    default:        write(STDOUT_FILENO, line, strlen(line)); break;
  }
}

struct termios orig_termios;

static void die(const char *msg) {
  tcsetattr(STDIN_FILENO, TCSAFLUSH, &orig_termios);
  cursor_set(true);                           // ensure visible
  write(STDOUT_FILENO, "\x1b[0m\x1b[H\x1b[2J", 10);
  perror(msg);
  exit(1);
}

static void disable_raw(void) {
  tcsetattr(STDIN_FILENO, TCSAFLUSH, &orig_termios);
  write(STDOUT_FILENO, "\x1b[?25h", 6);
  cursor_set(true);
}

static void enable_raw(void) {
  if (tcgetattr(STDIN_FILENO, &orig_termios) == -1)
    die("tcgetattr");
  atexit(disable_raw);

  struct termios raw = orig_termios;
  raw.c_iflag &= ~(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
  raw.c_oflag &= ~(OPOST);
  raw.c_cflag |= (CS8);
  raw.c_lflag &= ~(ECHO | ICANON | IEXTEN | ISIG);
  raw.c_cc[VMIN] = 1;
  raw.c_cc[VTIME] = 0;
  if (tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw) == -1)
    die("tcsetattr");

  write(STDOUT_FILENO, "\x1b[2J\x1b[H", 7);
}

static int term_rows = 49, term_cols = 160;

static void update_winsize(void) {
  struct winsize ws;
  if (ioctl(STDOUT_FILENO, TIOCGWINSZ, &ws) == -1 || ws.ws_col == 0) {
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

static void buf_init(Buffer *b, const char *fname) {
  b->lines = malloc(sizeof(char *));
  b->line_count = 1;
  b->lines[0] = strdup("");
  b->filename = strdup(fname ? fname : "untitled.txt");
  b->dirty = false;
}

static void buf_free(Buffer *b) {
  if (!b)
    return;
  for (size_t i = 0; i < b->line_count; ++i)
    free(b->lines[i]);
  free(b->lines);
  free(b->filename);
}

static void buf_load(Buffer *b, const char *path) {
  buf_init(b, path ? path : "untitled.txt");
  if (!path)
    return;

  FILE *f = fopen(path, "rb");
  if (!f)
    return;

  fseek(f, 0, SEEK_END);
  long n = ftell(f);
  fseek(f, 0, SEEK_SET);
  if (n < 0) {
    fclose(f);
    return;
  }
  char *data = malloc((size_t)n + 1);
  if (!data) {
    fclose(f);
    return;
  }
  size_t rn = fread(data, 1, (size_t)n, f);
  data[rn] = '\0';
  fclose(f);

  for (size_t i = 0; i < b->line_count; ++i)
    free(b->lines[i]);
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
      b->lines = realloc(b->lines, sizeof(char *) * (b->line_count + 1));
      b->lines[b->line_count++] = line;
      start = &data[i + 1];
    }
  }
  if (b->line_count == 0) {
    b->lines = malloc(sizeof(char *));
    b->lines[0] = strdup("");
    b->line_count = 1;
  }
  E.lang = detect_lang(b->filename);
  free(data);
  b->dirty = false;
}

static size_t line_len(Buffer *b, size_t y) {
  if (y >= b->line_count)
    return 0;
  return strlen(b->lines[y]);
}

static void buf_insert_char(Buffer *b, size_t y, size_t x, char c) {
  if (y >= b->line_count)
    return;
  char *ln = b->lines[y];
  size_t n = strlen(ln);
  if (x > n)
    x = n;
  char *nl = malloc(n + 2);
  memcpy(nl, ln, x);
  nl[x] = c;
  memcpy(nl + x + 1, ln + x, n - x + 1);
  free(ln);
  b->lines[y] = nl;
  b->dirty = true;
}

static void buf_insert_newline(Buffer *b, size_t y, size_t x) {
  if (y >= b->line_count)
    return;
  char *ln = b->lines[y];
  size_t n = strlen(ln);
  if (x > n)
    x = n;

  char *left = malloc(x + 1);
  char *right = strdup(ln + x);
  memcpy(left, ln, x);
  left[x] = '\0';

  b->lines[y] = left;
  b->lines = realloc(b->lines, sizeof(char *) * (b->line_count + 1));
  memmove(&b->lines[y + 2], &b->lines[y + 1],
          sizeof(char *) * (b->line_count - (y + 1)));
  b->lines[y + 1] = right;
  b->line_count++;
  free(ln);
  b->dirty = true;
}

static void buf_backspace(Buffer *b, size_t *y, size_t *x) {
  if (*y >= b->line_count)
    return;
  if (*x > 0) {
    char *ln = b->lines[*y];
    size_t n = strlen(ln);
    if (*x > n)
      *x = n;
    memmove(&ln[*x - 1], &ln[*x], n - *x + 1);
    (*x)--;
    b->dirty = true;
  } else if (*y > 0) {
    size_t prev_len = line_len(b, *y - 1);
    char *prev = b->lines[*y - 1];
    char *cur = b->lines[*y];
    size_t pn = strlen(prev), cn = strlen(cur);
    prev = realloc(prev, pn + cn + 1);
    memcpy(prev + pn, cur, cn + 1);
    b->lines[*y - 1] = prev;

    free(cur);
    memmove(&b->lines[*y], &b->lines[*y + 1],
            sizeof(char *) * (b->line_count - (*y + 1)));
    b->line_count--;
    (*y)--;
    *x = prev_len;
    b->dirty = true;
  }
}

static int buf_save(Buffer *b) {
  int fd = open(b->filename, O_WRONLY | O_CREAT | O_TRUNC, 0644);
  if (fd < 0)
    return -1;
  for (size_t i = 0; i < b->line_count; ++i) {
    size_t n = strlen(b->lines[i]);
    if (write(fd, b->lines[i], n) != (ssize_t)n) {
      close(fd);
      return -1;
    }
    if (i + 1 < b->line_count) {
      if (write(fd, "\n", 1) != 1) {
        close(fd);
        return -1;
      }
    }
  }
  close(fd);
  b->dirty = false;
  return 0;
}

static void set_status(const char *fmt, ...) {
  va_list ap;
  va_start(ap, fmt);
  vsnprintf(E.status_msg, sizeof(E.status_msg), fmt, ap);
  va_end(ap);
  E.status_at = time(NULL);
}

static void clamp_cursor(void) {
  if (E.cy >= E.buf.line_count)
    E.cy = E.buf.line_count ? E.buf.line_count - 1 : 0;
  size_t len = line_len(&E.buf, E.cy);
  if (E.cx > len)
    E.cx = len;
}

static void scroll(void) {
  int text_rows = term_rows - 2;
  if (text_rows < 1)
    text_rows = 1;

  if (E.cy < E.row_off)
    E.row_off = E.cy;
  if (E.cy >= E.row_off + (size_t)text_rows)
    E.row_off = E.cy - (size_t)text_rows + 1;

  if (E.cx < E.col_off)
    E.col_off = E.cx;
  if (E.cx >= E.col_off + (size_t)term_cols) {
    E.col_off = E.cx - (size_t)term_cols + 1;
  }
}

/* ----- drawing ----- */

static void draw_rows(void) {
  int text_rows = term_rows - 2;
  if (text_rows < 1)
    text_rows = 1;

  for (int y = 0; y < text_rows; ++y) {
    write(STDOUT_FILENO, "\x1b[K", 3);
    size_t file_y = E.row_off + (size_t)y;
    if (file_y < E.buf.line_count) {
      char *ln = E.buf.lines[file_y];
      size_t len = strlen(ln);
      size_t start = (E.col_off < len) ? E.col_off : len;
      size_t end = len;
      if (end > start + (size_t)term_cols)
        end = start + (size_t)term_cols;
      char tmp[term_cols + 1];
      memcpy(tmp, ln + start, end - start);
      tmp[end - start] = '\0';
      draw_highlighted(tmp);
    } else {
      write(STDOUT_FILENO, "~", 1);
    }
    if (y < text_rows - 1)
      write(STDOUT_FILENO, "\r\n", 2);
  }
}

static void draw_status(void) {
  char buf[512];
  const char *lang_name = "Plain";
  if (E.lang == LANG_HTML) lang_name = "HTML";
  else if (E.lang == LANG_C) lang_name = "C";
  else if (E.lang == LANG_PYTHON) lang_name = "Python";
  int n = snprintf(buf, sizeof(buf), "\x1b[7m %s%s | %s | %zu:%zu \x1b[m",
                   E.buf.filename, E.buf.dirty ? " +" : "",
                   lang_name, E.cy + 1, E.cx + 1);
  write(STDOUT_FILENO, "\r\n", 2);
  write(STDOUT_FILENO, "\x1b[K", 3);
  write(STDOUT_FILENO, buf, (size_t)n);

  write(STDOUT_FILENO, "\r\n", 2);
  write(STDOUT_FILENO, "\x1b[K", 3);
  if (E.status_msg[0] && time(NULL) - E.status_at < 4) {
    write(STDOUT_FILENO, E.status_msg, strlen(E.status_msg));
  }
}

static void refresh_screen(void) {
  clamp_cursor();
  scroll();

  write(STDOUT_FILENO, "\x1b[H", 3);

  draw_rows();
  draw_status();

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
    if (nread == -1 && errno != EAGAIN)
      die("read");
  }

  if (c == '\x1b') {
    char seq[3];
    if (read(STDIN_FILENO, &seq[0], 1) != 1)
      return '\x1b';
    if (read(STDIN_FILENO, &seq[1], 1) != 1)
      return '\x1b';

    if (seq[0] == '[') {
      if (seq[1] >= '0' && seq[1] <= '9') {
        if (read(STDIN_FILENO, &seq[2], 1) != 1)
          return '\x1b';
        if (seq[2] == '~') {
          switch (seq[1]) {
          case '1':
            return KEY_HOME;
          case '4':
            return KEY_END;
          case '5':
            return KEY_PAGE_UP;
          case '6':
            return KEY_PAGE_DOWN;
          case '7':
            return KEY_HOME;
          case '8':
            return KEY_END;
          }
        }
      } else {
        switch (seq[1]) {
        case 'A':
          return KEY_ARROW_UP;
        case 'B':
          return KEY_ARROW_DOWN;
        case 'C':
          return KEY_ARROW_RIGHT;
        case 'D':
          return KEY_ARROW_LEFT;
        case 'H':
          return KEY_HOME;
        case 'F':
          return KEY_END;
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
      if (E.cx > len)
        E.cx = len;
    }
    break;
  case KEY_ARROW_DOWN:
    if (E.cy + 1 < E.buf.line_count) {
      E.cy++;
      size_t len = line_len(&E.buf, E.cy);
      if (E.cx > len)
        E.cx = len;
    }
    break;
  case KEY_HOME:
    E.cx = 0;
    break;
  case KEY_END:
    E.cx = line_len(&E.buf, E.cy);
    break;
  case KEY_PAGE_UP:
  case KEY_PAGE_DOWN: {
    int rows = term_rows - 2;
    if (rows < 1)
      rows = 1;
    if (key == KEY_PAGE_UP) {
      if ((int)E.cy - rows < 0)
        E.cy = 0;
      else
        E.cy -= (size_t)rows;
    } else {
      size_t maxy = E.buf.line_count ? E.buf.line_count - 1 : 0;
      E.cy += (size_t)rows;
      if (E.cy > maxy)
        E.cy = maxy;
    }
    size_t len = line_len(&E.buf, E.cy);
    if (E.cx > len)
      E.cx = len;
  } break;
  }
}

static void insert_char(int c) {
  if (c == '\r')
    c = '\n';
  if (c == '\n') {
    buf_insert_newline(&E.buf, E.cy, E.cx);
    E.cy++;
    E.cx = 0;
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
  E.cursor_visible = true;

  update_winsize();
  enable_raw();
  set_status("insert-only | arrows/Home/End | Enter/Backspace | Ctrl+S save | "
             "Ctrl+C quit");

  while (1) {
    refresh_screen();

    int c = read_key();

    if (c == CTRL_KEY('c')) {
      write(STDOUT_FILENO, "\x1b[2J\x1b[H", 7);
      break;
    }
    if (c == CTRL_KEY('v')) {          // Ctrl+V toggles visibility
       E.cursor_visible = !E.cursor_visible;
       set_status("cursor %s", E.cursor_visible ? "shown" : "hidden");
       continue;
    }
    if (c == CTRL_KEY('s')) {
      if (buf_save(&E.buf) == 0)
        set_status("saved.");
      else
        set_status("save error: %s", strerror(errno));
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
    case 127:
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
      break;
    }
  }

  buf_free(&E.buf);
  return 0;
}
