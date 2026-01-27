#include <unistd.h>
#include <sys/syscall.h>

#ifndef SYS_reboot
#define SYS_reboot 169
#endif

/*
 * Poweroff command
 * Triggers system shutdown via the reboot syscall.
 * We use the standard Linux magic numbers for poweroff, although
 * the current kernel implementation may interpret any reboot syscall as poweroff.
 */
int main() {
    // LINUX_REBOOT_MAGIC1 = 0xfee1dead
    // LINUX_REBOOT_MAGIC2 = 672274793 = 0x28121969
    // LINUX_REBOOT_CMD_POWER_OFF = 0x4321fedc
    return syscall(SYS_reboot, 0xfee1dead, 0x28121969, 0x4321fedc, 0);
}
