#include <unistd.h>
#include <sys/wait.h>
#include <stdio.h>

int main() {
   

    int pid = fork();

    if (pid < 0) {
        printf("Init fork failed\n");
        return 1;
    }

    if (pid == 0) {
        char *argv[] = { "/bin/tsh", NULL };
        char *envp[] = { "PATH=/bin", "HOME=/", NULL };
        execve("/bin/tsh", argv, envp);
        printf("Exec failed!\n");
        return 1;
    }

   
    while (1) {
        int status;
        int res = waitpid(-1, &status, 0); // Wait for ANY child
        if (res > 0) {
             printf("Child %d exited with status %d, reaping...\n", res, status);
             
             if (res == pid) { // The shell we spawned
                 printf("Shell exited, restarting...\n");
                 pid = fork();
                 if (pid == 0) {
                     char *argv[] = { "/bin/tsh", NULL };
                     char *envp[] = { "PATH=/bin", "HOME=/", NULL };
                     execve("/bin/tsh", argv, envp);
                     return 1; 
                 }
                 printf("Restarted shell with PID %d\n", pid);
             }
        }
    }
    return 0;
}
