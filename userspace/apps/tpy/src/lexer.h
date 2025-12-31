// lexer.h
#ifndef MINIPY_LEXER_H
#define MINIPY_LEXER_H
#include <stdint.h>

typedef enum {
    TOK_EOF, TOK_UNKNOWN,
    TOK_IDENT, TOK_KEYWORD, TOK_NUMBER, TOK_STRING,
    TOK_PLUS, TOK_MINUS, TOK_STAR, TOK_SLASH,
    TOK_EQ, TOK_EQEQ, TOK_NE, TOK_LT, TOK_GT, TOK_LE, TOK_GE,
    TOK_MODULO, TOK_POW, TOK_FLOORDIV,
    TOK_BIT_AND, TOK_BIT_OR, TOK_BIT_XOR, TOK_BIT_NOT,
    TOK_AND, TOK_OR, TOK_NOT,
    TOK_LPAREN, TOK_RPAREN, TOK_LBRACKET, TOK_RBRACKET, TOK_COLON, TOK_COMMA,
    TOK_NEWLINE
} TokenKind;

typedef struct {
    TokenKind kind;
    int line, col;
    long long value;
    char text[64];
} Token;

typedef struct {
    const char *src;
    int pos;
    int line, col;
    int indent;
    int at_bol;
} Lexer;

void lexer_init(Lexer *lx, const char *src);
Token lexer_next(Lexer *lx);
const char *tok_name(TokenKind k);

#endif
