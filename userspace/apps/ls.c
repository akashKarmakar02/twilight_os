#include <stdio.h>
#define _GNU_SOURCE
#include <fcntl.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <errno.h>
#include <stdint.h>

// Minimal linux_dirent64 (works with musl/glibc)
struct linux_dirent64 {
    unsigned long long d_ino;   // inode
    long long          d_off;   // offset to next dirent
    unsigned short     d_reclen;// length of this record
    unsigned char      d_type;  // DT_*
    char               d_name[];// NUL-terminated name
};

#ifndef SYS_getdents64
#  if defined(__x86_64__)
#    define SYS_getdents64 217
#  elif defined(__aarch64__)
#    define SYS_getdents64 61
#  else
#    error "unknown arch: define SYS_getdents64"
#  endif
#endif

static ssize_t write_all(int fd, const void *buf, size_t n) {
    const unsigned char *p = (const unsigned char*)buf;
    while (n) {
        ssize_t w = write(fd, p, n);
        if (w <= 0) return -1;
        p += (size_t)w;
        n -= (size_t)w;
    }
    return 0;
}

static size_t z_strlen(const char *s) {
    const char *p = s;
    while (*p) p++;
    return (size_t)(p - s);
}

static int list_dir(const char *path) {
    int dfd = openat(AT_FDCWD, path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (dfd < 0) {
        const char msg[] = "openat failed\n";
        write_all(2, msg, sizeof msg - 1);
        return 1;
    }

    // Fixed stack buffer (no malloc)
    char buf[32768];

    for (;;) {
        int nread = (int)syscall(SYS_getdents64, dfd, buf, sizeof buf);
        if (nread == 0) break;                 // EOF
        if (nread < 0) {
            const char msg[] = "getdents64 failed\n";
            write_all(2, msg, sizeof msg - 1);
            close(dfd);
            return 1;
        }

        for (int bpos = 0; bpos < nread; ) {
            struct linux_dirent64 *d = (struct linux_dirent64 *)(buf + bpos);
            const char *name = d->d_name;
            int type = d->d_type;

            // skip "." and ".."
            if (!(name[0]=='.' && (name[1]=='\0' || (name[1]=='.' && name[2]=='\0')))) {
                size_t len = z_strlen(name);
                if (type == 4) {
                    printf("\x1b[94m%s\x1b[0m\n", name);
                } else {
                    printf("%s\n", name);
                }
            }

            bpos += d->d_reclen; // advance by record length
        }
    }

    close(dfd);
    return 0;
}

int main(int argc, char **argv) {
    const char *path = (argc > 1) ? argv[1] : ".";
    return list_dir(path);
}
