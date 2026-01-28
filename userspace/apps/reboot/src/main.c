#include <unistd.h>
#include <sys/syscall.h>

#ifndef SYS_reboot
#define SYS_reboot 169
#endif

/*
 * Reboot command
 * Triggers system restart via the reboot syscall.
 */
int main() {
    // LINUX_REBOOT_MAGIC1 = 0xfee1dead
    // LINUX_REBOOT_MAGIC2 = 672274793 = 0x28121969
    // LINUX_REBOOT_CMD_RESTART = 0x01234567
    return syscall(SYS_reboot, 0xfee1dead, 0x28121969, 0x01234567, 0);
}
