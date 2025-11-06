#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <errno.h>
#include <string.h>

int main(int argc, char *argv[]) {
    if (argc != 2) {
        fprintf(stderr, "Usage: %s <directory>\n", argv[0]);
        return 1;
    }

    const char *dir = argv[1];

    if (rmdir(dir) == -1) {
        fprintf(stderr, "Error removing directory '%s': %s\n", dir, strerror(errno));
        return 1;
    }

    printf("Directory '%s' removed successfully.\n", dir);
    return 0;
}
