#include <stdio.h>
#include <sys/stat.h>
#include <utime.h>
#include <fcntl.h>
#include <unistd.h>

int main(int argc, char *argv[]) {
    if (argc < 2) {
        fprintf(stderr, "Usage: %s <file> [file...]\n", argv[0]);
        return 1;
    }

    for (int i = 1; i < argc; i++) {
        const char *filename = argv[i];
        int fd = open(filename, O_WRONLY | O_CREAT, 0644);
        if (fd < 0) {
            perror(filename);
            continue;
        }
        close(fd);

        // Update the file's access and modification time
        if (utime(filename, NULL) < 0) {
            perror(filename);
        }
    }

    return 0;
}
