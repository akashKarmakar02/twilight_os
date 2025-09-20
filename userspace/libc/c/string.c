#include "../include/string.h"
#include <stddef.h>


size_t strlen(const char *s) { size_t n=0; while (*s++) n++; return n; }
int strcmp(const char *a, const char *b) { while (*a && *a==*b) { a++; b++; } return *(unsigned char*)a - *(unsigned char*)b; }
char *strcpy(char *d, const char *s) { char *r=d; while ((*d++ = *s++)); return r; }
char *strncpy(char *dest, const char *src, size_t n) {
    char *d = dest;
    size_t i = 0;

    // copy until n or until NUL
    for (; i < n && src[i] != '\0'; i++) {
        d[i] = src[i];
    }

    // pad the rest with NUL
    for (; i < n; i++) {
        d[i] = '\0';
    }

    return dest;
}
char *strncat(char *dest, const char *src, size_t n) {
    char *d = dest;

    // move d to the end of dest string
    while (*d) {
        d++;
    }

    // append up to n characters from src
    size_t i;
    for (i = 0; i < n && src[i] != '\0'; i++) {
        d[i] = src[i];
    }

    // always NUL terminate
    d[i] = '\0';

    return dest;
}
char *strchr(const char *s, int c) {
    char ch = (char)c;
    while (*s) {
        if (*s == ch) return (char *)s;
        s++;
    }
    return (ch == '\0') ? (char *)s : NULL;
}