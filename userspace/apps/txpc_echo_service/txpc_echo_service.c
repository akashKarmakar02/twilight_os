#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

static int send_fd(int sock, const char* msg, int fd) {
    struct iovec iov = {.iov_base = (void*)msg, .iov_len = strlen(msg)};
    char control[CMSG_SPACE(sizeof(int))];
    memset(control, 0, sizeof(control));
    struct msghdr msgh = {0};
    msgh.msg_iov = &iov;
    msgh.msg_iovlen = 1;
    msgh.msg_control = control;
    msgh.msg_controllen = sizeof(control);
    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msgh);
    cmsg->cmsg_len = CMSG_LEN(sizeof(int));
    cmsg->cmsg_level = SOL_SOCKET;
    cmsg->cmsg_type = SCM_RIGHTS;
    memcpy(CMSG_DATA(cmsg), &fd, sizeof(fd));
    return sendmsg(sock, &msgh, 0) >= 0 ? 0 : -1;
}

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

int main(void) {
    printf("txpc_echo_service: starting\n");

    int boot_sock = socket(AF_UNIX, SOCK_STREAM, 0);
    if (boot_sock < 0) {
        perror("txpc_echo_service: socket");
        return 1;
    }

    struct sockaddr_un addr = {0};
    addr.sun_family = AF_UNIX;
    strncpy(addr.sun_path, "/run/twinit/bootstrap.sock", sizeof(addr.sun_path) - 1);

    if (connect(boot_sock, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        perror("txpc_echo_service: connect bootstrap.sock");
        return 1;
    }

    int sv[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        perror("txpc_echo_service: socketpair");
        return 1;
    }

    // sv[0] is the service's endpoint, sv[1] is given to twinit
    if (send_fd(boot_sock, "TXPC_REGISTER\ndev.twilight.test\n", sv[1]) < 0) {
        perror("txpc_echo_service: send_fd TXPC_REGISTER");
        return 1;
    }
    close(sv[1]);

    char resp[256];
    ssize_t r = read(boot_sock, resp, sizeof(resp) - 1);
    if (r <= 0) {
        perror("txpc_echo_service: read response");
        return 1;
    }
    resp[r] = '\0';
    if (strncmp(resp, "TXPC_OK", 7) != 0) {
        printf("txpc_echo_service: failed to register: %s", resp);
        return 1;
    }

    printf("txpc_echo_service: successfully registered, waiting for clients...\n");

    while (1) {
        char buf[256];
        int client_fd = recv_fd(sv[0], buf, sizeof(buf));
        if (client_fd < 0) {
            perror("txpc_echo_service: recv_fd failed or twinit disconnected");
            break;
        }

        if (strncmp(buf, "TXPC_INCOMING", 13) == 0) {
            printf("txpc_echo_service: received client connection (fd=%d)\n", client_fd);

            if (write(client_fd, "hello from server\n", 18) < 0) {
                perror("txpc_echo_service: write to client");
                close(client_fd);
                continue;
            }

            char msg[128];
            ssize_t n = read(client_fd, msg, sizeof(msg) - 1);
            if (n > 0) {
                msg[n] = '\0';
                printf("txpc_echo_service: client replied: %s", msg);
            }
            close(client_fd);
        } else {
            printf("txpc_echo_service: received unknown command from twinit: %s\n", buf);
            close(client_fd);
        }
    }

    return 0;
}
