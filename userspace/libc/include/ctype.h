#pragma once
#ifdef __cplusplus
extern "C" {
#endif

/* make sure host macros don't leak in */
#ifdef isdigit
#undef isdigit
#endif
#ifdef isspace
#undef isspace
#endif
#ifdef isalpha
#undef isalpha
#endif
#ifdef tolower
#undef tolower
#endif

int isdigit(int c);
int isspace(int c);
int isalpha(int c);
int tolower(int c);

#ifdef __cplusplus
}
#endif
