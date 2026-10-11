/* Internet socket regression for #83. A host TCP/UDP echo peer listens on
 * 10.0.2.2:18081. Tests exercise real NIC I/O rather than kernel loopback. */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

#define CHECK(condition) do { if (!(condition)) { \
  fprintf(stderr, "NET_FAIL line=%d errno=%d\n", __LINE__, errno); exit(1); \
} } while (0)

static struct sockaddr_in peer(unsigned port) {
  struct sockaddr_in addr = { .sin_family = AF_INET, .sin_port = htons(port) };
  CHECK(inet_pton(AF_INET, "10.0.2.2", &addr.sin_addr) == 1);
  return addr;
}

static void tcp_echo(int nonblock) {
  struct sockaddr_in addr = peer(18081);
  int fd = socket(AF_INET, SOCK_STREAM | (nonblock ? SOCK_NONBLOCK : 0), 0);
  CHECK(fd >= 0);
  int result = connect(fd, (struct sockaddr *)&addr, sizeof(addr));
  if (nonblock) {
    CHECK(result == -1 && errno == EINPROGRESS);
    struct pollfd p = { .fd = fd, .events = POLLOUT };
    CHECK(poll(&p, 1, 3000) == 1 && (p.revents & POLLOUT));
    CHECK(fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) & ~O_NONBLOCK) == 0);
  } else { CHECK(result == 0); }
  unsigned char sent[4096], received[4096];
  for (size_t i = 0; i < sizeof(sent); i++) sent[i] = (unsigned char)(i * 37);
  size_t count = 0;
  while (count < sizeof(sent)) {
    ssize_t n = write(fd, sent + count, sizeof(sent) - count);
    CHECK(n > 0); count += (size_t)n;
  }
  count = 0;
  while (count < sizeof(received)) {
    ssize_t n = read(fd, received + count, sizeof(received) - count);
    CHECK(n > 0); count += (size_t)n;
  }
  CHECK(memcmp(sent, received, sizeof(sent)) == 0);
  struct pollfd p = { .fd = fd, .events = POLLIN };
  CHECK(poll(&p, 1, 3000) == 1 && (p.revents & POLLIN));
  CHECK(read(fd, received, sizeof(received)) == 0);
  close(fd);
  printf("NET_PASS tcp_%s partial_io_and_eof\n", nonblock ? "nonblock_connect" : "blocking");
}

static void udp_echo(void) {
  int fd = socket(AF_INET, SOCK_DGRAM, 0);
  CHECK(fd >= 0);
  struct sockaddr_in local = { .sin_family = AF_INET, .sin_port = htons(18082) };
  CHECK(bind(fd, (struct sockaddr *)&local, sizeof(local)) == 0);
  struct sockaddr_in addr = peer(18081);
  /* Connecting an already-bound UDP socket used to enter a HLT loop. */
  CHECK(connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0);
  CHECK(fcntl(fd, F_SETFL, O_RDWR | O_NONBLOCK) == 0);
  char buffer[8];
  CHECK(read(fd, buffer, sizeof(buffer)) == -1 && errno == EAGAIN);
  CHECK(fcntl(fd, F_SETFL, O_RDWR) == 0);
  CHECK(write(fd, "datagram", 8) == 8);
  CHECK(read(fd, buffer, sizeof(buffer)) == 8 && memcmp(buffer, "datagram", 8) == 0);
  CHECK(sendto(fd, "truncate", 8, 0, (struct sockaddr *)&addr, sizeof(addr)) == 8);
  CHECK(recvfrom(fd, buffer, 3, 0, NULL, NULL) == 3 && memcmp(buffer, "tru", 3) == 0);
  CHECK(sendto(fd, "again", 5, MSG_DONTWAIT, (struct sockaddr *)&addr, sizeof(addr)) == 5);
  CHECK(recvfrom(fd, buffer, sizeof(buffer), 0, NULL, NULL) == 5);
  close(fd);
  puts("NET_PASS udp_bound_connect_nonblock_truncation");
}

static void caught(int sig) { (void)sig; }

static void interrupt_read(void) {
  int fd = socket(AF_INET, SOCK_STREAM, 0);
  CHECK(fd >= 0);
  struct sockaddr_in addr = peer(18081);
  CHECK(connect(fd, (struct sockaddr *)&addr, sizeof(addr)) == 0);
  struct sigaction action = { .sa_handler = caught };
  sigemptyset(&action.sa_mask);
  CHECK(sigaction(SIGUSR1, &action, NULL) == 0);
  pid_t parent = getpid(), child = fork();
  CHECK(child >= 0);
  if (child == 0) { usleep(100000); kill(parent, SIGUSR1); _exit(0); }
  char byte;
  CHECK(read(fd, &byte, 1) == -1 && errno == EINTR);
  CHECK(waitpid(child, NULL, 0) == child);
  close(fd);
  puts("NET_PASS interruptible_read");
}

static void serve(void) {
  int fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, 0);
  CHECK(fd >= 0);
  struct sockaddr_in local = { .sin_family = AF_INET, .sin_port = htons(18083) };
  CHECK(bind(fd, (struct sockaddr *)&local, sizeof(local)) == 0);
  CHECK(listen(fd, 4) == 0);
  CHECK(accept(fd, NULL, NULL) == -1 && errno == EAGAIN);
  puts("NET_SERVER_READY"); fflush(stdout);
  struct pollfd p = { .fd = fd, .events = POLLIN };
  CHECK(poll(&p, 1, 10000) == 1 && (p.revents & POLLIN));
  int client = accept(fd, NULL, NULL);
  CHECK(client >= 0 && !(fcntl(client, F_GETFL) & O_NONBLOCK));
  puts("NET_ACCEPTED_WITHOUT_DATA"); fflush(stdout);
  char byte;
  CHECK(read(client, &byte, 1) == 1 && byte == 'x');
  unsigned char chunk[4096];
  for (size_t i = 0; i < sizeof(chunk); i++) chunk[i] = (unsigned char)i;
  size_t count = 0;
  while (count < 8 * 1024 * 1024) {
    ssize_t n = write(client, chunk + count % sizeof(chunk), sizeof(chunk) - count % sizeof(chunk));
    CHECK(n > 0); count += (size_t)n;
  }
  close(client); close(fd);
  puts("NET_PASS accept_no_payload_slow_read_graceful_close");
}

int main(int argc, char **argv) {
  setvbuf(stdout, NULL, _IONBF, 0);
  if (argc == 2 && strcmp(argv[1], "server") == 0) { serve(); return 0; }
  tcp_echo(0); tcp_echo(1); udp_echo(); interrupt_read();
  puts("NET_DONE client");
  return 0;
}
