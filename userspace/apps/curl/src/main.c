#include <arpa/inet.h>
#include <errno.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

typedef struct {
  char host[256];
  uint16_t port;
  char path[1024];
  int print_headers;
} url_t;

static void usage(void) {
  const char *msg =
      "usage: curl [-i] http://host[:port]/path\n"
      "       curl [-i] host[:port]/path\n"
      "\n"
      "notes:\n"
      "  - https is not supported (no TLS)\n"
      "  - DNS uses a simple UDP query to 8.8.8.8\n";
  write(2, msg, strlen(msg));
}

static int parse_url(const char *in, url_t *out) {
  memset(out, 0, sizeof(*out));
  out->port = 80;
  strcpy(out->path, "/");

  const char *s = in;
  if (strncmp(s, "http://", 7) == 0) {
    s += 7;
  } else if (strncmp(s, "https://", 8) == 0) {
    return -2;
  }

  const char *slash = strchr(s, '/');
  size_t host_len = slash ? (size_t)(slash - s) : strlen(s);
  if (host_len == 0 || host_len >= sizeof(out->host))
    return -1;

  char hostport[300];
  if (host_len >= sizeof(hostport))
    return -1;
  memcpy(hostport, s, host_len);
  hostport[host_len] = 0;

  const char *colon = strchr(hostport, ':');
  if (colon) {
    size_t hl = (size_t)(colon - hostport);
    if (hl == 0 || hl >= sizeof(out->host))
      return -1;
    memcpy(out->host, hostport, hl);
    out->host[hl] = 0;
    long p = strtol(colon + 1, NULL, 10);
    if (p <= 0 || p > 65535)
      return -1;
    out->port = (uint16_t)p;
  } else {
    strncpy(out->host, hostport, sizeof(out->host) - 1);
    out->host[sizeof(out->host) - 1] = 0;
  }

  if (slash) {
    strncpy(out->path, slash, sizeof(out->path) - 1);
    out->path[sizeof(out->path) - 1] = 0;
  }

  return 0;
}

static uint16_t dns_rand16(void) {
  struct timespec ts;
  if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {
    return (uint16_t)getpid();
  }
  uint64_t mix = (uint64_t)ts.tv_nsec ^ ((uint64_t)ts.tv_sec << 16) ^
                 ((uint64_t)getpid() << 1);
  return (uint16_t)(mix ^ (mix >> 16));
}

static int dns_encode_qname(uint8_t *out, size_t out_cap, const char *name,
                            size_t *out_len) {
  size_t n = 0;
  const char *p = name;
  while (*p) {
    const char *dot = strchr(p, '.');
    size_t lab_len = dot ? (size_t)(dot - p) : strlen(p);
    if (lab_len == 0 || lab_len > 63)
      return -1;
    if (n + 1 + lab_len >= out_cap)
      return -1;
    out[n++] = (uint8_t)lab_len;
    memcpy(out + n, p, lab_len);
    n += lab_len;
    if (!dot)
      break;
    p = dot + 1;
  }
  if (n + 1 >= out_cap)
    return -1;
  out[n++] = 0;
  *out_len = n;
  return 0;
}

static int dns_parse_name(const uint8_t *buf, size_t len, size_t *off) {
  size_t i = *off;
  if (i >= len)
    return -1;
  for (;;) {
    if (i >= len)
      return -1;
    uint8_t c = buf[i];
    if (c == 0) {
      i += 1;
      break;
    }
    if ((c & 0xC0) == 0xC0) {
      if (i + 1 >= len)
        return -1;
      i += 2;
      break;
    }
    size_t lab_len = c;
    i += 1;
    if (i + lab_len > len)
      return -1;
    i += lab_len;
  }
  *off = i;
  return 0;
}

static int dns_resolve_a(const char *name, struct in_addr *out_addr) {
  uint8_t query[512];
  memset(query, 0, sizeof(query));

  uint16_t id = dns_rand16();
  query[0] = (uint8_t)(id >> 8);
  query[1] = (uint8_t)(id & 0xFF);
  query[2] = 0x01; // RD
  query[3] = 0x00;
  query[4] = 0x00;
  query[5] = 0x01; // QDCOUNT=1

  size_t off = 12;
  size_t qn_len = 0;
  if (dns_encode_qname(query + off, sizeof(query) - off, name, &qn_len) != 0)
    return -1;
  off += qn_len;
  if (off + 4 > sizeof(query))
    return -1;
  query[off + 0] = 0x00;
  query[off + 1] = 0x01; // QTYPE=A
  query[off + 2] = 0x00;
  query[off + 3] = 0x01; // QCLASS=IN
  off += 4;

  int s = socket(AF_INET, SOCK_DGRAM, 0);
  if (s < 0)
    return -1;

  struct sockaddr_in dns;
  memset(&dns, 0, sizeof(dns));
  dns.sin_family = AF_INET;
  dns.sin_port = htons(53);
  inet_pton(AF_INET, "8.8.8.8", &dns.sin_addr);

  if (connect(s, (struct sockaddr *)&dns, sizeof(dns)) != 0) {
    close(s);
    return -1;
  }

  if (send(s, query, off, 0) < 0) {
    close(s);
    return -1;
  }

  struct pollfd pfd;
  pfd.fd = s;
  pfd.events = POLLIN;
  int pr = poll(&pfd, 1, 3000);
  if (pr <= 0) {
    close(s);
    return -1;
  }

  uint8_t resp[512];
  ssize_t n = recv(s, resp, sizeof(resp), 0);
  close(s);
  if (n < 12)
    return -1;

  uint16_t rid = (uint16_t)((resp[0] << 8) | resp[1]);
  if (rid != id)
    return -1;
  uint16_t flags = (uint16_t)((resp[2] << 8) | resp[3]);
  if ((flags & 0x8000) == 0)
    return -1;
  if ((flags & 0x000F) != 0)
    return -1;
  uint16_t qd = (uint16_t)((resp[4] << 8) | resp[5]);
  uint16_t an = (uint16_t)((resp[6] << 8) | resp[7]);

  size_t roff = 12;
  for (uint16_t i = 0; i < qd; i++) {
    if (dns_parse_name(resp, (size_t)n, &roff) != 0)
      return -1;
    if (roff + 4 > (size_t)n)
      return -1;
    roff += 4;
  }

  for (uint16_t i = 0; i < an; i++) {
    if (dns_parse_name(resp, (size_t)n, &roff) != 0)
      return -1;
    if (roff + 10 > (size_t)n)
      return -1;
    uint16_t type = (uint16_t)((resp[roff + 0] << 8) | resp[roff + 1]);
    uint16_t cls = (uint16_t)((resp[roff + 2] << 8) | resp[roff + 3]);
    uint16_t rdlen = (uint16_t)((resp[roff + 8] << 8) | resp[roff + 9]);
    roff += 10;
    if (roff + rdlen > (size_t)n)
      return -1;
    if (type == 1 && cls == 1 && rdlen == 4) {
      memcpy(out_addr, resp + roff, 4);
      return 0;
    }
    roff += rdlen;
  }

  return -1;
}

static int connect_tcp(const char *host, uint16_t port,
                       struct sockaddr_in *out_peer) {
  struct sockaddr_in peer;
  memset(&peer, 0, sizeof(peer));
  peer.sin_family = AF_INET;
  peer.sin_port = htons(port);

  if (inet_pton(AF_INET, host, &peer.sin_addr) != 1) {
    struct in_addr addr;
    if (dns_resolve_a(host, &addr) != 0) {
      return -1;
    }
    peer.sin_addr = addr;
  }

  int s = socket(AF_INET, SOCK_STREAM, 0);
  if (s < 0)
    return -1;

  if (connect(s, (struct sockaddr *)&peer, sizeof(peer)) != 0) {
    close(s);
    return -1;
  }

  if (out_peer)
    *out_peer = peer;
  return s;
}

int main(int argc, char **argv) {
  url_t url;
  const char *arg_url = NULL;

  url.print_headers = 0;
  for (int i = 1; i < argc; i++) {
    if (strcmp(argv[i], "-i") == 0) {
      url.print_headers = 1;
    } else if (argv[i][0] == '-') {
      usage();
      return 2;
    } else {
      arg_url = argv[i];
    }
  }

  if (!arg_url) {
    usage();
    return 2;
  }

  int pr = parse_url(arg_url, &url);
  if (pr == -2) {
    const char *msg = "curl: https not supported\n";
    write(2, msg, strlen(msg));
    return 2;
  }
  if (pr != 0) {
    usage();
    return 2;
  }

  struct sockaddr_in peer;
  int s = connect_tcp(url.host, url.port, &peer);
  if (s < 0) {
    char buf[256];
    snprintf(buf, sizeof(buf), "curl: connect failed: %s\n", strerror(errno));
    write(2, buf, strlen(buf));
    return 1;
  }

  char req[1600];
  snprintf(req, sizeof(req),
           "GET %s HTTP/1.1\r\n"
           "Host: %s\r\n"
           "User-Agent: twilight-curl/0.1\r\n"
           "Connection: close\r\n"
           "\r\n",
           url.path, url.host);

  ssize_t wn = send(s, req, strlen(req), 0);
  if (wn < 0) {
    close(s);
    const char *msg = "curl: send failed\n";
    write(2, msg, strlen(msg));
    return 1;
  }

  int seen_headers = url.print_headers;
  char rbuf[2048];
  char header_buf[8192];
  size_t header_len = 0;

  for (;;) {
    struct pollfd fds[2];
    fds[0].fd = s;
    fds[0].events = POLLIN;
    fds[0].revents = 0;
    fds[1].fd = 0;
    fds[1].events = POLLIN;
    fds[1].revents = 0;

    int pr = poll(fds, 2, -1);
    if (pr < 0) {
      if (errno == EINTR)
        continue;
      break;
    }

    if (fds[1].revents & POLLIN) {
      char c = 0;
      ssize_t cn = read(0, &c, 1);
      if (cn == 1 && (unsigned char)c == 0x03) {
        close(s);
        return 130;
      }
    }

    if (!(fds[0].revents & (POLLIN | POLLHUP)))
      continue;

    ssize_t rn = recv(s, rbuf, sizeof(rbuf), 0);
    if (rn == 0)
      break;
    if (rn < 0) {
      if (errno == EINTR)
        continue;
      break;
    }

    if (!seen_headers) {
      size_t need = header_len + (size_t)rn;
      if (need > sizeof(header_buf)) {
        // headers too large; fall back to printing everything
        seen_headers = 1;
        write(1, rbuf, (size_t)rn);
        continue;
      }
      memcpy(header_buf + header_len, rbuf, (size_t)rn);
      header_len = need;

      char *p = NULL;
      for (size_t i = 0; i + 3 < header_len; i++) {
        if (header_buf[i] == '\r' && header_buf[i + 1] == '\n' &&
            header_buf[i + 2] == '\r' && header_buf[i + 3] == '\n') {
          p = header_buf + i + 4;
          size_t body_off = (size_t)(p - header_buf);
          size_t body_len = header_len - body_off;
          if (body_len > 0) {
            write(1, p, body_len);
          }
          seen_headers = 1;
          header_len = 0;
          break;
        }
      }
      continue;
    }

    write(1, rbuf, (size_t)rn);
  }

  close(s);
  return 0;
}
