// lexer.c - minimal Python-style lexer for MiniPy
// (c) 2025 MIT License
// Reads source code string and produces tokens.
//
// Supports:
//  - identifiers and keywords (def, if, else, elif, while, for, in, return, break, continue, pass, and, or, not, import, print)
//  - integers
//  - strings "..." with escapes
//  - operators: + - * / // % ** = == != < > <= >= & | ^ ~ ( ) : ,
//  - newlines, indentation tokens (for Python-like block handling)
//
// Usage:
//   struct Lexer lx;
//   lexer_init(&lx, source);
//   Token t;
//   while ((t = lexer_next(&lx)).kind != TOK_EOF) { ... }

#include <ctype.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include "lexer.h"

// ---------- keywords ----------
static const char *kw[] = {
    "def","if","else","elif","while","for","in","return","break","continue","pass","and","or","not","import",NULL
};

static void set_text(Token *t, const char *s) {
    size_t i = 0;
    while (s[i] && i + 1 < sizeof t->text) { t->text[i] = s[i]; i++; }
    t->text[i] = 0;
}

static TokenKind kw_kind(const char *s) {
    for (int i=0; kw[i]; ++i) {
        if (strcmp(s, kw[i])==0) return TOK_KEYWORD;
    }
    return TOK_IDENT;
}

// ---------- lexer core ----------

void lexer_init(Lexer *lx, const char *src) {
    lx->src = src;
    lx->pos = 0;
    lx->line = 1;
    lx->col  = 1;
    lx->indent = 0;
    lx->at_bol = 1;
}

static char peek(Lexer *lx) {
    return lx->src[lx->pos];
}

static char advance(Lexer *lx) {
    char c = lx->src[lx->pos++];
    if (c=='\n'){ lx->line++; lx->col=1; lx->at_bol=1; }
    else lx->col++;
    return c;
}

static void skip_ws(Lexer *lx) {
    while (isspace((unsigned char)peek(lx)) && peek(lx)!='\n')
        advance(lx);
}

// helper: read identifier or keyword
static Token read_ident(Lexer *lx) {
    Token t = {0};
    t.kind = TOK_IDENT;
    t.line = lx->line;
    t.col  = lx->col;

    int i=0;
    while (isalnum((unsigned char)peek(lx)) || peek(lx)=='_') {
        if (i < (int)sizeof(t.text)-1)
            t.text[i++] = advance(lx);
        else advance(lx);
    }
    t.text[i]=0;
    t.kind = kw_kind(t.text);
    return t;
}

// helper: read number
static Token read_number(Lexer *lx) {
    Token t = {0};
    t.kind = TOK_NUMBER;
    t.line = lx->line;
    t.col  = lx->col;
    int i=0;
    while (isdigit((unsigned char)peek(lx))) {
        if (i<(int)sizeof(t.text)-1)
            t.text[i++] = advance(lx);
        else advance(lx);
    }
    t.text[i]=0;
    t.value = strtoll(t.text, NULL, 10);
    return t;
}

// helper: read string literal
static Token read_string(Lexer *lx) {
    Token t = {0};
    t.kind = TOK_STRING;
    t.line = lx->line;
    t.col  = lx->col;

    char quote = advance(lx); // consume "
    int i=0;
    while (peek(lx) && peek(lx)!=quote) {
        char c = advance(lx);
        if (c=='\\') {
            char n=advance(lx);
            switch(n){
                case 'n': c='\n'; break;
                case 't': c='\t'; break;
                case 'r': c='\r'; break;
                default: c=n; break;
            }
        }
        if (i<(int)sizeof(t.text)-1) t.text[i++]=c;
    }
    t.text[i]=0;
    if (peek(lx)==quote) advance(lx); // closing quote
    return t;
}

Token lexer_next(Lexer *lx) {
    Token t = {0};

    skip_ws(lx);

    char c = peek(lx);
    if (c==0) {
        t.kind=TOK_EOF; return t;
    }

    // newlines
    if (c=='\n') {
        advance(lx);
        t.kind = TOK_NEWLINE;
        t.line = lx->line-1;
        t.col = 1;
        return t;
    }

    // identifier / keyword
    if (isalpha((unsigned char)c) || c=='_')
        return read_ident(lx);

    // number
    if (isdigit((unsigned char)c))
        return read_number(lx);

    // string
    if (c=='"' || c=='\'')
        return read_string(lx);

    // operators / punctuators
    t.line = lx->line;
    t.col  = lx->col;
    switch (c) {
        case '+': advance(lx); t.kind = TOK_PLUS;   set_text(&t, "+"); break;
        case '-': advance(lx); t.kind = TOK_MINUS;  set_text(&t, "-"); break;
        case '*': advance(lx);
            if (peek(lx) == '*') { advance(lx); t.kind = TOK_POW; set_text(&t, "**"); }
            else { t.kind = TOK_STAR; set_text(&t, "*"); }
            break;
        case '/': advance(lx);
            if (peek(lx) == '/') { advance(lx); t.kind = TOK_FLOORDIV; set_text(&t, "//"); }
            else { t.kind = TOK_SLASH; set_text(&t, "/"); }
            break;
        case '%': advance(lx); t.kind = TOK_MODULO;  set_text(&t, "%"); break;
        case '(': advance(lx); t.kind = TOK_LPAREN; set_text(&t, "("); break;
        case ')': advance(lx); t.kind = TOK_RPAREN; set_text(&t, ")"); break;
        case '[': advance(lx); t.kind = TOK_LBRACKET; set_text(&t, "["); break;
        case ']': advance(lx); t.kind = TOK_RBRACKET; set_text(&t, "]"); break;
        case ':': advance(lx); t.kind = TOK_COLON;  set_text(&t, ":"); break;
        case ',': advance(lx); t.kind = TOK_COMMA;  set_text(&t, ","); break;
        case '&': advance(lx);
            if (peek(lx) == '&') { advance(lx); t.kind = TOK_AND; set_text(&t, "&&"); }
            else { t.kind = TOK_BIT_AND; set_text(&t, "&"); }
            break;
        case '|': advance(lx);
            if (peek(lx) == '|') { advance(lx); t.kind = TOK_OR; set_text(&t, "||"); }
            else { t.kind = TOK_BIT_OR; set_text(&t, "|"); }
            break;
        case '^': advance(lx); t.kind = TOK_BIT_XOR;  set_text(&t, "^"); break;
        case '~': advance(lx); t.kind = TOK_BIT_NOT;  set_text(&t, "~"); break;

        case '=':
            advance(lx);
            if (peek(lx) == '=') { advance(lx); t.kind = TOK_EQEQ; set_text(&t, "=="); }
            else { t.kind = TOK_EQ; set_text(&t, "="); }
            break;

        case '<':
            advance(lx);
            if (peek(lx) == '=') { advance(lx); t.kind = TOK_LE; set_text(&t, "<="); }
            else { t.kind = TOK_LT; set_text(&t, "<"); }
            break;

        case '>':
            advance(lx);
            if (peek(lx) == '=') { advance(lx); t.kind = TOK_GE; set_text(&t, ">="); }
            else { t.kind = TOK_GT; set_text(&t, ">"); }
            break;

        case '!':
            advance(lx);
            if (peek(lx) == '=') { advance(lx); t.kind = TOK_NE; set_text(&t, "!="); }
            else { t.kind = TOK_NOT; set_text(&t, "!"); }
            break;

        default:
            advance(lx);
            t.kind = TOK_UNKNOWN;
            char buf[2] = { c, 0 };
            set_text(&t, buf);
            break;
    }
    return t;
}

const char *tok_name(TokenKind k){
    switch(k){
        case TOK_EOF: return "EOF";
        case TOK_IDENT: return "IDENT";
        case TOK_KEYWORD: return "KEYWORD";
        case TOK_NUMBER: return "NUMBER";
        case TOK_STRING: return "STRING";
        case TOK_PLUS: return "PLUS";
        case TOK_MINUS: return "MINUS";
        case TOK_STAR: return "STAR";
        case TOK_SLASH: return "SLASH";
        case TOK_MODULO: return "MODULO";
        case TOK_POW: return "POW";
        case TOK_FLOORDIV: return "FLOORDIV";
        case TOK_EQ: return "EQ";
        case TOK_EQEQ: return "EQEQ";
        case TOK_NE: return "NE";
        case TOK_LT: return "LT";
        case TOK_GT: return "GT";
        case TOK_LE: return "LE";
        case TOK_GE: return "GE";
        case TOK_BIT_AND: return "BIT_AND";
        case TOK_BIT_OR: return "BIT_OR";
        case TOK_BIT_XOR: return "BIT_XOR";
        case TOK_BIT_NOT: return "BIT_NOT";
        case TOK_AND: return "AND";
        case TOK_OR: return "OR";
        case TOK_NOT: return "NOT";
        case TOK_LPAREN: return "LPAREN";
        case TOK_RPAREN: return "RPAREN";
        case TOK_LBRACKET: return "LBRACKET";
        case TOK_RBRACKET: return "RBRACKET";
        case TOK_COLON: return "COLON";
        case TOK_COMMA: return "COMMA";
        case TOK_NEWLINE: return "NEWLINE";
        default: return "UNKNOWN";
    }
}
