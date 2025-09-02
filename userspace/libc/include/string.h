// string.h
#pragma once
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif

size_t strlen(const char *s);
int strcmp(const char *a, const char *b);
char *strcpy(char *d, const char *s);

#ifdef __cplusplus
}
#endif
