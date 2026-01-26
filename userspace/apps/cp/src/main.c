#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <string.h>
#include <sys/stat.h>

#define BUF_SIZE 4096

void usage(const char *progname) {
    fprintf(stderr, "Usage: %s src dest\n", progname);
    exit(1);
}

int main(int argc, char *argv[]) {
    if (argc != 3) {
        usage(argv[0]);
    }

    const char *src_path = argv[1];
    const char *dest_path = argv[2];

    int src_fd = open(src_path, O_RDONLY);
    if (src_fd < 0) {
        fprintf(stderr, "cp: cannot open '%s': %s\n", src_path, strerror(errno));
        return 1;
    }

    struct stat st;
    if (fstat(src_fd, &st) < 0) {
        fprintf(stderr, "cp: cannot stat '%s': %s\n", src_path, strerror(errno));
        close(src_fd);
        return 1;
    }

    int dest_fd = open(dest_path, O_WRONLY | O_CREAT | O_TRUNC, st.st_mode & 0777);
    if (dest_fd < 0) {
        fprintf(stderr, "cp: cannot create '%s': %s\n", dest_path, strerror(errno));
        close(src_fd);
        return 1;
    }

    char buf[BUF_SIZE];
    ssize_t bytes_read, bytes_written;

    while ((bytes_read = read(src_fd, buf, BUF_SIZE)) > 0) {
        char *ptr = buf;
        while (bytes_read > 0) {
            bytes_written = write(dest_fd, ptr, bytes_read);
            if (bytes_written < 0) {
                fprintf(stderr, "cp: write error: %s\n", strerror(errno));
                close(src_fd);
                close(dest_fd);
                return 1;
            }
            bytes_read -= bytes_written;
            ptr += bytes_written;
        }
    }

    if (bytes_read < 0) {
        fprintf(stderr, "cp: read error: %s\n", strerror(errno));
    }

    close(src_fd);
    close(dest_fd);

    return (bytes_read < 0) ? 1 : 0;
}
