#ifndef UTILS_H
#define UTILS_H

#include <stddef.h>

const char *resolve_path(const char *cmd, char out[512]);
void build_prompt(char *out, size_t outsz);

#endif
