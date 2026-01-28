#include "parser.h"
#include <ctype.h>
#include <stdlib.h>
#include <string.h>

void free_argv(char **argv) {
  if (!argv)
    return;
  for (size_t i = 0; argv[i]; i++)
    free(argv[i]);
  free(argv);
}

int parse_argv(const char *line, char ***argv_out) {
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

void free_strv(char **v) {
  if (!v)
    return;
  for (size_t i = 0; v[i]; i++)
    free(v[i]);
  free(v);
}

int split_pipeline(const char *line, char ***out_segs) {
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

void rstrip_newline(char *s) {
  size_t n = strlen(s);
  if (n && s[n - 1] == '\n')
    s[n - 1] = '\0';
}

static inline int ascii_isspace(unsigned char c) {
  return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\v' ||
         c == '\f';
}

char *lstrip(char *s) {
  while (*s && ascii_isspace((unsigned char)*s))
    s++;
  return s;
}

char *trim(char *s) {
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
