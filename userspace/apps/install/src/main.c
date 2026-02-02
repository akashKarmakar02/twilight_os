#include <stdio.h>
#include <unistd.h>

int main() {
  syscall(700);

  char username[64];
  printf("\nInstallation complete.\n");
  printf("Enter username to login: ");
  fflush(stdout);

  if (scanf("%63s", username) == 1) {
    char *argv[] = {"/bin/logind", "-u", username, NULL};
    execv("/bin/logind", argv);
    perror("execv failed");
  } else {
    printf("Failed to read username\n");
  }
  printf("It is recommended to reboot the system now.\n");
  fflush(stdout);

  return 0;
}