#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <ctype.h>
#include <errno.h>

void usage(const char *progname) {
    fprintf(stderr, "Usage: %s [-l] [-w] [-c] [file]\n", progname);
    exit(1);
}

void count_file(FILE *fp, const char *name, int show_lines, int show_words, int show_bytes) {
    long lines = 0;
    long words = 0;
    long bytes = 0;
    int in_word = 0;
    int c;

    while ((c = fgetc(fp)) != EOF) {
        bytes++;
        if (c == '\n') {
            lines++;
        }
        if (isspace(c)) {
            in_word = 0;
        } else if (!in_word) {
            in_word = 1;
            words++;
        }
    }

    if (show_lines) printf("%ld ", lines);
    if (show_words) printf("%ld ", words);
    if (show_bytes) printf("%ld ", bytes);
    if (name) printf("%s", name);
    printf("\n");
}

int main(int argc, char *argv[]) {
    int show_lines = 0;
    int show_words = 0;
    int show_bytes = 0;
    const char *filename = NULL;

    for (int i = 1; i < argc; i++) {
        if (argv[i][0] == '-') {
            for (size_t j = 1; j < strlen(argv[i]); j++) {
                switch (argv[i][j]) {
                    case 'l': show_lines = 1; break;
                    case 'w': show_words = 1; break;
                    case 'c': show_bytes = 1; break;
                    default: usage(argv[0]);
                }
            }
        } else {
            filename = argv[i];
        }
    }

    if (!show_lines && !show_words && !show_bytes) {
        show_lines = show_words = show_bytes = 1;
    }

    if (filename) {
        FILE *fp = fopen(filename, "r");
        if (!fp) {
            fprintf(stderr, "wc: %s: %s\n", filename, strerror(errno));
            return 1;
        }
        count_file(fp, filename, show_lines, show_words, show_bytes);
        fclose(fp);
    } else {
        count_file(stdin, NULL, show_lines, show_words, show_bytes);
    }

    return 0;
}
