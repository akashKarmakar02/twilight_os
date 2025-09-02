#include "../include/ctype.h"

/* double-safety in case something snuck in before this include */
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

int isdigit(int c) { return c >= '0' && c <= '9'; }
int isspace(int c) { return c==' '||c=='\t'||c=='\n'||c=='\r'||c=='\f'||c=='\v'; }
int isalpha(int c) { return (c>='a'&&c<='z')||(c>='A'&&c<='Z'); }
int tolower(int c) { return (c>='A'&&c<='Z') ? c+('a'-'A') : c; }
