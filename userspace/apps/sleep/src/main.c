#include <stdint.h>

#define SYS_NANOSLEEP 35
#define SYS_EXIT 60
#define SYS_WRITE 1

struct timespec {
    long tv_sec;   // seconds
    long tv_nsec;  // nanoseconds
};

// Minimal syscall wrapper (up to 3 args)
static inline long syscall3(long n, long a1, long a2, long a3) {
    long ret;
    asm volatile (
        "syscall"
        : "=a"(ret)
        : "a"(n), "D"(a1), "S"(a2), "d"(a3)
        : "rcx", "r11", "memory"
    );
    return ret;
}

// Minimal write helper for errors
static void print(const char *s) {
    const char *p = s;
    while (*p) p++;
    syscall3(SYS_WRITE, 1, (long)s, p - s);
}

int main(int argc, char **argv) {
    if (argc < 2) {
        print("Usage: sleep <seconds>\n");
        syscall3(SYS_EXIT, 1, 0, 0);
    }

    // Parse seconds (simple atol)
    long seconds = 0;
    char *p = argv[1];
    while (*p >= '0' && *p <= '9') {
        seconds = seconds * 10 + (*p - '0');
        p++;
    }

    struct timespec ts;
    ts.tv_sec = seconds;
    ts.tv_nsec = 0;

    syscall3(SYS_NANOSLEEP, (long)&ts, 0, 0);

    syscall3(SYS_EXIT, 0, 0, 0);
}
