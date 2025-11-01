// main.c — MiniPy runner: script file + REPL (MIT)
#include "ast.h"
#include "lexer.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static char *slurp(const char *path) {
  FILE *f = fopen(path, "rb");
  if (!f)
    return NULL;
  fseek(f, 0, SEEK_END);
  long n = ftell(f);
  fseek(f, 0, SEEK_SET);
  char *buf = (char *)malloc((size_t)n + 1);
  if (!buf) {
    fclose(f);
    return NULL;
  }
  size_t rd = fread(buf, 1, (size_t)n, f);
  fclose(f);
  buf[rd] = 0;
  return buf;
}

static int run_source(const char *code) {
  Lexer lx;
  lexer_init(&lx, code);
  ParseResult r = parse_module(&lx);
  if (!r.ok) {
    fprintf(stderr, "parse failed\n");
    return 1;
  }
  return eval_module(&r.mod);
}

static void repl(void) {
  puts("MiniPy REPL — blank line to run, :q to quit");
  fflush(stdout);
  char line[1024];

  // accumulating buffer for a block
  char *buf = NULL;
  size_t cap = 0, len = 0;

  for (;;) {
    fputs(len ? "... " : ">>> ", stdout);
    fflush(stdout);
    if (!fgets(line, sizeof line, stdin))
      break;

    // trim trailing \r\n
    size_t L = strlen(line);
    while (L && (line[L - 1] == '\n' || line[L - 1] == '\r'))
      line[--L] = 0;

    if (strcmp(line, ":q") == 0)
      break;

    // blank line => execute accumulated code
    if (L == 0) {
      if (len) {
        // NUL-terminate
        if (len == cap) {
          cap = cap ? cap * 2 : 1024;
          buf = (char *)realloc(buf, cap);
        }
        buf[len] = 0;

        int rc = run_source(buf);
        if (rc != 0) {
          // keep buffer to allow quick edits; or clear—here we clear
        }
        // clear buffer for next block
        len = 0;
      }
      continue;
    }

    // grow buffer and append this line + newline
    size_t need = len + L + 1 /*\n*/ + 1 /*\0*/;
    if (need > cap) {
      size_t newcap = cap ? cap : 1024;
      while (need > newcap)
        newcap *= 2;
      char *nb = (char *)realloc(buf, newcap);
      if (!nb) {
        fprintf(stderr, "out of memory\n");
        free(buf);
        fflush(stdout);
        return;
      }
      buf = nb;
      cap = newcap;
    }
    memcpy(buf + len, line, L);
    len += L;
    buf[len++] = '\n';
    // keep NUL at end for convenience
    buf[len] = 0;
  }

  free(buf);
}

int main(int argc, char **argv) {
  if (argc > 1) {
    char *code = slurp(argv[1]);
    if (!code) {
      fprintf(stderr, "cannot read %s\n", argv[1]);
      return 1;
    }
    int rc = run_source(code);
    free(code);
    return rc;
  }

  repl();
  return 0;
}
