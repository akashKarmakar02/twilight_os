#ifndef EXECUTOR_H
#define EXECUTOR_H

/* Returns 0 on success, -1 on failure */
int run_command(char *const argv[], int background);

/* Returns 0 on success, -1 on failure */
int run_pipeline(char **segs, int nseg, int background);

/* Reap completed background children without blocking. */
void reap_background_jobs(void);

#endif
