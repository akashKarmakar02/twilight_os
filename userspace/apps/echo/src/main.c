#include <unistd.h>
#include <string.h>

int main(int argc, char **argv) {
    for (int i = 1; i < argc; i++) {
        char *s = argv[i];
        int len = strlen(s);

        // Handle quotes
        if (len >= 2 && s[0] == '"' && s[len - 1] == '"') {
            s[len - 1] = '\0';
            s++;
            len -= 2;
        }

        write(1, s, strlen(s));

        if (i < argc - 1)
            write(1, " ", 1);
    }

    write(1, "\n", 1);
    return 0;
}
