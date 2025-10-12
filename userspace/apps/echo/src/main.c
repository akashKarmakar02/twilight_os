#include <unistd.h>

int main(int argc, char **argv) {
    for (int i = 1; i < argc; i++) {
        char *s = argv[i];
        // write string
        char *p = s;
        while (*p) p++;
        write(1, s, p - s);

        // if not last arg, write space
        if (i < argc - 1) {
            write(1, " ", 1);
        }
    }

    // write newline
    write(1, "\n", 1);
    return 0;
}
