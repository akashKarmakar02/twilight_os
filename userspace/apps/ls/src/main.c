#include <stdio.h>
#define _GNU_SOURCE
#include <fcntl.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <sys/stat.h>
#include <stdint.h>

struct linux_dirent64 {
    unsigned long long d_ino;
    long long          d_off;
    unsigned short     d_reclen;
    unsigned char      d_type;   // DT_*
    char               d_name[];
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

#ifndef SYS_stat
#  define SYS_stat 4
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
static size_t z_strlen(const char *s) { const char *p=s; while(*p) p++; return (size_t)(p-s); }
static void z_strcpy(char *dst, const char *src) { while ((*dst++ = *src++)); }
static void z_strcat(char *dst, const char *src) { while (*dst) dst++; while ((*dst++ = *src++)); }

static int is_dot_or_dotdot(const char *name) {
    return (name[0]=='.' && (name[1]=='\0' || (name[1]=='.' && name[2]=='\0')));
}

static void mode_to_permstr(mode_t m, char out[11]) {
    // file type
    out[0] = S_ISDIR(m)  ? 'd' :
             S_ISLNK(m)  ? 'l' :
             S_ISCHR(m)  ? 'c' :
             S_ISBLK(m)  ? 'b' :
             S_ISSOCK(m) ? 's' :
             S_ISFIFO(m) ? 'p' : '-';
    out[1] = (m & S_IRUSR)?'r':'-';
    out[2] = (m & S_IWUSR)?'w':'-';
    out[3] = (m & S_IXUSR)?'x':'-';
    out[4] = (m & S_IRGRP)?'r':'-';
    out[5] = (m & S_IWGRP)?'w':'-';
    out[6] = (m & S_IXGRP)?'x':'-';
    out[7] = (m & S_IROTH)?'r':'-';
    out[8] = (m & S_IWOTH)?'w':'-';
    out[9] = (m & S_IXOTH)?'x':'-';
    if (m & S_ISUID) out[3] = (out[3]=='x')?'s':'S';
    if (m & S_ISGID) out[6] = (out[6]=='x')?'s':'S';
    if (m & S_ISVTX) out[9] = (out[9]=='x')?'t':'T';
    out[10] = '\0';
}

static void print_name_colored(const char *name, int d_type, mode_t mode) {
    int is_dir = (d_type==4) || (S_ISDIR(mode));
    int is_chr = (d_type==2) || (S_ISCHR(mode)); // DT_CHR == 2
    if (is_dir) {
        printf("\x1b[94m%s\x1b[0m", name);
    } else if (is_chr) {
        printf("\x1b[33m%s\x1b[0m", name);
    } else {
        printf("%s", name);
    }
}

static int list_dir(const char *path, int flag_long, int flag_all) {
    int dfd = openat(AT_FDCWD, path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (dfd < 0) {
        const char msg[] = "openat failed\n";
        write_all(2, msg, sizeof msg - 1);
        return 1;
    }

    char buf[32768];

    for (;;) {
        int nread = (int)syscall(SYS_getdents64, dfd, buf, sizeof buf);
        if (nread == 0) break;     // EOF
        if (nread < 0) {
            const char msg[] = "getdents64 failed\n";
            write_all(2, msg, sizeof msg - 1);
            close(dfd);
            return 1;
        }

        for (int bpos = 0; bpos < nread; ) {
            struct linux_dirent64 *d = (struct linux_dirent64 *)(buf + bpos);
            const char *name = d->d_name;

            if (!flag_all && is_dot_or_dotdot(name)) {
                bpos += d->d_reclen;
                continue;
            }

            if (!flag_long) {
                print_name_colored(name, d->d_type, 0);
                putchar('\n');
            } else {
                char full[4096];
                full[0] = '\0';
                if (path[0]=='\0') { full[0] = '.'; full[1] = '\0'; }
                z_strcpy(full, path);
                size_t plen = z_strlen(full);
                if (plen==0 || full[plen-1] != '/') z_strcat(full, "/");
                z_strcat(full, name);

                struct stat st;
                int rc = (int)syscall(SYS_stat, full, &st);
                if (rc < 0) {
                    char perms[11]; mode_to_permstr(0, perms);
                    printf("%s %3d %5d %5d %9d ",
                           perms, 0, 0, 0, 0);
                    print_name_colored(name, d->d_type, 0);
                    putchar('\n');
                } else {
                    char perms[11]; mode_to_permstr(st.st_mode, perms);
                    printf("%s %3lu %5u %5u %9lld ",
                           perms,
                           (unsigned long)st.st_nlink,
                           (unsigned)st.st_uid,
                           (unsigned)st.st_gid,
                           (long long)st.st_size);
                    print_name_colored(name, d->d_type, st.st_mode);
                    putchar('\n');
                }
            }

            bpos += d->d_reclen;
        }
    }

    close(dfd);
    return 0;
}

int main(int argc, char **argv) {
    int flag_long = 0, flag_all = 0;
    const char *path = ".";
    for (int i = 1; i < argc; i++) {
        const char *a = argv[i];
        if (a[0] == '-' && a[1] != '\0') {
            for (int j = 1; a[j]; j++) {
                if (a[j] == 'l') flag_long = 1;
                else if (a[j] == 'a') flag_all = 1;
            }
        } else {
            path = a;
        }
    }
    return list_dir(path, flag_long, flag_all);
}
