// string.h
#pragma once
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif

size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
char *strcpy(char *d, const char *s);
char *strncpy(char *d, const char *s, size_t n);

#ifdef __cplusplus
}
#endif
