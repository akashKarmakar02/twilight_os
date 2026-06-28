#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

/**
 * Receives a message and a file descriptor from a UNIX domain socket.
 * @param sock Socket to read from.
 * @param buf Buffer for the received message text.
 * @param buf_size Size of buf in bytes.
 * @returns The received file descriptor, or -1 on failure.
 */
static int recv_fd(int sock, char* buf, size_t buf_size) {
    struct iovec iov = {.iov_base = buf, .iov_len = buf_size - 1};
    char control[CMSG_SPACE(sizeof(int))];
    memset(control, 0, sizeof(control));
    struct msghdr msg = {0};
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control;
    msg.msg_controllen = sizeof(control);
    
    ssize_t received = recvmsg(sock, &msg, 0);
    if (received <= 0) return -1;
    buf[received] = '\0';
    
    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    if (!cmsg || cmsg->cmsg_level != SOL_SOCKET || cmsg->cmsg_type != SCM_RIGHTS) {
        return -1;
    }
    int fd = -1;
    memcpy(&fd, CMSG_DATA(cmsg), sizeof(fd));
    return fd;
}

/**
 * Connects to the bootstrap socket and exchanges a request with the target service.
 * @returns 0 on success, 1 if connection setup, request sending, or service communication fails.
 */
int main(void) {
    printf("txpc_client: starting\n");

    int boot_sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (boot_sock < 0) {
        perror("txpc_client: socket");
        return 1;
    }

    struct sockaddr_un addr = {0};
    addr.sun_family = AF_UNIX;
    strncpy(addr.sun_path, "/run/twinit/bootstrap.sock", sizeof(addr.sun_path) - 1);

    if (connect(boot_sock, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        perror("txpc_client: connect bootstrap.sock");
        return 1;
    }

    if (write(boot_sock, "TXPC_CONNECT\ndev.twilight.test\n", 31) < 0) {
        perror("txpc_client: write TXPC_CONNECT");
        return 1;
    }

    char buf[256];
    int service_fd = recv_fd(boot_sock, buf, sizeof(buf));
    
    if (service_fd < 0) {
        printf("txpc_client: failed to connect, response: %s\n", buf[0] ? buf : "<none>");
        return 1;
    }

    if (strncmp(buf, "TXPC_OK", 7) != 0) {
        printf("txpc_client: connection denied: %s\n", buf);
        return 1;
    }

    printf("txpc_client: connected to dev.twilight.test (fd=%d)\n", service_fd);
    
    if (write(service_fd, "hello from client\n", 18) < 0) {
        perror("txpc_client: write to service");
        return 1;
    }

    char resp[128];
    ssize_t n = read(service_fd, resp, sizeof(resp) - 1);
    if (n > 0) {
        resp[n] = '\0';
        printf("txpc_client: reply: %s\n", resp);
    } else {
        printf("txpc_client: no reply from service\n");
    }

    close(service_fd);
    close(boot_sock);
    return 0;
}
