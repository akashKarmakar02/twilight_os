#include <stdio.h>

typedef long long i64;

static int is_space(int c) {
  return c == ' ' || c == '\t' || c == '\r' || c == '\f' || c == '\v';
}
static int is_digit(int c) { return c >= '0' && c <= '9'; }
static int to_lower(int c) {
  return (c >= 'A' && c <= 'Z') ? c + ('a' - 'A') : c;
}

static int read_line(char *buf, int cap) {
  int n = 0, c;
  while ((c = getchar()) != EOF) {
    if (c == '\r')
      continue;
    if (c == '\n')
      break;
    if (n < cap - 1)
      buf[n++] = (char)c;
  }
  buf[n] = '\0';
  return (c == EOF && n == 0) ? 0 : n;
}

typedef struct {
  const char *s;
  int i;
} Lex;
static void skip_ws(Lex *L) {
  while (is_space((unsigned char)L->s[L->i]) || L->s[L->i] == ' ')
    L->i++;
}
static int eat(Lex *L, int c) {
  skip_ws(L);
  if (L->s[L->i] == c) {
    L->i++;
    return 1;
  }
  return 0;
}

static i64 parse_expr(Lex *L, int *ok);

static i64 parse_number(Lex *L, int *ok) {
  skip_ws(L);
  int neg = 0;
  if (L->s[L->i] == '-') {
    neg = 1;
    L->i++;
  }
  if (!is_digit((unsigned char)L->s[L->i])) {
    *ok = 0;
    return 0;
  }
  i64 v = 0;
  while (is_digit((unsigned char)L->s[L->i])) {
    v = v * 10 + (L->s[L->i] - '0');
    L->i++;
  }
  return neg ? -v : v;
}

static i64 parse_atom(Lex *L, int *ok) {
  skip_ws(L);
  if (eat(L, '(')) {
    i64 v = parse_expr(L, ok);
    if (!*ok || !eat(L, ')')) {
      *ok = 0;
      return 0;
    }
    return v;
  }
  if (eat(L, '+'))
    return parse_atom(L, ok);
  if (eat(L, '-'))
    return -parse_atom(L, ok);
  return parse_number(L, ok);
}

static i64 ipow(i64 a, i64 b) {
  if (b < 0)
    return 0;
  i64 r = 1;
  while (b) {
    if (b & 1)
      r *= a;
    a *= a;
    b >>= 1;
  }
  return r;
}
static i64 parse_pow(Lex *L, int *ok) {
  i64 v = parse_atom(L, ok);
  if (!*ok)
    return 0;
  while (eat(L, '^')) {
    i64 rhs = parse_pow(L, ok);
    if (!*ok)
      return 0;
    v = ipow(v, rhs);
  }
  return v;
}

static i64 parse_term(Lex *L, int *ok) {
  i64 v = parse_pow(L, ok);
  if (!*ok)
    return 0;
  for (;;) {
    if (eat(L, '*')) {
      i64 r = parse_pow(L, ok);
      if (!*ok)
        return 0;
      v *= r;
    } else if (eat(L, '/')) {
      i64 r = parse_pow(L, ok);
      if (!*ok || r == 0) {
        *ok = 0;
        return 0;
      }
      v /= r;
    } else if (eat(L, '%')) {
      i64 r = parse_pow(L, ok);
      if (!*ok || r == 0) {
        *ok = 0;
        return 0;
      }
      v %= r;
    } else
      break;
  }
  return v;
}

static i64 parse_expr(Lex *L, int *ok) {
  i64 v = parse_term(L, ok);
  if (!*ok)
    return 0;
  for (;;) {
    if (eat(L, '+')) {
      i64 r = parse_term(L, ok);
      if (!*ok)
        return 0;
      v += r;
    } else if (eat(L, '-')) {
      i64 r = parse_term(L, ok);
      if (!*ok)
        return 0;
      v -= r;
    } else
      break;
  }
  return v;
}

static void trim(char *s) {
  int a = 0;
  while (is_space((unsigned char)s[a]) || s[a] == ' ')
    a++;
  int b = a;
  while (s[b])
    b++;
  while (b > a && (is_space((unsigned char)s[b - 1]) || s[b - 1] == ' '))
    b--;
  int n = b - a;
  for (int i = 0; i < n; i++)
    s[i] = s[a + i];
  s[n] = '\0';
}

int main(void) {
  char line[1024];

  char copyright_shit[] = "bc 0.1.0\nCopyright 2025 BSD 3-Clause License\n";
  printf("%s", copyright_shit);

  for (;;) {
    printf("> ");
    fflush(stdout);
    if (!read_line(line, sizeof line)) {
      puts("");
      break;
    }
    trim(line);
    if (!line[0])
      continue;

    int q = (to_lower(line[0]) == 'q' && to_lower(line[1]) == 'u' &&
             to_lower(line[2]) == 'i' && to_lower(line[3]) == 't' && !line[4]);
    int e = (to_lower(line[0]) == 'e' && to_lower(line[1]) == 'x' &&
             to_lower(line[2]) == 'i' && to_lower(line[3]) == 't' && !line[4]);
    if (q || e)
      break;

    Lex L = {line, 0};
    int ok = 1;
    i64 v = parse_expr(&L, &ok);
    skip_ws(&L);
    if (!ok || line[L.i] != '\0') {
      puts("error");
      continue;
    }
    printf("%lld\n", (long long)v);
  }
  return 0;
}
