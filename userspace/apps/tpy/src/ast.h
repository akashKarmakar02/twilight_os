// ast.h — shared AST & parser/eval interfaces (MIT)
// Include in parser.c, eval.c, main.c

#ifndef MINIPY_AST_H
#define MINIPY_AST_H

#include "lexer.h"

/* ---------- AST ---------- */
#define MAX_STMTS   2048
#define MAX_EXPRS   4096
#define MAX_PARAMS  16
#define MAX_ARGS    16
#define MAX_IF_ARMS 16

typedef enum {
    NK_NUMBER, NK_STRING, NK_IDENT,
    NK_BINOP,        // a (op) b, op in sval
    NK_UNOP,         // op a, op in sval
    NK_CALL,         // a(args)
    NK_PAREN,        // (a)
    NK_LIST,         // [expr, ...]
    NK_SUBSCRIPT     // expr[expr]
} ExprKind;

typedef struct Expr Expr;

typedef struct {
    Expr *items[MAX_ARGS];
    int count;
} ExprList;

typedef struct {
    char names[MAX_PARAMS][64];
    int count;
} ParamList;

struct Expr {
    ExprKind kind;
    Token    tok;          // first token (debug)
    Expr    *a, *b;        // BINOP: a op b; PAREN: a; CALL: a=callee
    ExprList args;         // CALL
    long long  ival;       // NUMBER
    char       sval[64];   // STRING/IDENT/op
};


typedef enum {
    SK_EXPR,
    SK_RETURN,
    SK_FUNCDEF,
    SK_ASSIGN,           // name = expr
    SK_FOR,              // for var in expr: stmt
    SK_IF,               // if expr: stmt [elif expr: stmt] [else: stmt]
    SK_WHILE,            // while expr: stmt
    SK_BREAK,
    SK_CONTINUE,
    SK_PASS,
    SK_PRINT,            // print expr, ...
    SK_IMPORT,           // import expr
    SK_DEF,              // def name(params): stmt
    SK_END,              // end of file
} StmtKind;

typedef struct Stmt Stmt;
struct Stmt {
    StmtKind kind;
    Token    tok;
    Expr    *expr;         // EXPR/RETURN/ASSIGN uses this as RHS
    char     lhs[64];      // for SK_ASSIGN (identifier name), for SK_FOR (loop variable)

    // funcdef payload:
    char     fname[64];
    ParamList params;
    Stmt    *body;

    int n_arms;                         // how many if/elif arms are present
    struct Expr *conds[MAX_IF_ARMS];    // cond for each arm
    struct Stmt *bodies[MAX_IF_ARMS];   // body for each arm
    struct Stmt *else_body;             // optional else body (NULL if none)
};

typedef struct {
    Stmt *items[MAX_STMTS];
    int count;
} StmtList;

typedef struct {
    StmtList body;
} Module;

/* ---------- Parser API ---------- */
typedef struct {
    Module mod;
    int ok;  // 1 success
} ParseResult;

ParseResult parse_module(Lexer *lx);
void ast_dump(const Module *m);

/* ---------- Evaluator API ---------- */
typedef enum { VT_NONE=0, VT_INT, VT_STR, VT_LIST } ValueType;

typedef struct Value Value;

typedef struct {
    Value *items;
    int count;
    int capacity;
} ValueList;

struct Value {
    ValueType type;
    long long i;
    const char *s;   // non-owning pointer into string pool or literal
    ValueList list;  // for VT_LIST
};

// Execute module; returns 0 on success. Built-ins: print(...)
int eval_module(const Module *m);

#endif // MINIPY_AST_H
