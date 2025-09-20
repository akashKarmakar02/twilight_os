#pragma once
#ifdef __cplusplus
extern "C" {
#endif
extern int errno;
int *__errno_location(void);
char *strerror(int e);
#ifdef __cplusplus
}
#endif
