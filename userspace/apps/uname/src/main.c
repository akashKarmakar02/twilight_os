#define _GNU_SOURCE
#include <unistd.h>
#include <stdint.h>
#include <sys/syscall.h>

#define SYS_WRITE 1
#define SYS_UNAME 63
#define SYS_EXIT 60
#define STDOUT 1

typedef struct {
    char sysname[65];
    char nodename[65];
    char release[65];
    char version[65];
    char machine[65];
    char domainname[65];
} utsname_t;

static void write_buf(const char *buf, uint64_t n) {
    syscall(SYS_WRITE, STDOUT, buf, n);
}

static void write_cstr(const char *s) {
    const char *p = s;
    while (*p) p++;
    write_buf(s, p - s);
}

static void maybe_space(int *printed) {
    if (*printed) write_buf(" ", 1);
}

int main(int argc, char **argv) {
    utsname_t uts;
    long ret = syscall(SYS_UNAME, &uts);
    if (ret < 0) syscall(SYS_EXIT, 1);

    int flags = 0, printed = 0;

    if (argc == 1) flags |= 1<<0;

    for (int i = 1; i < argc; i++) {
        if (argv[i][0] != '-') continue;
        for (int j=1; argv[i][j]; j++) {
            switch(argv[i][j]) {
                case 'a': flags |= (1<<0)|(1<<1)|(1<<2)|(1<<3)|(1<<4)|(1<<5); break;
                case 's': flags |= 1<<0; break;
                case 'n': flags |= 1<<1; break;
                case 'r': flags |= 1<<2; break;
                case 'v': flags |= 1<<3; break;
                case 'm': flags |= 1<<4; break;
                case 'o': flags |= 1<<5; break;
                default: syscall(SYS_EXIT, 1);
            }
        }
    }

    if (flags & (1<<0)) { maybe_space(&printed); write_cstr(uts.sysname); printed=1; }
    if (flags & (1<<1)) { maybe_space(&printed); write_cstr(uts.nodename); printed=1; }
    if (flags & (1<<2)) { maybe_space(&printed); write_cstr(uts.release); printed=1; }
    if (flags & (1<<3)) { maybe_space(&printed); write_cstr(uts.version); printed=1; }
    if (flags & (1<<4)) { maybe_space(&printed); write_cstr(uts.machine); printed=1; }
    if (flags & (1<<5)) { maybe_space(&printed); write_cstr("Twilight/Next"); printed=1; }

    write_buf("\n", 1);
}
