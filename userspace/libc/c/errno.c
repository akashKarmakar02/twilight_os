#include "../include/errno.h"

// If you have TLS, make this thread-local; otherwise a single global is fine for now.
int errno;

int *__errno_location(void) {
    return &errno;
}

char *strerror(int e) {
    switch (e) {
    case 1:  return "Operation not permitted";
    case 2:  return "No such file or directory";
    case 8:  return "Exec format error";
    case 13: return "Permission denied";
    case 14: return "Bad address";
    case 22: return "Invalid argument";
    default: return "Unknown error";
    }
}
