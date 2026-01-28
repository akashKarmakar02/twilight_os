#ifndef EXECUTOR_H
#define EXECUTOR_H

/* Returns 0 on success, -1 on failure */
int run_command(char *const argv[]);

/* Returns 0 on success, -1 on failure */
int run_pipeline(char **segs, int nseg);

#endif
