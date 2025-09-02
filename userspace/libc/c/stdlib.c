// stdlib.c
#include "../include/stdlib.h"
#include "../include/ctype.h"

double strtod(const char *s, char **endp) {
    double val = 0.0, frac = 0.1;
    int neg = 0;
    if (*s == '-') { neg=1; s++; }
    while (isdigit(*s)) { val = val*10 + (*s - '0'); s++; }
    if (*s == '.') {
        s++;
        while (isdigit(*s)) { val += frac*(*s - '0'); frac *= 0.1; s++; }
    }
    if (endp) *endp = (char*)s;
    return neg ? -val : val;
}
