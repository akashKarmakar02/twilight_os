#define _XOPEN_SOURCE 700
#include <unistd.h>
#include <stdio.h>

int main() {
    char *c = crypt("key", "salt");
    if (c) printf("Success\n");
    return 0;
}
