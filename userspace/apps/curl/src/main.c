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
#if defined(__has_include)
#if __has_include(<wolfssl/options.h>)
#include <wolfssl/options.h>
#endif
#endif
#include <wolfssl/ssl.h>

#ifndef WOLFSSL_SNI_HOST_NAME
#define WOLFSSL_SNI_HOST_NAME 0
int wolfSSL_UseSNI(WOLFSSL *ssl, unsigned char type, const void *data,
                   unsigned short size);
#endif

#define DNS_SERVER "8.8.8.8"
#define HEADER_BUF_CAP 8192

typedef struct {
  char host[256];
  uint16_t port;
  char path[1024];
  int is_https;
  int print_headers;
  char *output_file;
  int remote_name;
} url_t;

typedef struct {
  int fd;
  int use_tls;
  WOLFSSL_CTX *ctx;
  WOLFSSL *ssl;
} transport_t;

static void usage(void) {
  const char *msg =
      "usage: curl [options] url\n"
      "options:\n"
      "  -i          Include protocol response headers in the output\n"
      "  -o <file>   Write output to <file>\n"
      "  -O          Write output to a local file named like the remote file\n"
      "\n"
      "notes:\n"
      "  - supports http and https (wolfSSL)\n"
      "  - DNS uses a simple UDP query to 8.8.8.8\n";
  write(2, msg, strlen(msg));
}

static int parse_url(const char *in, url_t *out) {
  out->port = 80;
  out->is_https = 0;
  strcpy(out->path, "/");

  const char *s = in;
  if (strncmp(s, "http://", 7) == 0) {
    s += 7;
  } else if (strncmp(s, "https://", 8) == 0) {
    s += 8;
    out->is_https = 1;
    out->port = 443;
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
    size_t hl = strlen(hostport);
    if (hl >= sizeof(out->host))
      return -1;
    memcpy(out->host, hostport, hl + 1);
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
  query[2] = 0x01;
  query[3] = 0x00;
  query[4] = 0x00;
  query[5] = 0x01;

  size_t off = 12;
  size_t qn_len = 0;
  if (dns_encode_qname(query + off, sizeof(query) - off, name, &qn_len) != 0)
    return -1;
  off += qn_len;
  if (off + 4 > sizeof(query))
    return -1;
  query[off + 0] = 0x00;
  query[off + 1] = 0x01;
  query[off + 2] = 0x00;
  query[off + 3] = 0x01;
  off += 4;

  int s = socket(AF_INET, SOCK_DGRAM, 0);
  if (s < 0)
    return -1;

  struct sockaddr_in dns;
  memset(&dns, 0, sizeof(dns));
  dns.sin_family = AF_INET;
  dns.sin_port = htons(53);
  inet_pton(AF_INET, DNS_SERVER, &dns.sin_addr);

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

static int connect_tcp(const char *host, uint16_t port) {
  struct sockaddr_in peer;
  memset(&peer, 0, sizeof(peer));
  peer.sin_family = AF_INET;
  peer.sin_port = htons(port);

  if (inet_pton(AF_INET, host, &peer.sin_addr) != 1) {
    struct in_addr addr;
    if (dns_resolve_a(host, &addr) != 0)
      return -1;
    peer.sin_addr = addr;
  }

  int s = socket(AF_INET, SOCK_STREAM, 0);
  if (s < 0)
    return -1;
  if (connect(s, (struct sockaddr *)&peer, sizeof(peer)) != 0) {
    close(s);
    return -1;
  }
  return s;
}

static int write_all_fd(int fd, const void *buf, size_t len) {
  const uint8_t *p = (const uint8_t *)buf;
  size_t off = 0;
  while (off < len) {
    ssize_t n = write(fd, p + off, len - off);
    if (n < 0) {
      if (errno == EINTR)
        continue;
      return -1;
    }
    off += (size_t)n;
  }
  return 0;
}

static void draw_progress(size_t current, size_t total) {
  const int bar_width = 40;
  float ratio = 0.0f;
  if (total > 0)
    ratio = (float)current / (float)total;
  if (ratio > 1.0f)
    ratio = 1.0f;

  int filled = (int)(ratio * bar_width);
  char bar[64];
  memset(bar, ' ', sizeof(bar));
  bar[bar_width] = 0;
  for (int i = 0; i < filled; i++)
    bar[i] = '=';
  if (filled < bar_width)
    bar[filled] = '>';

  char msg[160];
  int percent = (int)(ratio * 100.0f);
  int len;
  if (total > 0) {
    len = snprintf(msg, sizeof(msg), "\r[%-40s] %3d%% %lu/%lu bytes", bar,
                   percent, (unsigned long)current, (unsigned long)total);
  } else {
    len = snprintf(msg, sizeof(msg), "\rDownloaded: %lu bytes",
                   (unsigned long)current);
  }
  if (len > 0)
    write(2, msg, (size_t)len);
}

static size_t parse_content_length(const char *headers) {
  const char *needle = "Content-Length:";
  const char *p = strstr(headers, needle);
  if (!p)
    return 0;
  p += strlen(needle);
  while (*p == ' ' || *p == '\t')
    p++;
  return (size_t)strtoull(p, NULL, 10);
}

static char *get_filename_from_path(char *path) {
  char *p = strrchr(path, '/');
  if (p) {
    if (*(p + 1) == 0)
      return "index.html";
    return p + 1;
  }
  if (strlen(path) == 0)
    return "index.html";
  return path;
}

static const char *find_ca_bundle(void) {
  static const char *candidates[] = {
      "/etc/ssl/cert.pem",
      "/etc/ssl/certs/ca-certificates.crt",
      "/etc/ssl/ca-bundle.pem",
  };
  for (size_t i = 0; i < sizeof(candidates) / sizeof(candidates[0]); i++) {
    if (access(candidates[i], R_OK) == 0) {
      return candidates[i];
    }
  }
  return NULL;
}

static void print_wolfssl_error(const char *where, WOLFSSL *ssl, int ret) {
  int err = wolfSSL_get_error(ssl, ret);
  char errbuf[160];
  wolfSSL_ERR_error_string_n((unsigned long)err, errbuf, sizeof(errbuf));
  fprintf(stderr, "curl: %s failed: wolfssl=%d (%s)\n", where, err, errbuf);
}

static int transport_write(transport_t *t, const void *buf, size_t len) {
  const uint8_t *p = (const uint8_t *)buf;
  size_t off = 0;

  while (off < len) {
    if (!t->use_tls) {
      ssize_t n = send(t->fd, p + off, len - off, 0);
      if (n < 0) {
        if (errno == EINTR)
          continue;
        return -1;
      }
      off += (size_t)n;
    } else {
      int n = wolfSSL_write(t->ssl, p + off, (int)(len - off));
      if (n > 0) {
        off += (size_t)n;
        continue;
      }
      int err = wolfSSL_get_error(t->ssl, n);
      if (err == WOLFSSL_ERROR_WANT_READ || err == WOLFSSL_ERROR_WANT_WRITE)
        continue;
      print_wolfssl_error("TLS write", t->ssl, n);
      errno = EIO;
      return -1;
    }
  }
  return 0;
}

static ssize_t transport_read(transport_t *t, void *buf, size_t len) {
  if (!t->use_tls) {
    for (;;) {
      ssize_t n = recv(t->fd, buf, len, 0);
      if (n < 0 && errno == EINTR)
        continue;
      return n;
    }
  }

  for (;;) {
    int n = wolfSSL_read(t->ssl, buf, (int)len);
    if (n > 0)
      return n;
    if (n == 0)
      return 0;

    int err = wolfSSL_get_error(t->ssl, n);
    if (err == WOLFSSL_ERROR_WANT_READ || err == WOLFSSL_ERROR_WANT_WRITE)
      continue;
    if (err == WOLFSSL_ERROR_ZERO_RETURN)
      return 0;
    print_wolfssl_error("TLS read", t->ssl, n);
    errno = EIO;
    return -1;
  }
}

static void transport_close(transport_t *t) {
  if (t->ssl) {
    wolfSSL_shutdown(t->ssl);
    wolfSSL_free(t->ssl);
    t->ssl = NULL;
  }
  if (t->ctx) {
    wolfSSL_CTX_free(t->ctx);
    t->ctx = NULL;
  }
  if (t->fd >= 0) {
    close(t->fd);
    t->fd = -1;
  }
}

static int transport_enable_tls(transport_t *t, const url_t *url) {
  wolfSSL_Init();

  WOLFSSL_METHOD *method = wolfTLS_client_method();
  if (!method) {
    fprintf(stderr, "curl: TLS method init failed\n");
    return -1;
  }

  t->ctx = wolfSSL_CTX_new(method);
  if (!t->ctx) {
    fprintf(stderr, "curl: TLS context init failed\n");
    return -1;
  }

  const char *ca_bundle = find_ca_bundle();
  if (ca_bundle &&
      wolfSSL_CTX_load_verify_locations(t->ctx, ca_bundle, NULL) == WOLFSSL_SUCCESS) {
    wolfSSL_CTX_set_verify(t->ctx, WOLFSSL_VERIFY_PEER, NULL);
  } else {
    wolfSSL_CTX_set_verify(t->ctx, WOLFSSL_VERIFY_NONE, NULL);
    if (ca_bundle) {
      fprintf(stderr,
              "curl: warning: failed to load CA bundle '%s'; TLS verification disabled\n",
              ca_bundle);
    } else {
      fprintf(stderr,
              "curl: warning: no CA bundle found; TLS verification disabled\n");
    }
  }

  t->ssl = wolfSSL_new(t->ctx);
  if (!t->ssl) {
    fprintf(stderr, "curl: TLS session init failed\n");
    return -1;
  }

  if (wolfSSL_check_domain_name(t->ssl, url->host) != WOLFSSL_SUCCESS) {
    fprintf(stderr, "curl: TLS hostname verification setup failed\n");
    return -1;
  }

  if (wolfSSL_UseSNI(t->ssl, WOLFSSL_SNI_HOST_NAME, (const unsigned char *)url->host,
                     (unsigned short)strlen(url->host)) != WOLFSSL_SUCCESS) {
    fprintf(stderr, "curl: TLS SNI setup failed\n");
    return -1;
  }

  if (wolfSSL_set_fd(t->ssl, t->fd) != WOLFSSL_SUCCESS) {
    fprintf(stderr, "curl: TLS set_fd failed\n");
    return -1;
  }

  int rc = wolfSSL_connect(t->ssl);
  if (rc != WOLFSSL_SUCCESS) {
    print_wolfssl_error("TLS handshake", t->ssl, rc);
    return -1;
  }

  return 0;
}

static int transport_connect(transport_t *t, const url_t *url) {
  memset(t, 0, sizeof(*t));
  t->fd = -1;
  t->use_tls = url->is_https;

  t->fd = connect_tcp(url->host, url->port);
  if (t->fd < 0)
    return -1;

  if (t->use_tls && transport_enable_tls(t, url) != 0) {
    transport_close(t);
    return -1;
  }

  return 0;
}

int main(int argc, char **argv) {
  url_t url;
  memset(&url, 0, sizeof(url));

  const char *arg_url = NULL;
  for (int i = 1; i < argc; i++) {
    if (strcmp(argv[i], "-i") == 0) {
      url.print_headers = 1;
    } else if (strcmp(argv[i], "-o") == 0) {
      if (i + 1 >= argc) {
        const char *msg = "curl: option -o requires an argument\n";
        write(2, msg, strlen(msg));
        return 2;
      }
      url.output_file = argv[++i];
    } else if (strcmp(argv[i], "-O") == 0) {
      url.remote_name = 1;
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

  if (url.output_file && url.remote_name) {
    const char *msg = "curl: cannot use both -o and -O\n";
    write(2, msg, strlen(msg));
    return 2;
  }

  if (parse_url(arg_url, &url) != 0) {
    usage();
    return 2;
  }

  char file_buf[256];
  if (url.remote_name) {
    char *fname = get_filename_from_path(url.path);
    strncpy(file_buf, fname, sizeof(file_buf) - 1);
    file_buf[sizeof(file_buf) - 1] = 0;
    url.output_file = file_buf;
  }

  transport_t tr;
  if (transport_connect(&tr, &url) != 0) {
    char buf[256];
    snprintf(buf, sizeof(buf), "curl: connect failed: %s\n", strerror(errno));
    write(2, buf, strlen(buf));
    return 1;
  }

  char req[1600];
  snprintf(req, sizeof(req),
           "GET %s HTTP/1.1\r\n"
           "Host: %s\r\n"
           "User-Agent: twilight-curl/0.4\r\n"
           "Connection: close\r\n"
           "\r\n",
           url.path, url.host);

  if (transport_write(&tr, req, strlen(req)) != 0) {
    transport_close(&tr);
    const char *msg = "curl: send failed\n";
    write(2, msg, strlen(msg));
    return 1;
  }

  FILE *fout = NULL;
  int fd_out = 1;
  if (url.output_file) {
    fout = fopen(url.output_file, "wb");
    if (!fout) {
      perror("curl: fopen");
      transport_close(&tr);
      return 1;
    }
    fd_out = fileno(fout);
  }

  char rbuf[2048];
  char header_buf[HEADER_BUF_CAP];
  size_t header_len = 0;
  size_t content_length = 0;
  size_t total_body_received = 0;
  int headers_done = 0;

  for (;;) {
    struct pollfd fds[2];
    fds[0].fd = tr.fd;
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
        if (fout)
          fclose(fout);
        transport_close(&tr);
        write(2, "\nInterrupted\n", 13);
        return 130;
      }
    }

    if (!(fds[0].revents & (POLLIN | POLLHUP)))
      continue;

    ssize_t rn = transport_read(&tr, rbuf, sizeof(rbuf));
    if (rn == 0)
      break;
    if (rn < 0) {
      if (fout)
        fclose(fout);
      transport_close(&tr);
      perror("recv");
      return 1;
    }

    if (!headers_done) {
      size_t need = header_len + (size_t)rn;
      if (need > sizeof(header_buf)) {
        headers_done = 1;
        if (write_all_fd(fd_out, rbuf, (size_t)rn) != 0) {
          if (fout)
            fclose(fout);
          transport_close(&tr);
          return 1;
        }
        total_body_received += (size_t)rn;
        if (url.output_file)
          draw_progress(total_body_received, 0);
        continue;
      }

      memcpy(header_buf + header_len, rbuf, (size_t)rn);
      header_len = need;

      for (size_t i = 0; i + 3 < header_len; i++) {
        if (header_buf[i] == '\r' && header_buf[i + 1] == '\n' &&
            header_buf[i + 2] == '\r' && header_buf[i + 3] == '\n') {
          header_buf[i + 4] = 0;
          content_length = parse_content_length(header_buf);

          if (url.print_headers) {
            if (write_all_fd(1, header_buf, i + 4) != 0) {
              if (fout)
                fclose(fout);
              transport_close(&tr);
              return 1;
            }
          }

          size_t body_chunk_len = header_len - (i + 4);
          if (body_chunk_len > 0) {
            if (write_all_fd(fd_out, header_buf + i + 4, body_chunk_len) != 0) {
              if (fout)
                fclose(fout);
              transport_close(&tr);
              return 1;
            }
            total_body_received += body_chunk_len;
          }

          headers_done = 1;
          break;
        }
      }

      if (headers_done) {
        if (url.output_file)
          draw_progress(total_body_received, content_length);
        if (content_length > 0 && total_body_received >= content_length)
          break;
      }
    } else {
      if (write_all_fd(fd_out, rbuf, (size_t)rn) != 0) {
        if (fout)
          fclose(fout);
        transport_close(&tr);
        return 1;
      }
      total_body_received += (size_t)rn;
      if (url.output_file)
        draw_progress(total_body_received, content_length);
      if (content_length > 0 && total_body_received >= content_length)
        break;
    }
  }

  if (url.output_file) {
    write(2, "\n", 1);
    fclose(fout);
  }

  transport_close(&tr);
  return 0;
}
