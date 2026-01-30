#ifndef BUILTINS_H
#define BUILTINS_H

/* Returns 1 if command was handled, 0 if not a builtin, -1 on error (if handled but failed) */
int handle_builtin(int argc, char **argv);

#endif
