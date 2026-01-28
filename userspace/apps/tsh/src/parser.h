#ifndef PARSER_H
#define PARSER_H

#include <stddef.h>

void free_argv(char **argv);
int parse_argv(const char *line, char ***argv_out);
void free_strv(char **v);
int split_pipeline(const char *line, char ***out_segs);
void rstrip_newline(char *s);
char *lstrip(char *s);
char *trim(char *s);

#endif
