// userspace/libc/include/stdio.h
#pragma once
#include <stddef.h>
#ifdef __cplusplus
extern "C" {
#endif
    int printf(const char *fmt, ...);
    int scanf(const char *fmt, ...);
    int getchar(void);
    int putchar(int c);
    int puts(const char *s);
//    int __isoc23_scanf(const char*, ...) __attribute__((alias("scanf")));
//    int __isoc99_scanf (const char*, ...) __attribute__((alias("scanf")));

    typedef struct __twlite_FILE {
        int     fd;
        char   *buf;
        size_t  len;
        size_t  cap;
        int     flags;
    } FILE;

    extern FILE *stdin;
    extern FILE *stdout;
    extern FILE *stderr;


    int fflush(FILE *stream);   // returns 0 on success, EOF (-1) on error
    // add these to your header
    char *fgets(char *s, int size, FILE *stream);
    int fprintf(FILE *stream, const char *fmt, ...); // optional, if you implement it


    #ifdef __cplusplus
}
#endif
