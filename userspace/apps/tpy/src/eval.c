// eval.c — MiniPy tree-walking evaluator (MIT)
#include <stdio.h>
#include <string.h>
#include <setjmp.h>
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


static Value V_INT(long long x){ Value v={VT_INT}; v.i=x; return v; }
static Value V_STR(const char*s){ Value v={VT_STR}; v.s=s; return v; }
static Value V_NONE(void){ Value v={VT_NONE}; return v; }

static int is_truthy(Value v){
    switch(v.type){
        case VT_INT: return v.i != 0;
        case VT_STR: return v.s && v.s[0] != 0;
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
        else                       printf("None");
        if (i+1<argc) printf(" ");
    }
    printf("\n");
    return V_NONE();
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

static Value binop_apply(const char *op, Value a, Value b){
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
    // string equality only
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

    // builtin print
    if (strcmp(callee->sval, "print")==0) {
        return builtin_print(call->args.count, (const Expr *const*)call->args.items, env, rc);
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
