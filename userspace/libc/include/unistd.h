#pragma once
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef long ssize_t;

ssize_t write(int fd, const void *buf, size_t len);
ssize_t read(int fd, void *buf, size_t len);
void _exit(int status);
void exit(int status);

#ifdef __cplusplus
}
#endif
