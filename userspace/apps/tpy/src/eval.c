// eval.c — MiniPy tree-walking evaluator (MIT)
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <setjmp.h>
#include <ctype.h>
#include "ast.h"

/* ---------- Env: symbols & functions ---------- */
#define MAX_BINDINGS  256
#define MAX_FUNCS     256

typedef struct { char name[64]; Value val; } Binding;

typedef struct Env Env;
struct Env {
    Binding slots[MAX_BINDINGS];
    int count;
    Env *parent;
};

typedef struct {
    char name[64];
    const Stmt *def;   // SK_FUNCDEF stmt
} Func;

static Func FUNCS[MAX_FUNCS];
static int  N_FUNCS = 0;

/* ---------- Return unwinding ---------- */
typedef struct {
    jmp_buf jb;
    Value   ret;
    int     active;
} RetCtx;

static Env TOP_ENV;
static int TOP_INIT = 0;


static Value V_INT(long long x){ Value v={0}; v.type=VT_INT; v.i=x; v.list.items=NULL; v.list.count=0; v.list.capacity=0; return v; }
static Value V_STR(const char*s){ Value v={0}; v.type=VT_STR; v.s=s; v.list.items=NULL; v.list.count=0; v.list.capacity=0; return v; }
static Value V_NONE(void){ Value v={0}; v.type=VT_NONE; v.list.items=NULL; v.list.count=0; v.list.capacity=0; return v; }

static Value V_LIST(void) {
    Value v = {0};
    v.type = VT_LIST;
    v.list.items = NULL;
    v.list.count = 0;
    v.list.capacity = 0;
    return v;
}

/* String pool for dynamically allocated strings */
#define MAX_STR_POOL 256
static char *STR_POOL[MAX_STR_POOL];
static int STR_POOL_COUNT = 0;

static const char *str_pool_alloc(const char *src) {
    if (STR_POOL_COUNT >= MAX_STR_POOL) return NULL;
    size_t len = strlen(src);
    char *s = malloc(len + 1);
    if (!s) return NULL;
    strcpy(s, src);
    STR_POOL[STR_POOL_COUNT++] = s;
    return s;
}

/* ---------- Expression eval ---------- */
static long long ipow(long long base, long long exp) {
    if (exp < 0) return 0;
    long long result = 1;
    while (exp) {
        if (exp & 1) result *= base;
        base *= base;
        exp >>= 1;
    }
    return result;
}

static Value list_copy(const Value *src) {
    Value out = V_LIST();
    if (src->type != VT_LIST) return out;
    if (src->list.count <= 0) return out;
    out.list.capacity = src->list.count;
    out.list.items = malloc(sizeof(Value) * out.list.capacity);
    if (!out.list.items) return V_NONE();
    for (int i=0;i<src->list.count;i++) out.list.items[i] = src->list.items[i];
    out.list.count = src->list.count;
    return out;
}

static Value list_with_cap(int cap) {
    Value v = V_LIST();
    if (cap < 1) cap = 4;
    v.list.capacity = cap;
    v.list.items = malloc(sizeof(Value) * v.list.capacity);
    if (!v.list.items) return V_NONE();
    v.list.count = 0;
    return v;
}

static int list_push_inplace(Value *lst, Value x) {
    if (lst->type != VT_LIST) return 0;
    if (lst->list.count >= lst->list.capacity) {
        int nc = lst->list.capacity ? lst->list.capacity * 2 : 4;
        Value *ni = realloc(lst->list.items, sizeof(Value) * nc);
        if (!ni) return 0;
        lst->list.items = ni;
        lst->list.capacity = nc;
    }
    lst->list.items[lst->list.count++] = x;
    return 1;
}

static int clampi(int x, int lo, int hi) {
    if (x < lo) return lo;
    if (x > hi) return hi;
    return x;
}

static int is_truthy(Value v){
    switch(v.type){
        case VT_INT: return v.i != 0;
        case VT_STR: return v.s && v.s[0] != 0;
        case VT_LIST: return v.list.count > 0;
        default: return 0;
    }
}

/* ---------- Env helpers ---------- */
static void env_init(Env *e, Env *parent){ e->count=0; e->parent=parent; }
static int env_set(Env *e, const char *name, Value v){
    for (int i=0;i<e->count;i++){
        if (strcmp(e->slots[i].name, name)==0){ e->slots[i].val=v; return 1; }
    }
    if (e->count >= MAX_BINDINGS) return 0;
    strncpy(e->slots[e->count].name, name, sizeof e->slots[e->count].name);
    e->slots[e->count].val=v;
    e->count++;
    return 1;
}

static int env_update(Env *e, const char *name, Value v){
    for (Env *cur=e; cur; cur=cur->parent){
        for (int i=0;i<cur->count;i++){
            if (strcmp(cur->slots[i].name, name)==0){ cur->slots[i].val=v; return 1; }
        }
    }
    return 0;
}

static Value env_get(Env *e, const char *name, int *ok){
    for (Env *cur=e; cur; cur=cur->parent){
        for (int i=0;i<cur->count;i++){
            if (strcmp(cur->slots[i].name, name)==0){ if(ok)*ok=1; return cur->slots[i].val; }
        }
    }
    if (ok) *ok = 0;
    return V_NONE();
}

/* ---------- Func registry ---------- */
static const Stmt *func_lookup(const char *name){
    for(int i=0;i<N_FUNCS;i++) if(strcmp(FUNCS[i].name, name)==0) return FUNCS[i].def;
    return NULL;
}
static int func_register(const Stmt *def){
    if(N_FUNCS>=MAX_FUNCS) return 0;
    strncpy(FUNCS[N_FUNCS].name, def->fname, sizeof FUNCS[N_FUNCS].name);
    FUNCS[N_FUNCS].def = def;
    N_FUNCS++;
    return 1;
}

/* ---------- Forward decls ---------- */
static Value eval_expr(const Expr *e, Env *env, RetCtx *rc);
static void  eval_stmt(const Stmt *s, Env *env, RetCtx *rc);

/* ---------- Builtin: print(...) ---------- */
static Value builtin_print(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    for(int i=0;i<argc;i++){
        Value v = eval_expr(argv[i], env, rc);
        if (v.type == VT_INT)      printf("%lld", v.i);
        else if (v.type == VT_STR) printf("%s", v.s ? v.s : "");
        else if (v.type == VT_LIST) {
            printf("[");
            for (int j = 0; j < v.list.count; j++) {
                Value item = v.list.items[j];
                if (item.type == VT_INT) printf("%lld", item.i);
                else if (item.type == VT_STR) printf("%s", item.s ? item.s : "");
                else if (item.type == VT_LIST) printf("[...]");
                else printf("None");
                if (j + 1 < v.list.count) printf(", ");
            }
            printf("]");
        }
        else                       printf("None");
        if (i+1<argc) printf(" ");
    }
    printf("\n");
    return V_NONE();
}

/* ---------- Builtin: input([prompt]) ---------- */
static Value builtin_input(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc > 0) {
        Value prompt = eval_expr(argv[0], env, rc);
        if (prompt.type == VT_STR && prompt.s) {
            printf("%s", prompt.s);
            fflush(stdout);
        } else if (prompt.type == VT_INT) {
            printf("%lld", prompt.i);
            fflush(stdout);
        }
    }

    char line[1024];
    if (!fgets(line, sizeof(line), stdin)) {
        return V_STR("");
    }

    // Remove trailing newline
    size_t len = strlen(line);
    while (len > 0 && (line[len-1] == '\n' || line[len-1] == '\r')) {
        line[--len] = '\0';
    }

    const char *s = str_pool_alloc(line);
    return V_STR(s ? s : "");
}

/* ---------- Builtin: len(x) ---------- */
static Value builtin_len(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_NONE();
    Value v = eval_expr(argv[0], env, rc);
    if (v.type == VT_STR) {
        return V_INT((long long)strlen(v.s ? v.s : ""));
    } else if (v.type == VT_LIST) {
        return V_INT((long long)v.list.count);
    }
    return V_NONE();
}

/* ---------- Builtin: str(x) ---------- */
static Value builtin_str(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_NONE();
    Value v = eval_expr(argv[0], env, rc);
    char buf[128];
    if (v.type == VT_INT) {
        snprintf(buf, sizeof(buf), "%lld", v.i);
        return V_STR(str_pool_alloc(buf));
    } else if (v.type == VT_STR) {
        return v; // Already a string
    } else {
        return V_STR("None");
    }
}

/* ---------- Builtin: int(x) ---------- */
static Value builtin_int(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_NONE();
    Value v = eval_expr(argv[0], env, rc);
    if (v.type == VT_INT) {
        return v; // Already an int
    } else if (v.type == VT_STR && v.s) {
        // Parse string to int
        char *end;
        long long val = strtoll(v.s, &end, 10);
        if (*end == '\0' || isspace((unsigned char)*end)) {
            return V_INT(val);
        }
    }
    return V_INT(0); // Default to 0 on error
}

/* ---------- Builtin: abs(x) ---------- */
static Value builtin_abs(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_NONE();
    Value v = eval_expr(argv[0], env, rc);
    if (v.type == VT_INT) {
        return V_INT(v.i < 0 ? -v.i : v.i);
    }
    return V_NONE();
}

/* ---------- Builtin: max(...) ---------- */
static Value builtin_max(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc == 0) return V_NONE();
    Value max_val = eval_expr(argv[0], env, rc);
    if (max_val.type != VT_INT) return V_NONE();

    for (int i = 1; i < argc; i++) {
        Value v = eval_expr(argv[i], env, rc);
        if (v.type == VT_INT && v.i > max_val.i) {
            max_val = v;
        }
    }
    return max_val;
}

/* ---------- Builtin: min(...) ---------- */
static Value builtin_min(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc == 0) return V_NONE();
    Value min_val = eval_expr(argv[0], env, rc);
    if (min_val.type != VT_INT) return V_NONE();

    for (int i = 1; i < argc; i++) {
        Value v = eval_expr(argv[i], env, rc);
        if (v.type == VT_INT && v.i < min_val.i) {
            min_val = v;
        }
    }
    return min_val;
}

/* ---------- Builtin: range([start,] stop[, step]) ---------- */
static Value builtin_range(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    // For simplicity, range returns a string representation
    // In a full implementation, this would return an iterable
    long long start = 0, stop = 0, step = 1;

    if (argc == 1) {
        Value v = eval_expr(argv[0], env, rc);
        if (v.type == VT_INT) stop = v.i;
        else return V_NONE();
    } else if (argc == 2) {
        Value v1 = eval_expr(argv[0], env, rc);
        Value v2 = eval_expr(argv[1], env, rc);
        if (v1.type == VT_INT && v2.type == VT_INT) {
            start = v1.i;
            stop = v2.i;
        } else return V_NONE();
    } else if (argc == 3) {
        Value v1 = eval_expr(argv[0], env, rc);
        Value v2 = eval_expr(argv[1], env, rc);
        Value v3 = eval_expr(argv[2], env, rc);
        if (v1.type == VT_INT && v2.type == VT_INT && v3.type == VT_INT) {
            start = v1.i;
            stop = v2.i;
            step = v3.i;
        } else return V_NONE();
    } else {
        return V_NONE();
    }

    // Build range string representation
    char buf[512];
    int pos = 0;
    for (long long i = start; (step > 0 && i < stop) || (step < 0 && i > stop); i += step) {
        if (pos > 0) pos += snprintf(buf + pos, sizeof(buf) - pos, ", ");
        pos += snprintf(buf + pos, sizeof(buf) - pos, "%lld", i);
        if (pos >= (int)(sizeof(buf) - 20)) break; // Prevent overflow
    }
    return V_STR(str_pool_alloc(buf));
}

static Value builtin_type(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_STR("none");
    Value v = eval_expr(argv[0], env, rc);
    switch (v.type) {
        case VT_INT:  return V_STR("int");
        case VT_STR:  return V_STR("str");
        case VT_LIST: return V_STR("list");
        default:      return V_STR("none");
    }
}

/* ---------- Builtin: pow(a,b) (integers) ---------- */
static Value builtin_powf(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 2) return V_NONE();
    Value a = eval_expr(argv[0], env, rc);
    Value b = eval_expr(argv[1], env, rc);
    if (a.type != VT_INT || b.type != VT_INT) return V_NONE();
    return V_INT(ipow(a.i, b.i));
}

/* ---------- Builtin: sum(list or variadic ints) ---------- */
static Value builtin_sum(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    long long acc = 0;
    if (argc == 1) {
        Value v = eval_expr(argv[0], env, rc);
        if (v.type == VT_LIST) {
            for (int i=0;i<v.list.count;i++)
                if (v.list.items[i].type == VT_INT) acc += v.list.items[i].i;
            return V_INT(acc);
        }
    }
    for (int i=0;i<argc;i++){
        Value v = eval_expr(argv[i], env, rc);
        if (v.type == VT_INT) acc += v.i;
    }
    return V_INT(acc);
}

/* ---------- Builtin: join(sep, list_of_str) -> str ---------- */
static Value builtin_join(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 2) return V_NONE();
    Value sepv = eval_expr(argv[0], env, rc);
    Value lv   = eval_expr(argv[1], env, rc);
    const char *sep = (sepv.type==VT_STR && sepv.s) ? sepv.s : "";
    if (lv.type != VT_LIST) return V_NONE();

    /* first pass: length */
    size_t total = 1; // NUL
    int n = lv.list.count;
    for (int i=0;i<n;i++){
        Value it = lv.list.items[i];
        const char *s = (it.type==VT_STR && it.s) ? it.s : "";
        total += strlen(s);
        if (i+1<n) total += strlen(sep);
    }
    char *buf = malloc(total);
    if (!buf) return V_NONE();
    buf[0]=0;

    /* build */
    for (int i=0;i<n;i++){
        Value it = lv.list.items[i];
        const char *s = (it.type==VT_STR && it.s) ? it.s : "";
        strcat(buf, s);
        if (i+1<n) strcat(buf, sep);
    }
    const char *pooled = str_pool_alloc(buf);
    free(buf);
    return V_STR(pooled ? pooled : "");
}

/* ---------- Builtin: split(str, sep) -> list[str] ---------- */
static Value builtin_split(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc < 1 || argc > 2) return V_NONE();
    Value sv = eval_expr(argv[0], env, rc);
    Value sepv = (argc==2) ? eval_expr(argv[1], env, rc) : V_STR(" ");
    if (sv.type != VT_STR) return V_NONE();
    const char *s = sv.s ? sv.s : "";
    const char *sep = (sepv.type==VT_STR && sepv.s && sepv.s[0]) ? sepv.s : " ";

    Value out = list_with_cap(8);
    if (out.type != VT_LIST) return V_NONE();

    size_t seplen = strlen(sep);
    const char *p = s;
    while (*p) {
        const char *q = strstr(p, sep);
        size_t len = q ? (size_t)(q - p) : strlen(p);
        char *tmp = malloc(len + 1);
        if (!tmp) return V_NONE();
        memcpy(tmp, p, len); tmp[len] = 0;
        const char *pooled = str_pool_alloc(tmp);
        free(tmp);
        if (!list_push_inplace(&out, V_STR(pooled ? pooled : ""))) return V_NONE();
        if (!q) break;
        p = q + seplen;
    }
    return out;
}

/* ---------- Builtin: substr(s, start, len) -> str ---------- */
static Value builtin_substr(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 3) return V_NONE();
    Value sv = eval_expr(argv[0], env, rc);
    Value st = eval_expr(argv[1], env, rc);
    Value ln = eval_expr(argv[2], env, rc);
    if (sv.type!=VT_STR || st.type!=VT_INT || ln.type!=VT_INT) return V_NONE();
    const char *s = sv.s ? sv.s : "";
    int L = (int)strlen(s);
    int a = clampi((int)st.i, 0, L);
    int n = clampi((int)ln.i, 0, L - a);
    char *buf = malloc((size_t)n + 1); if (!buf) return V_NONE();
    memcpy(buf, s + a, (size_t)n); buf[n]=0;
    const char *pooled = str_pool_alloc(buf);
    free(buf);
    return V_STR(pooled ? pooled : "");
}

/* ---------- Builtin: find(s, sub) -> index or -1 ---------- */
static Value builtin_find(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 2) return V_NONE();
    Value sv = eval_expr(argv[0], env, rc);
    Value tv = eval_expr(argv[1], env, rc);
    if (sv.type!=VT_STR || tv.type!=VT_STR) return V_INT(-1);
    const char *s = sv.s ? sv.s : "";
    const char *t = tv.s ? tv.s : "";
    const char *p = strstr(s, t);
    return V_INT(p ? (long long)(p - s) : -1);
}

/* ---------- Builtin: startswith/endswith(s, prefix/suffix) -> int 0/1 ---------- */
static Value builtin_startswith(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 2) return V_NONE();
    Value sv = eval_expr(argv[0], env, rc);
    Value pv = eval_expr(argv[1], env, rc);
    if (sv.type!=VT_STR || pv.type!=VT_STR) return V_INT(0);
    const char *s = sv.s ? sv.s : "", *p = pv.s ? pv.s : "";
    size_t lp = strlen(p);
    return V_INT(strncmp(s, p, lp) == 0);
}
static Value builtin_endswith(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 2) return V_NONE();
    Value sv = eval_expr(argv[0], env, rc);
    Value pv = eval_expr(argv[1], env, rc);
    if (sv.type!=VT_STR || pv.type!=VT_STR) return V_INT(0);
    const char *s = sv.s ? sv.s : "", *p = pv.s ? pv.s : "";
    size_t ls = strlen(s), lp = strlen(p);
    if (lp > ls) return V_INT(0);
    return V_INT(strcmp(s + (ls - lp), p) == 0);
}

/* ---------- Builtin: tolower(s)/toupper(s) ---------- */
static Value builtin_tolower(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_NONE();
    Value sv = eval_expr(argv[0], env, rc);
    if (sv.type != VT_STR) return V_NONE();
    const char *s = sv.s ? sv.s : "";
    size_t n = strlen(s);
    char *buf = malloc(n+1); if (!buf) return V_NONE();
    for (size_t i=0;i<n;i++) buf[i] = (char)tolower((unsigned char)s[i]);
    buf[n]=0;
    const char *pooled = str_pool_alloc(buf);
    free(buf);
    return V_STR(pooled ? pooled : "");
}
static Value builtin_toupper(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_NONE();
    Value sv = eval_expr(argv[0], env, rc);
    if (sv.type != VT_STR) return V_NONE();
    const char *s = sv.s ? sv.s : "";
    size_t n = strlen(s);
    char *buf = malloc(n+1); if (!buf) return V_NONE();
    for (size_t i=0;i<n;i++) buf[i] = (char)toupper((unsigned char)s[i]);
    buf[n]=0;
    const char *pooled = str_pool_alloc(buf);
    free(buf);
    return V_STR(pooled ? pooled : "");
}

/* ---------- Builtin: ord(s)/chr(i) ---------- */
static Value builtin_ord(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_NONE();
    Value sv = eval_expr(argv[0], env, rc);
    if (sv.type != VT_STR || !sv.s || !sv.s[0]) return V_INT(0);
    return V_INT((unsigned char)sv.s[0]);
}
static Value builtin_chr(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 1) return V_NONE();
    Value iv = eval_expr(argv[0], env, rc);
    if (iv.type != VT_INT) return V_NONE();
    char buf[2]; buf[0] = (char)(unsigned char)(iv.i & 0xFF); buf[1]=0;
    return V_STR(str_pool_alloc(buf));
}

/* ---------- Builtin: slice(list, start, end) -> list (non-mutating) ---------- */
static Value builtin_slice(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 3) return V_NONE();
    Value lv = eval_expr(argv[0], env, rc);
    Value sv = eval_expr(argv[1], env, rc);
    Value ev = eval_expr(argv[2], env, rc);
    if (lv.type != VT_LIST || sv.type != VT_INT || ev.type != VT_INT) return V_NONE();
    int n = lv.list.count;
    int a = (int)sv.i, b = (int)ev.i;
    if (a < 0) a += n; if (b < 0) b += n;
    a = clampi(a, 0, n); b = clampi(b, 0, n);
    if (b < a) b = a;

    Value out = list_with_cap(b - a);
    if (out.type != VT_LIST) return V_NONE();
    for (int i=a;i<b;i++) list_push_inplace(&out, lv.list.items[i]);
    return out;
}

/* ---------- Builtin: push(list, x) -> new list (non-mutating) ---------- */
static Value builtin_push(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 2) return V_NONE();
    Value lv = eval_expr(argv[0], env, rc);
    Value xv = eval_expr(argv[1], env, rc);
    if (lv.type != VT_LIST) return V_NONE();
    Value out = list_copy(&lv);
    if (out.type != VT_LIST) return V_NONE();
    list_push_inplace(&out, xv);
    return out;
}

/* ---------- Builtin: concat(list1, list2) -> new list ---------- */
static Value builtin_concat(int argc, const Expr *const* argv, Env *env, RetCtx *rc){
    if (argc != 2) return V_NONE();
    Value a = eval_expr(argv[0], env, rc);
    Value b = eval_expr(argv[1], env, rc);
    if (a.type != VT_LIST || b.type != VT_LIST) return V_NONE();
    Value out = list_with_cap(a.list.count + b.list.count);
    if (out.type != VT_LIST) return V_NONE();
    for (int i=0;i<a.list.count;i++) list_push_inplace(&out, a.list.items[i]);
    for (int i=0;i<b.list.count;i++) list_push_inplace(&out, b.list.items[i]);
    return out;
}


static Value binop_apply(const char *op, Value a, Value b){
    // String concatenation and operations
    if (strcmp(op, "+") == 0) {
        // String concatenation
        if (a.type == VT_STR && b.type == VT_STR) {
            const char *s1 = a.s ? a.s : "";
            const char *s2 = b.s ? b.s : "";
            size_t len1 = strlen(s1);
            size_t len2 = strlen(s2);
            char *result = malloc(len1 + len2 + 1);
            if (!result) return V_NONE();
            strcpy(result, s1);
            strcat(result, s2);
            const char *pooled = str_pool_alloc(result);
            free(result);
            return V_STR(pooled ? pooled : "");
        }
        // Int + String: convert int to string and concatenate
        if (a.type == VT_INT && b.type == VT_STR) {
            char buf[128];
            snprintf(buf, sizeof(buf), "%lld", a.i);
            const char *s2 = b.s ? b.s : "";
            size_t len1 = strlen(buf);
            size_t len2 = strlen(s2);
            char *result = malloc(len1 + len2 + 1);
            if (!result) return V_NONE();
            strcpy(result, buf);
            strcat(result, s2);
            const char *pooled = str_pool_alloc(result);
            free(result);
            return V_STR(pooled ? pooled : "");
        }
        // String + Int: concatenate string with converted int
        if (a.type == VT_STR && b.type == VT_INT) {
            const char *s1 = a.s ? a.s : "";
            char buf[128];
            snprintf(buf, sizeof(buf), "%lld", b.i);
            size_t len1 = strlen(s1);
            size_t len2 = strlen(buf);
            char *result = malloc(len1 + len2 + 1);
            if (!result) return V_NONE();
            strcpy(result, s1);
            strcat(result, buf);
            const char *pooled = str_pool_alloc(result);
            free(result);
            return V_STR(pooled ? pooled : "");
        }
    }

    // String multiplication: "hello" * 3
    if (strcmp(op, "*") == 0) {
        if (a.type == VT_STR && b.type == VT_INT && b.i > 0) {
            const char *s = a.s ? a.s : "";
            size_t len = strlen(s);
            char *result = malloc(len * (size_t)b.i + 1);
            if (!result) return V_NONE();
            result[0] = '\0';
            for (long long i = 0; i < b.i; i++) {
                strcat(result, s);
            }
            const char *pooled = str_pool_alloc(result);
            free(result);
            return V_STR(pooled ? pooled : "");
        }
        if (a.type == VT_INT && b.type == VT_STR && a.i > 0) {
            const char *s = b.s ? b.s : "";
            size_t len = strlen(s);
            char *result = malloc(len * (size_t)a.i + 1);
            if (!result) return V_NONE();
            result[0] = '\0';
            for (long long i = 0; i < a.i; i++) {
                strcat(result, s);
            }
            const char *pooled = str_pool_alloc(result);
            free(result);
            return V_STR(pooled ? pooled : "");
        }
    }

    // Integer operations
    if (a.type==VT_INT && b.type==VT_INT){
        if      (strcmp(op, "+")==0) return V_INT(a.i + b.i);
        else if (strcmp(op, "-")==0) return V_INT(a.i - b.i);
        else if (strcmp(op, "*")==0) return V_INT(a.i * b.i);
        else if (strcmp(op, "/")==0) return V_INT(b.i==0 ? 0 : a.i / b.i);
        else if (strcmp(op, "//")==0) return V_INT(b.i==0 ? 0 : (long long)(a.i / b.i));
        else if (strcmp(op, "%")==0) return V_INT(b.i==0 ? 0 : a.i % b.i);
        else if (strcmp(op, "**")==0) return V_INT(ipow(a.i, b.i));
        else if (strcmp(op, "==")==0) return V_INT(a.i == b.i);
        else if (strcmp(op, "!=")==0) return V_INT(a.i != b.i);
        else if (strcmp(op, "<")==0)  return V_INT(a.i <  b.i);
        else if (strcmp(op, "<=")==0) return V_INT(a.i <= b.i);
        else if (strcmp(op, ">")==0)  return V_INT(a.i >  b.i);
        else if (strcmp(op, ">=")==0) return V_INT(a.i >= b.i);
        else if (strcmp(op, "&")==0) return V_INT(a.i & b.i);
        else if (strcmp(op, "|")==0) return V_INT(a.i | b.i);
        else if (strcmp(op, "^")==0) return V_INT(a.i ^ b.i);
        else if (strcmp(op, "&&")==0) return V_INT(is_truthy(a) && is_truthy(b));
        else if (strcmp(op, "||")==0) return V_INT(is_truthy(a) || is_truthy(b));
    }

    // String equality
    if (a.type==VT_STR && b.type==VT_STR) {
        if (strcmp(op,"==")==0)
            return V_INT( (a.s && b.s) ? strcmp(a.s,b.s)==0 : a.s==b.s );
        else if (strcmp(op,"!=")==0)
            return V_INT( (a.s && b.s) ? strcmp(a.s,b.s)!=0 : a.s!=b.s );
    }

    // unsupported types → None
    return V_NONE();
}

static Value unop_apply(const char *op, Value a) {
    if (a.type==VT_INT) {
        if (strcmp(op, "~")==0) return V_INT(~a.i);
        else if (strcmp(op, "!")==0) return V_INT(!is_truthy(a));
    }
    return V_NONE();
}

static Value eval_call(const Expr *call, Env *env, RetCtx *rc){
    // callee can be IDENT only (tiny subset)
    const Expr *callee = call->a;
    if (!callee || callee->kind != NK_IDENT) return V_NONE();

    // builtin functions
    const char *fn_name = callee->sval;
    if (strcmp(fn_name, "print") == 0) {
        return builtin_print(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "input") == 0) {
        return builtin_input(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "len") == 0) {
        return builtin_len(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "str") == 0) {
        return builtin_str(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "int") == 0) {
        return builtin_int(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "abs") == 0) {
        return builtin_abs(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "max") == 0) {
        return builtin_max(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "min") == 0) {
        return builtin_min(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "range") == 0) {
        return builtin_range(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "range") == 0) {
        return builtin_range(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "type") == 0) {
        return builtin_type(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "pow") == 0) {
        return builtin_powf(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "sum") == 0) {
        return builtin_sum(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "join") == 0) {
        return builtin_join(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "split") == 0) {
        return builtin_split(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "substr") == 0) {
        return builtin_substr(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "find") == 0) {
        return builtin_find(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "startswith") == 0) {
        return builtin_startswith(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "endswith") == 0) {
        return builtin_endswith(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "tolower") == 0) {
        return builtin_tolower(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "toupper") == 0) {
        return builtin_toupper(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "ord") == 0) {
        return builtin_ord(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "chr") == 0) {
        return builtin_chr(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "slice") == 0) {
        return builtin_slice(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "push") == 0) {
        return builtin_push(call->args.count, (const Expr *const*)call->args.items, env, rc);
    } else if (strcmp(fn_name, "concat") == 0) {
      return builtin_concat(call->args.count,
                            (const Expr *const *)call->args.items, env, rc);
    }

    // user function
    const Stmt *fn = func_lookup(callee->sval);
    if (!fn || fn->kind != SK_FUNCDEF) return V_NONE();

    // bind args -> params in a new child env
    Env child; env_init(&child, env);
    int nparams = fn->params.count;
    int nargs   = call->args.count;
    if (nargs != nparams) {
        fprintf(stderr, "TypeError: %s expects %d args, got %d\n", fn->fname, nparams, nargs);
        return V_NONE();
    }
    for (int i=0;i<nparams;i++){
        Value v = eval_expr(call->args.items[i], env, rc);
        env_set(&child, fn->params.names[i], v);
    }

    // run body with return-capture
    if (!rc->active) { rc->active = 1; } // mark usable
    int j = setjmp(rc->jb);
    if (j == 0) {
        eval_stmt(fn->body, &child, rc);
        // no explicit return → None
        return V_NONE();
    } else {
        // longjmp landed here with rc->ret
        return rc->ret;
    }
}

static Value eval_expr(const Expr *e, Env *env, RetCtx *rc){
    if (!e) return V_NONE();
    switch (e->kind){
        case NK_NUMBER: return V_INT(e->ival);
        case NK_STRING: return V_STR(e->sval);
        case NK_IDENT: {
            int ok=0; Value v = env_get(env, e->sval, &ok);
            if (!ok) return V_NONE();
            return v;
        }
        case NK_PAREN: return eval_expr(e->a, env, rc);
        case NK_CALL:  return eval_call(e, env, rc);
        case NK_LIST: {
            Value list = V_LIST();
            list.list.capacity = e->args.count > 0 ? e->args.count : 4;
            list.list.items = malloc(sizeof(Value) * list.list.capacity);
            if (!list.list.items) return V_NONE();

            for (int i = 0; i < e->args.count; i++) {
                Value item = eval_expr(e->args.items[i], env, rc);
                if (list.list.count >= list.list.capacity) {
                    list.list.capacity *= 2;
                    list.list.items = realloc(list.list.items, sizeof(Value) * list.list.capacity);
                    if (!list.list.items) return V_NONE();
                }
                list.list.items[list.list.count++] = item;
            }
            return list;
        }
        case NK_SUBSCRIPT: {
            Value container = eval_expr(e->a, env, rc);
            Value index_val = eval_expr(e->b, env, rc);

            if (container.type == VT_LIST && index_val.type == VT_INT) {
                int idx = (int)index_val.i;
                if (idx >= 0 && idx < container.list.count) {
                    return container.list.items[idx];
                }
            }
            return V_NONE();
        }
        case NK_UNOP: {
            Value a = eval_expr(e->a, env, rc);
            return unop_apply(e->sval, a);
        }
        case NK_BINOP: {
            Value a = eval_expr(e->a, env, rc);
            Value b = eval_expr(e->b, env, rc);
            return binop_apply(e->sval, a, b);
        }
        default: return V_NONE();
    }
}

/* ---------- Statement eval ---------- */
static void eval_stmt(const Stmt *s, Env *env, RetCtx *rc){
    if (!s) return;
    switch (s->kind){
        case SK_EXPR:
            (void)eval_expr(s->expr, env, rc);
            break;
        case SK_RETURN: {
            Value v = eval_expr(s->expr, env, rc);
            rc->ret = v;
            longjmp(rc->jb, 1);
            break; // unreachable
        }
        case SK_FUNCDEF:
            // register for later calls
            if (!func_register(s)) {
                fprintf(stderr, "error: func registry full\n");
            }
            break;
        case SK_ASSIGN: {
            Value v = eval_expr(s->expr, env, rc);
            // assign to current env (Python semantics: local unless declared global)
            env_set(env, s->lhs, v);
            break;
        }
        case SK_FOR: {
            Value iterable = eval_expr(s->expr, env, rc);
            if (iterable.type == VT_LIST) {
                for (int i = 0; i < iterable.list.count; i++) {
                    Value item = iterable.list.items[i];
                    env_set(env, s->lhs, item);
                    eval_stmt(s->body, env, rc);
                }
            }
            break;
        }
        case SK_IF: {
            for (int i = 0; i < s->n_arms; i++) {
                Value cv = eval_expr(s->conds[i], env, rc);
                if (is_truthy(cv)) {
                    eval_stmt(s->bodies[i], env, rc);
                    return;
                }
            }
            if (s->else_body) {
                eval_stmt(s->else_body, env, rc);
            }
            return;
        }
        default:
            break;
    }
}

/* ---------- Module eval ---------- */
int eval_module(const Module *m){
    if (!TOP_INIT) { env_init(&TOP_ENV, NULL); TOP_INIT = 1; }

    RetCtx rc = {0};

    // collect defs (persist across runs too)
    for (int i=0;i<m->body.count;i++) {
        const Stmt *s = m->body.items[i];
        if (s && s->kind == SK_FUNCDEF) {
            func_register(s); // fine if duplicate; could be replaced or ignored
        }
    }
    // run top-level statements in TOP_ENV
    for (int i=0;i<m->body.count;i++) {
        eval_stmt(m->body.items[i], &TOP_ENV, &rc);
    }
    return 0;
}
