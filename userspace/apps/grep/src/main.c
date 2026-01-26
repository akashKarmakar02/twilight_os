#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <errno.h>

void usage(const char *progname) {
    fprintf(stderr, "Usage: %s [-n] pattern [file]\n", progname);
    exit(1);
}

int grep_file(FILE *fp, const char *pattern, int show_line_numbers, const char *filename) {
    char *line = NULL;
    size_t len = 0;
    ssize_t read;
    int line_num = 0;
    int matches = 0;

    while ((read = getline(&line, &len, fp)) != -1) {
        line_num++;
        if (strstr(line, pattern) != NULL) {
            if (show_line_numbers) {
                if (filename)
                    printf("%s:%d:%s", filename, line_num, line);
                else
                    printf("%d:%s", line_num, line);
            } else {
                if (filename)
                    printf("%s:%s", filename, line);
                else
                    printf("%s", line);
            }
            matches++;
        }
    }

    free(line);
    return matches;
}

int main(int argc, char *argv[]) {
    int show_line_numbers = 0;
    const char *pattern = NULL;
    const char *filename = NULL;
    int arg_idx = 1;

    if (argc < 2) {
        usage(argv[0]);
    }

    if (strcmp(argv[arg_idx], "-n") == 0) {
        show_line_numbers = 1;
        arg_idx++;
    }

    if (arg_idx >= argc) {
        usage(argv[0]);
    }

    pattern = argv[arg_idx++];
    
    // Check if we have a file argument
    if (arg_idx < argc) {
        filename = argv[arg_idx];
        FILE *fp = fopen(filename, "r");
        if (!fp) {
            fprintf(stderr, "grep: %s: %s\n", filename, strerror(errno));
            return 1;
        }
        int count = grep_file(fp, pattern, show_line_numbers, NULL); // Don't print filename if only one file
        fclose(fp);
        return count > 0 ? 0 : 1;
    } else {
        // Read from stdin
        int count = grep_file(stdin, pattern, show_line_numbers, NULL);
        return count > 0 ? 0 : 1;
    }
}
