#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

static const char *k_root = "/var/www";

static void write_all(int fd, const void *buf, size_t len) {
  const uint8_t *p = (const uint8_t *)buf;
  while (len) {
    ssize_t n = write(fd, p, len);
    if (n < 0) {
      if (errno == EINTR)
        continue;
      break;
    }
    if (n == 0)
      break;
    p += (size_t)n;
    len -= (size_t)n;
  }
}

static void send_simple(int fd, int code, const char *reason,
                        const char *content_type, const char *body) {
  char hdr[512];
  size_t body_len = body ? strlen(body) : 0;
  int n = snprintf(hdr, sizeof(hdr),
                   "HTTP/1.1 %d %s\r\n"
                   "Connection: close\r\n"
                   "Content-Type: %s\r\n"
                   "Content-Length: %zu\r\n"
                   "\r\n",
                   code, reason, content_type, body_len);
  if (n > 0)
    write_all(fd, hdr, (size_t)n);
  if (body_len)
    write_all(fd, body, body_len);
}

static const char *content_type_for_path(const char *path) {
  const char *dot = strrchr(path, '.');
  if (!dot)
    return NULL;
  if (strcmp(dot, ".html") == 0)
    return "text/html; charset=utf-8";
  if (strcmp(dot, ".css") == 0)
    return "text/css; charset=utf-8";
  if (strcmp(dot, ".js") == 0)
    return "application/javascript; charset=utf-8";
  return NULL;
}

static int is_safe_path(const char *url_path) {
  if (!url_path || url_path[0] != '/')
    return 0;
  // keep it simple: reject any attempts at traversal or percent-encoding
  if (strstr(url_path, "..") != NULL)
    return 0;
  if (strchr(url_path, '\\') != NULL)
    return 0;
  if (strchr(url_path, '%') != NULL)
    return 0;
  return 1;
}

static void build_fs_path(char *out, size_t out_cap, const char *url_path) {
  // strip query string
  char path[1024];
  strncpy(path, url_path, sizeof(path) - 1);
  path[sizeof(path) - 1] = 0;
  char *q = strchr(path, '?');
  if (q)
    *q = 0;

  // default document
  if (strcmp(path, "/") == 0 || path[strlen(path) - 1] == '/') {
    if (snprintf(out, out_cap, "%s%sindex.html", k_root, path) < 0) {
      out[0] = 0;
    }
    return;
  }

  if (snprintf(out, out_cap, "%s%s", k_root, path) < 0) {
    out[0] = 0;
  }
}

static void send_file(int cfd, const char *fs_path, const char *ctype) {
  int f = open(fs_path, O_RDONLY);
  if (f < 0) {
    return;
  }
  off_t end = lseek(f, 0, 2);
  if (end < 0) {
    close(f);
    return;
  }
  (void)lseek(f, 0, 0);

  char hdr[512];
  int hn = snprintf(hdr, sizeof(hdr),
                    "HTTP/1.1 200 OK\r\n"
                    "Connection: close\r\n"
                    "Content-Type: %s\r\n"
                    "Content-Length: %ld\r\n"
                    "\r\n",
                    ctype, (long)end);
  if (hn > 0)
    write_all(cfd, hdr, (size_t)hn);

  char buf[2048];
  for (;;) {
    ssize_t n = read(f, buf, sizeof(buf));
    if (n == 0)
      break;
    if (n < 0) {
      if (errno == EINTR)
        continue;
      break;
    }
    write_all(cfd, buf, (size_t)n);
  }
  close(f);
}

static void try_send_404(int cfd) {
  char p[256];
  snprintf(p, sizeof(p), "%s/404.html", k_root);
  const char *ctype = "text/html; charset=utf-8";
  int f = open(p, O_RDONLY);
  if (f >= 0) {
    close(f);
    send_file(cfd, p, ctype);
    return;
  }

  send_simple(cfd, 404, "Not Found", ctype,
              "<!doctype html><html><body><h1>404 Not Found</h1></body></html>\n");
}

static void handle_client(int cfd) {
  char req[4096];
  ssize_t n = read(cfd, req, sizeof(req) - 1);
  if (n <= 0)
    return;
  req[n] = 0;

  char *line_end = strstr(req, "\r\n");
  if (!line_end)
    line_end = strchr(req, '\n');
  if (!line_end) {
    send_simple(cfd, 400, "Bad Request", "text/plain; charset=utf-8",
                "bad request\n");
    return;
  }
  *line_end = 0;

  // Parse: METHOD SP PATH SP VERSION
  char method[16], path[1024], version[16];
  method[0] = path[0] = version[0] = 0;
  if (sscanf(req, "%15s %1023s %15s", method, path, version) != 3) {
    send_simple(cfd, 400, "Bad Request", "text/plain; charset=utf-8",
                "bad request\n");
    return;
  }

  if (strcmp(method, "GET") != 0) {
    send_simple(cfd, 405, "Method Not Allowed", "text/plain; charset=utf-8",
                "only GET is supported\n");
    return;
  }

  if (!is_safe_path(path)) {
    send_simple(cfd, 404, "Not Found", "text/plain; charset=utf-8",
                "not found\n");
    return;
  }

  char fs_path[1400];
  build_fs_path(fs_path, sizeof(fs_path), path);
  if (fs_path[0] == 0) {
    try_send_404(cfd);
    return;
  }

  const char *ctype = content_type_for_path(fs_path);
  if (!ctype) {
    try_send_404(cfd);
    return;
  }

  int test = open(fs_path, O_RDONLY);
  if (test < 0) {
    try_send_404(cfd);
    return;
  }
  close(test);

  send_file(cfd, fs_path, ctype);
}

int main(void) {
  int s = socket(AF_INET, SOCK_STREAM, 0);
  if (s < 0) {
    const char *msg = "httpd: socket failed\n";
    write_all(2, msg, strlen(msg));
    return 1;
  }

  // Make accept non-blocking so we can exit on Ctrl+C without signals.
  int fl = fcntl(s, F_GETFL, 0);
  if (fl >= 0) {
    (void)fcntl(s, F_SETFL, fl | O_NONBLOCK);
  }

  // best-effort reuseaddr; kernel currently treats it as a no-op but returns success.
  int one = 1;
  (void)setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));

  struct sockaddr_in addr;
  memset(&addr, 0, sizeof(addr));
  addr.sin_family = AF_INET;
  addr.sin_port = htons(80);
  addr.sin_addr.s_addr = htonl(0); // 0.0.0.0

  if (bind(s, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
    char buf[128];
    snprintf(buf, sizeof(buf), "httpd: bind failed: %s\n", strerror(errno));
    write_all(2, buf, strlen(buf));
    close(s);
    return 1;
  }

  if (listen(s, 16) != 0) {
    const char *msg = "httpd: listen failed\n";
    write_all(2, msg, strlen(msg));
    close(s);
    return 1;
  }

  const char *ready = "httpd: serving /var/www on port 80\n";
  write_all(1, ready, strlen(ready));

  for (;;) {
    struct pollfd fds[2];
    fds[0].fd = 0;
    fds[0].events = POLLIN;
    fds[0].revents = 0;
    fds[1].fd = s;
    fds[1].events = POLLIN;
    fds[1].revents = 0;

    int pr = poll(fds, 2, -1);
    if (pr < 0) {
      if (errno == EINTR)
        continue;
      continue;
    }

    if (fds[0].revents & POLLIN) {
      char c = 0;
      ssize_t rn = read(0, &c, 1);
      if (rn == 1 && (unsigned char)c == 0x03) {
        const char *bye = "httpd: exiting\n";
        write_all(1, bye, strlen(bye));
        close(s);
        return 0;
      }
    }

    // Drain all pending connections.
    for (;;) {
      int cfd = accept(s, NULL, NULL);
      if (cfd < 0) {
        if (errno == EAGAIN || errno == EINTR)
          break;
        break;
      }
      handle_client(cfd);
      close(cfd);
    }
  }
}
