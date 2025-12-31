// parser.c - MiniPy recursive-descent parser building an AST
// MIT License (c) 2025
//
// Depends only on lexer.h (from the previous step).
// No malloc: fixed-size arenas. Adjust MAX_* as you like.
//
// Grammar (subset):
//   module      := stmt* EOF
//   stmt        := "return" expr NEWLINE?
//                | "def" IDENT "(" paramlist? ")" ":" ( stmt | NEWLINE stmt )
//                | expr NEWLINE?
//   paramlist   := IDENT ("," IDENT)*
//   arglist     := expr ("," expr)*
//   expr        := comp
//   comp        := sum ( (==|<|>|<=|>=) sum )*
//   sum         := term ( ("+"|"-") term )*
//   term        := factor ( ("*"|"/") factor )*
//   factor      := primary
//   primary     := NUMBER | STRING | IDENT call? | "(" expr ")"
//   call        := "(" arglist? ")"
//
// NOTES:
// - def body: either on same line after ":" or exactly one stmt on the next line.
// - NEWLINE is optional after a simple stmt (we're permissive).

#include <stdio.h>
#include <string.h>
#include "lexer.h"
#include "ast.h"

/* ---------- Config ---------- */
#define MAX_STMTS   2048
#define MAX_EXPRS   4096
#define MAX_PARAMS  16
#define MAX_ARGS    16
#define MAX_FUNCS   256

/* ---------- Parser state ---------- */
typedef struct {
    Lexer *lx;
    Token  cur;
    int    had_error;
} Parser;


typedef enum {
    PREC_LOWEST=0,
    PREC_CMP,    // == != < > <= >=
    PREC_ADD,    // + -
    PREC_MUL,    // * / // %
    PREC_POW,    // **
    PREC_BIT,    // & | ^ ~
    PREC_LOGICAL,// && ||
    PREC_NOT,    // !
    PREC_PRIMARY // atoms/calls
} Precedence;

static Precedence precedence_of(TokenKind k);
static int is_binop(TokenKind k);
static int is_right_assoc(TokenKind k);

/* parse_binop_rhs is referenced from parse_primary, so declare it now */
static Expr *parse_binop_rhs(Parser *p, Precedence min_prec, Expr *lhs);

static Expr *parse_expr(Parser *p);
static Stmt *parse_stmt(Parser *p);
static Expr *parse_expr_continuation(Parser *p, Expr *lhs); // fwd
static Expr *parse_primary(Parser *p);
static Stmt *parse_if_stmt(Parser *p);

/* ---------- Arenas ---------- */
static Expr EXPR_ARENA[MAX_EXPRS];
static int  EXPR_TOP = 0;
static Stmt STMT_ARENA[MAX_STMTS];
static int  STMT_TOP = 0;

static Expr *new_expr(void) {
    if (EXPR_TOP >= MAX_EXPRS) return NULL;
    Expr *e = &EXPR_ARENA[EXPR_TOP++];
    memset(e, 0, sizeof(*e));
    return e;
}
static Stmt *new_stmt(void) {
    if (STMT_TOP >= MAX_STMTS) return NULL;
    Stmt *s = &STMT_ARENA[STMT_TOP++];
    memset(s, 0, sizeof(*s));
    return s;
}

/* ---------- Utilities ---------- */
static void parser_next(Parser *p) { p->cur = lexer_next(p->lx); }

static int is_kw(const Token *t, const char *kw) {
    return t->kind == TOK_KEYWORD && strcmp(t->text, kw) == 0;
}
static int accept(Parser *p, TokenKind k) {
    if (p->cur.kind == k) { parser_next(p); return 1; }
    return 0;
}
static int accept_kw(Parser *p, const char *kw) {
    if (is_kw(&p->cur, kw)) { parser_next(p); return 1; }
    return 0;
}
static void expect(Parser *p, TokenKind k, const char *msg) {
    if (!accept(p, k)) {
        fprintf(stderr, "Parse error: expected %s at line %d, got %s '%s'\n",
                msg, p->cur.line, tok_name(p->cur.kind), p->cur.text);
        p->had_error = 1;
    }
}
static void optional_newlines(Parser *p) {
    while (p->cur.kind == TOK_NEWLINE) parser_next(p);
}

/* ---------- Forward decls for expressions/statements ---------- */
static Expr *parse_expr(Parser *p);
static Stmt *parse_stmt(Parser *p);
static Expr *parse_expr_continuation(Parser *p, Expr *lhs); // fwd

/* ---------- primaries ---------- */
static Expr *parse_primary(Parser *p) {
    Expr *e = NULL;

    // unary operators
    if (p->cur.kind == TOK_NOT || p->cur.kind == TOK_BIT_NOT) {
        Token optok = p->cur;
        parser_next(p);
        Expr *operand = parse_primary(p);
        if (!operand) return NULL;
        
        Expr *unop = new_expr(); if (!unop) return operand;
        unop->kind = NK_UNOP;
        unop->tok = optok;
        strncpy(unop->sval, optok.text, sizeof unop->sval);
        unop->a = operand;
        return unop;
    }

    if (p->cur.kind == TOK_NUMBER) {
        e = new_expr(); if (!e) return NULL;
        e->kind = NK_NUMBER;
        e->tok  = p->cur;
        e->ival = p->cur.value;
        parser_next(p);
        return e;
    }
    if (p->cur.kind == TOK_STRING) {
        e = new_expr(); if (!e) return NULL;
        e->kind = NK_STRING;
        e->tok  = p->cur;
        strncpy(e->sval, p->cur.text, sizeof e->sval);
        parser_next(p);
        return e;
    }
    // List literal: [expr, ...]
    if (p->cur.kind == TOK_LBRACKET) {
        Token list_tok = p->cur;
        parser_next(p);
        e = new_expr(); if (!e) return NULL;
        e->kind = NK_LIST;
        e->tok = list_tok;
        e->args.count = 0;
        
        if (p->cur.kind != TOK_RBRACKET) {
            Expr *item = parse_expr(p);
            if (item && e->args.count < MAX_ARGS)
                e->args.items[e->args.count++] = item;
            
            while (accept(p, TOK_COMMA)) {
                if (p->cur.kind == TOK_RBRACKET) break; // trailing comma
                item = parse_expr(p);
                if (item && e->args.count < MAX_ARGS)
                    e->args.items[e->args.count++] = item;
            }
        }
        expect(p, TOK_RBRACKET, "']'");
        return e;
    }
    if (p->cur.kind == TOK_IDENT) {
        e = new_expr(); if (!e) return NULL;
        e->kind = NK_IDENT;
        e->tok  = p->cur;
        strncpy(e->sval, p->cur.text, sizeof e->sval);
        parser_next(p);

        // possible call: IDENT "(" args? ")"
        // possible subscript: IDENT "[" expr "]"
        if (accept(p, TOK_LPAREN)) {
            Expr *call = new_expr(); if (!call) return e;
            call->kind = NK_CALL;
            call->tok  = e->tok;
            call->a    = e; // callee

            // args?
            if (p->cur.kind != TOK_RPAREN) {
                // at least one expr
                Expr *arg = parse_expr(p);
                if (arg && call->args.count < MAX_ARGS)
                    call->args.items[call->args.count++] = arg;

                while (accept(p, TOK_COMMA)) {
                    arg = parse_expr(p);
                    if (arg && call->args.count < MAX_ARGS)
                        call->args.items[call->args.count++] = arg;
                }
            }
            expect(p, TOK_RPAREN, "')'");
            // also check for subscript after call: func()[index]
            if (accept(p, TOK_LBRACKET)) {
                Expr *subscript = new_expr(); if (!subscript) return call;
                subscript->kind = NK_SUBSCRIPT;
                subscript->tok = e->tok;
                subscript->a = call;
                
                Expr *index = parse_expr(p);
                if (index) subscript->b = index;
                
                expect(p, TOK_RBRACKET, "']'");
                return parse_binop_rhs(p, PREC_CMP, subscript);
            }
            return parse_binop_rhs(p, PREC_CMP, call);
        }
        // possible subscript: IDENT "[" expr "]"
        if (accept(p, TOK_LBRACKET)) {
            Expr *subscript = new_expr(); if (!subscript) return e;
            subscript->kind = NK_SUBSCRIPT;
            subscript->tok = e->tok;
            subscript->a = e;
            
            Expr *index = parse_expr(p);
            if (index) subscript->b = index;
            
            expect(p, TOK_RBRACKET, "']'");
            return parse_binop_rhs(p, PREC_CMP, subscript);
        }
        return parse_binop_rhs(p, PREC_CMP, e);
    }
    if (accept(p, TOK_LPAREN)) {
        Expr *inner = parse_expr(p);
        expect(p, TOK_RPAREN, "')'");
        Expr *pe = new_expr(); if (!pe) return inner;
        pe->kind = NK_PAREN;
        pe->tok  = p->cur;
        pe->a    = inner;
        // check for subscript after parentheses: (expr)[index]
        if (accept(p, TOK_LBRACKET)) {
            Expr *subscript = new_expr(); if (!subscript) return pe;
            subscript->kind = NK_SUBSCRIPT;
            subscript->tok = pe->tok;
            subscript->a = pe;
            
            Expr *index = parse_expr(p);
            if (index) subscript->b = index;
            
            expect(p, TOK_RBRACKET, "']'");
            return parse_binop_rhs(p, PREC_CMP, subscript);
        }
        return parse_binop_rhs(p, PREC_CMP, pe);
    }

    fprintf(stderr, "Parse error: unexpected token %s '%s' at line %d\n",
            tok_name(p->cur.kind), p->cur.text, p->cur.line);
    p->had_error = 1;
    // attempt recovery
    parser_next(p);
    return NULL;
}

/* ---------- precedence climbing for binary ops ---------- */

static Precedence precedence_of(TokenKind k) {
    switch (k) {
        case TOK_AND: case TOK_OR:
            return PREC_LOGICAL;
        case TOK_NOT:
            return PREC_NOT;
        case TOK_BIT_AND: case TOK_BIT_OR: case TOK_BIT_XOR:
            return PREC_BIT;
        case TOK_POW:
            return PREC_POW;
        case TOK_EQEQ: case TOK_NE: case TOK_LT: case TOK_GT: case TOK_LE: case TOK_GE:
            return PREC_CMP;
        case TOK_PLUS: case TOK_MINUS:
            return PREC_ADD;
        case TOK_STAR: case TOK_SLASH: case TOK_FLOORDIV: case TOK_MODULO:
            return PREC_MUL;
        default: return PREC_LOWEST;
    }
}

static int is_binop(TokenKind k) {
    return k==TOK_PLUS || k==TOK_MINUS || k==TOK_STAR || k==TOK_SLASH || k==TOK_FLOORDIV || k==TOK_MODULO || k==TOK_POW ||
           k==TOK_EQEQ || k==TOK_NE || k==TOK_LT || k==TOK_GT || k==TOK_LE || k==TOK_GE ||
           k==TOK_BIT_AND || k==TOK_BIT_OR || k==TOK_BIT_XOR ||
           k==TOK_AND || k==TOK_OR;
}

static int is_right_assoc(TokenKind k) {
    return k == TOK_POW;
}

static Expr *parse_binop_rhs(Parser *p, Precedence min_prec, Expr *lhs) {
    for (;;) {
        TokenKind opk = p->cur.kind;
        if (!is_binop(opk)) break;

        Precedence prec = precedence_of(opk);
        if (prec < min_prec) break;

        Token optok = p->cur;
        parser_next(p); // consume op

        Expr *rhs = parse_primary(p);
        // handle right associative operators: for right-assoc ops use >=, for left-assoc use >
        if (is_right_assoc(opk)) {
            while (is_binop(p->cur.kind) &&
                   precedence_of(p->cur.kind) >= prec) {
                rhs = parse_binop_rhs(p, precedence_of(p->cur.kind), rhs);
            }
        } else {
            while (is_binop(p->cur.kind) &&
                   precedence_of(p->cur.kind) > prec) {
                rhs = parse_binop_rhs(p, precedence_of(p->cur.kind), rhs);
            }
        }

        Expr *bin = new_expr(); if (!bin) return lhs;
        bin->kind = NK_BINOP;
        bin->tok  = optok;
        strncpy(bin->sval, optok.text, sizeof bin->sval); // op text for debug
        bin->a = lhs; bin->b = rhs;
        lhs = bin;
    }
    return lhs;
}

static Expr *parse_expr(Parser *p) {
    Expr *lhs = parse_primary(p);
    return parse_binop_rhs(p, PREC_CMP, lhs);
}

/* ---------- params & function defs ---------- */

static void parse_paramlist(Parser *p, ParamList *out) {
    memset(out, 0, sizeof *out);
    if (p->cur.kind == TOK_RPAREN) return;
    if (p->cur.kind != TOK_IDENT) return;

    strncpy(out->names[out->count++], p->cur.text, 64);
    parser_next(p);
    while (accept(p, TOK_COMMA)) {
        if (p->cur.kind == TOK_IDENT) {
            if (out->count < MAX_PARAMS)
                strncpy(out->names[out->count++], p->cur.text, 64);
            parser_next(p);
        } else {
            fprintf(stderr, "Parse error: expected param name at line %d\n", p->cur.line);
            p->had_error = 1;
            break;
        }
    }
}

static Stmt *parse_funcdef(Parser *p) {
    // already consumed "def"
    if (p->cur.kind != TOK_IDENT) {
        fprintf(stderr, "Parse error: expected function name after 'def' at line %d\n", p->cur.line);
        p->had_error = 1; return NULL;
    }
    Token name = p->cur; parser_next(p);

    expect(p, TOK_LPAREN, "'('");

    ParamList params;
    parse_paramlist(p, &params);

    expect(p, TOK_RPAREN, "')'");
    expect(p, TOK_COLON, "':'");

    // Body: either same-line statement OR a NEWLINE then exactly one statement
    int same_line = (p->cur.kind != TOK_NEWLINE && p->cur.kind != TOK_EOF);

    Stmt *body = NULL;
    if (same_line) {
        body = parse_stmt(p);
    } else {
        optional_newlines(p);
        body = parse_stmt(p);
    }

    Stmt *fn = new_stmt(); if (!fn) return body;
    fn->kind = SK_FUNCDEF;
    fn->tok  = name;
    strncpy(fn->fname, name.text, sizeof fn->fname);
    fn->params = params;
    fn->body   = body;
    return fn;
}

/* ---------- statements ---------- */

static Stmt *stmt_from_expr(Expr *e, Token where) {
    Stmt *s = new_stmt(); if (!s) return NULL;
    s->kind = SK_EXPR;
    s->tok  = where;
    s->expr = e;
    return s;
}

static Expr *parse_ident_prefix_then_expr(Parser *p, const Token ident_tok) {
    // construct IDENT node
    Expr *id = new_expr(); if (!id) return NULL;
    id->kind = NK_IDENT;
    id->tok  = ident_tok;
    strncpy(id->sval, ident_tok.text, sizeof id->sval);

    // optional call: IDENT "(" args? ")"
    if (accept(p, TOK_LPAREN)) {
        Expr *call = new_expr(); if (!call) return id;
        call->kind = NK_CALL;
        call->tok  = ident_tok;
        call->a    = id;

        // args?
        if (p->cur.kind != TOK_RPAREN) {
            Expr *arg = parse_expr(p);
            if (arg && call->args.count < MAX_ARGS)
                call->args.items[call->args.count++] = arg;
            while (accept(p, TOK_COMMA)) {
                arg = parse_expr(p);
                if (arg && call->args.count < MAX_ARGS)
                    call->args.items[call->args.count++] = arg;
            }
        }
            expect(p, TOK_RPAREN, "')'");
            // continue with binops using call as lhs
            // also check for subscript after call
            if (accept(p, TOK_LBRACKET)) {
                Expr *subscript = new_expr(); if (!subscript) return call;
                subscript->kind = NK_SUBSCRIPT;
                subscript->tok = ident_tok;
                subscript->a = call; // the list/array result
                
                Expr *index = parse_expr(p);
                if (index) subscript->b = index;
                
                expect(p, TOK_RBRACKET, "']'");
                return parse_binop_rhs(p, PREC_CMP, subscript);
            }
            return parse_binop_rhs(p, PREC_CMP, call);
        }
        // possible subscript: IDENT "[" expr "]"
        if (accept(p, TOK_LBRACKET)) {
            Expr *subscript = new_expr(); if (!subscript) return id;
            subscript->kind = NK_SUBSCRIPT;
            subscript->tok = ident_tok;
            subscript->a = id; // the list/array
            
            Expr *index = parse_expr(p);
            if (index) subscript->b = index; // the index
            
            expect(p, TOK_RBRACKET, "']'");
            // continue with binops using subscript as lhs
            return parse_binop_rhs(p, PREC_CMP, subscript);
        }

        // continue with binops using ident as lhs
        return parse_binop_rhs(p, PREC_CMP, id);
}


static Stmt *parse_stmt(Parser *p) {
    optional_newlines(p);
    if (p->cur.kind == TOK_EOF) return NULL;

    // return
    if (is_kw(&p->cur, "return")) {
        Token rtok = p->cur; parser_next(p);
        Expr *e = parse_expr(p);
        if (p->cur.kind == TOK_NEWLINE) parser_next(p);
        Stmt *s = new_stmt(); if (!s) return NULL;
        s->kind = SK_RETURN; s->tok = rtok; s->expr = e;
        return s;
    }

    // def
    if (is_kw(&p->cur, "def")) {
        parser_next(p);
        Stmt *s = parse_funcdef(p);
        if (p->cur.kind == TOK_NEWLINE) parser_next(p);
        return s;
    }

    // Possible assignment: IDENT '=' expr
    // for loop: for IDENT in expr: stmt
    if (is_kw(&p->cur, "for")) {
        Token for_tok = p->cur; parser_next(p);
        if (p->cur.kind != TOK_IDENT) {
            fprintf(stderr, "Parse error: expected identifier after 'for' at line %d\n", p->cur.line);
            p->had_error = 1;
            return NULL;
        }
        Token var_tok = p->cur; parser_next(p);
        
        if (!accept_kw(p, "in")) {
            fprintf(stderr, "Parse error: expected 'in' after variable name at line %d\n", p->cur.line);
            p->had_error = 1;
            return NULL;
        }
        
        Expr *iterable = parse_expr(p);
        expect(p, TOK_COLON, "':'");
        
        int same_line = (p->cur.kind != TOK_NEWLINE && p->cur.kind != TOK_EOF);
        Stmt *body = NULL;
        if (same_line) {
            body = parse_stmt(p);
        } else {
            optional_newlines(p);
            body = parse_stmt(p);
        }
        
        Stmt *s = new_stmt(); if (!s) return body;
        s->kind = SK_FOR;
        s->tok = for_tok;
        strncpy(s->lhs, var_tok.text, sizeof s->lhs);
        s->expr = iterable;
        s->body = body;
        if (p->cur.kind == TOK_NEWLINE) parser_next(p);
        return s;
    }

    if (is_kw(&p->cur, "if")) {
        return parse_if_stmt(p);
    }

    if (p->cur.kind == TOK_IDENT) {
        Token first = p->cur;         // keep name
        parser_next(p);               // consume IDENT

        if (p->cur.kind == TOK_EQ) {  // assignment
            parser_next(p);           // consume '='
            Expr *rhs = parse_expr(p);
            if (p->cur.kind == TOK_NEWLINE) parser_next(p);

            Stmt *s = new_stmt(); if (!s) return NULL;
            s->kind = SK_ASSIGN;
            s->tok  = first;
            strncpy(s->lhs, first.text, sizeof s->lhs);
            s->expr = rhs;
            return s;
        }

        // Not assignment → expression stmt starting with that IDENT
        Expr *e = parse_ident_prefix_then_expr(p, first);
        if (p->cur.kind == TOK_NEWLINE) parser_next(p);
        return stmt_from_expr(e, first);
    }

    // generic expression statement
    Expr *e = parse_expr(p);
    if (p->cur.kind == TOK_NEWLINE) parser_next(p);
    return stmt_from_expr(e, e ? e->tok : p->cur);
}

static void parse_if_arm(Parser *p, Stmt *node, const char *who) {
    Expr *cond = parse_expr(p);
    expect(p, TOK_COLON, "':'");
    int same_line = (p->cur.kind != TOK_NEWLINE && p->cur.kind != TOK_EOF);
    Stmt *body = NULL;
    if (same_line) body = parse_stmt(p); else { optional_newlines(p); body = parse_stmt(p); }

    if (node->n_arms < MAX_IF_ARMS) {
        node->conds[node->n_arms]  = cond;
        node->bodies[node->n_arms] = body;
        node->n_arms++;
    } else {
        fprintf(stderr, "Parse error: too many %s/elif arms (max %d)\n", who, MAX_IF_ARMS);
        p->had_error = 1;
    }
}

static Stmt *parse_if_stmt(Parser *p) {
    Token if_tok = p->cur; parser_next(p);
    Stmt *node = new_stmt(); if (!node) return NULL;
    node->kind = SK_IF; node->tok = if_tok; node->n_arms = 0; node->else_body = NULL;

    parse_if_arm(p, node, "if");
    if (p->cur.kind == TOK_NEWLINE) parser_next(p);

    while (is_kw(&p->cur, "elif")) { parser_next(p); parse_if_arm(p, node, "elif"); if (p->cur.kind == TOK_NEWLINE) parser_next(p); }

    if (is_kw(&p->cur, "else")) {
        parser_next(p);
        expect(p, TOK_COLON, "':'");
        int same_line = (p->cur.kind != TOK_NEWLINE && p->cur.kind != TOK_EOF);
        Stmt *eb = NULL;
        if (same_line) eb = parse_stmt(p); else { optional_newlines(p); eb = parse_stmt(p); }
        node->else_body = eb;
        if (p->cur.kind == TOK_NEWLINE) parser_next(p);
    }
    return node;
}

// Build an IDENT primary (and optional call), then continue with binops


ParseResult parse_module(Lexer *lx) {
    Parser p = {0};
    p.lx = lx;
    p.had_error = 0;

    EXPR_TOP = 0;
    STMT_TOP = 0;

    parser_next(&p);
    Module m = {0};

    while (p.cur.kind != TOK_EOF) {
        Stmt *s = parse_stmt(&p);
        if (s) {
            if (m.body.count < MAX_STMTS)
                m.body.items[m.body.count++] = s;
        } else {
            // synchronization: skip to next NEWLINE/EOF
            while (p.cur.kind != TOK_NEWLINE && p.cur.kind != TOK_EOF)
                parser_next(&p);
            if (p.cur.kind == TOK_NEWLINE) parser_next(&p);
        }
    }

    ParseResult r;
    r.mod = m;
    r.ok  = p.had_error ? 0 : 1;
    return r;
}

/* ---------- AST dumper (debug) ---------- */
static void dump_expr(const Expr *e, int depth);

static void pad(int n){ while(n--) fputc(' ', stdout); }

static void dump_args(const ExprList *xs, int d){
    for (int i=0;i<xs->count;i++){
        pad(d); printf("arg[%d]:\n", i);
        dump_expr(xs->items[i], d+2);
    }
}

static void dump_expr(const Expr *e, int depth) {
    if (!e) { pad(depth); printf("(null-expr)\n"); return; }
    switch (e->kind) {
        case NK_NUMBER: pad(depth); printf("NUMBER %lld\n", e->ival); break;
        case NK_STRING: pad(depth); printf("STRING \"%s\"\n", e->sval); break;
        case NK_IDENT:  pad(depth); printf("IDENT %s\n", e->sval); break;
        case NK_PAREN:  pad(depth); printf("PAREN\n"); dump_expr(e->a, depth+2); break;
        case NK_UNOP:
            pad(depth); printf("UNOP '%s'\n", e->sval);
            pad(depth+2); printf("operand:\n"); dump_expr(e->a, depth+4);
            break;
        case NK_CALL:
            pad(depth); printf("CALL\n");
            pad(depth+2); printf("callee:\n");
            dump_expr(e->a, depth+4);
            pad(depth+2); printf("args:\n");
            dump_args(&e->args, depth+4);
            break;
        case NK_BINOP:
            pad(depth); printf("BINOP '%s'\n", e->sval);
            pad(depth+2); printf("lhs:\n"); dump_expr(e->a, depth+4);
            pad(depth+2); printf("rhs:\n"); dump_expr(e->b, depth+4);
            break;
        default:
            pad(depth); printf("?(expr)\n");
    }
}

static void dump_stmt(const Stmt *s, int depth) {
    if (!s) { pad(depth); printf("(null-stmt)\n"); return; }
    switch (s->kind) {
        case SK_EXPR:
            pad(depth); printf("STMT: EXPR\n");
            dump_expr(s->expr, depth+2);
            break;
        case SK_RETURN:
            pad(depth); printf("STMT: RETURN\n");
            dump_expr(s->expr, depth+2);
            break;
        case SK_FUNCDEF:
            pad(depth); printf("STMT: DEF %s(", s->fname);
            for (int i=0;i<s->params.count;i++){
                printf("%s%s", i?", ":"", s->params.names[i]);
            }
            printf(")\n");
            pad(depth+2); printf("body:\n");
            dump_stmt(s->body, depth+4);
            break;
        case SK_IF: {
            pad(depth); printf("STMT: IF\n");
            for (int i = 0; i < s->n_arms; i++) {
                pad(depth+2); printf("arm %d cond:\n", i);
                dump_expr(s->conds[i], depth+4);
                pad(depth+2); printf("arm %d body:\n", i);
                dump_stmt(s->bodies[i], depth+4);
            }
            if (s->else_body) {
                pad(depth+2); printf("else:\n");
                dump_stmt(s->else_body, depth+4);
            }
            break;
        }
        default:
            pad(depth); printf("?(stmt)\n");
    }
}

void ast_dump(const Module *m) {
    printf("MODULE\n");
    for (int i=0;i<m->body.count;i++) {
        dump_stmt(m->body.items[i], 2);
    }
}

/* ---------- Optional: tiny driver if compiled alone ---------- */
#ifdef MINIPY_PARSER_STANDALONE
int main(void){
    const char *code =
        "def sq(x):\n"
        "    return x * x\n"
        "print(sq(5))\n";

    Lexer lx; lexer_init(&lx, code);
    ParseResult r = parse_module(&lx);
    if (!r.ok) { fprintf(stderr, "Parse failed\n"); return 1; }
    ast_dump(&r.mod);
    return 0;
}
#endif
