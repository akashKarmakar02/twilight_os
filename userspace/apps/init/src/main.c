#include <dirent.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

int has_users() {
  DIR *d = opendir("/home");
  if (!d)
    return 0;

  struct dirent *dir;
  int found = 0;
  while ((dir = readdir(d)) != NULL) {
    if (strcmp(dir->d_name, ".") != 0 && strcmp(dir->d_name, "..") != 0 &&
        dir->d_type == DT_DIR) {
      found = 1;
      break;
    }
  }
  closedir(d);
  return found;
}

void launch_shell_or_login() {
  int user_exists = has_users();

  if (user_exists) {
    char *argv[] = {"/bin/logind", NULL};
    char *envp[] = {"PATH=/bin", "HOME=/", NULL};
    execve("/bin/logind", argv, envp);
  } else {
    char *argv[] = {"/bin/tsh", NULL};
    char *envp[] = {"PATH=/bin", "HOME=/", NULL};
    execve("/bin/tsh", argv, envp);
  }
}

int main() {
  int pid = fork();

  if (pid < 0) {
    printf("Init fork failed\n");
    return 1;
  }

  if (pid == 0) {
    launch_shell_or_login();
    printf("Exec failed!\n");
    return 1;
  }

  while (1) {
    int status;
    int res = waitpid(-1, &status, 0); // Wait for ANY child
    if (res > 0) {
      printf("Child %d exited with status %d, reaping...\n", res, status);

      if (res == pid) { // The process we spawned
        printf("Process exited, restarting...\n");
        pid = fork();
        if (pid == 0) {
          launch_shell_or_login();
          return 1;
        }
        printf("Restarted process with PID %d\n", pid);
      }
    }
  }
  return 0;
}
