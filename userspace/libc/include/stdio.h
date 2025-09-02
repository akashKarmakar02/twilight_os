// userspace/libc/include/stdio.h
#pragma once
#include <stddef.h>
#ifdef __cplusplus
extern "C" { #endif
int printf(const char *fmt, ...);
int scanf(const char *fmt, ...);
int getchar(void);
int putchar(int c);
int puts(const char *s);
int __isoc23_scanf(const char*, ...) __attribute__((alias("scanf")));
int __isoc99_scanf (const char*, ...) __attribute__((alias("scanf")));
#ifdef __cplusplus
} #endif
