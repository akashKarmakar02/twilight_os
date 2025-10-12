#include <errno.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

static void rstrip_newline(char *s) {
    size_t n = strlen(s);
    if (n && s[n-1] == '\n') s[n-1] = '\0';
}
// replace: #include <ctype.h>
static inline int ascii_isspace(unsigned char c) {
    return c==' ' || c=='\t' || c=='\n' || c=='\r' || c=='\v' || c=='\f';
}


static char *lstrip(char *s) {
    while (*s && ascii_isspace((unsigned char)*s)) s++;
    return s;
}

int main(void) {
    char prompt[4096];

    for (;;) {
        printf("root@twilight_os# ");
        if (!fgets(prompt, sizeof prompt, stdin)) break;

        rstrip_newline(prompt);
        char *cmd = lstrip(prompt);
        if (*cmd == '\0') continue;          // empty line

        if (strcmp(cmd, "exit") == 0) break;

        // Build argv: split on first space (very simple tokenizing)
        char *space = strchr(cmd, ' ');
        if (space) *space = '\0';
        char *const argv[] = { cmd, space ? space + 1 : NULL, NULL };

        // Provide an empty environment vector (safer than NULL on custom kernels)
        char *const envp[] = { NULL };

        long rc = syscall(SYS_execve, cmd, argv, envp);
        if (rc == -2) {
            // use errno, not -rc
            printf("tsh: %s: %s\n", cmd, strerror(errno));
            continue;
        }
    }
    return 0;
}
