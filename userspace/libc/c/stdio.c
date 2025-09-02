// userspace/libc/c/stdio.c
#include "../include/unistd.h"
int getchar(void){ unsigned char c; long r=read(0,&c,1); return r<=0? -1 : c; }
int putchar(int c){ unsigned char ch=(unsigned char)c; return (int)write(1,&ch,1); }
int puts(const char *s){ long n=0; while (*s) { n+=write(1,s,1); s++; } n+=write(1,"\n",1); return (int)n; }
