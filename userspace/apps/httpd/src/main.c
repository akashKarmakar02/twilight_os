#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

static const char *k_root_default = "/var/www";

static void write_all(int fd, const void *buf, size_t len) {
  const uint8_t *p = (const uint8_t *)buf;
  while (len) {
    ssize_t n = write(fd, p, len);
    if (n < 0) {
      if (errno == EINTR)
        continue;
      if (errno == EAGAIN || errno == EWOULDBLOCK) {
        struct pollfd wfds;
        wfds.fd = fd;
        wfds.events = POLLOUT;
        wfds.revents = 0;
        (void)poll(&wfds, 1, 5000);
        continue;
      }
      break;
    }
    if (n == 0)
      break;
    p += (size_t)n;
    len -= (size_t)n;
  }
}

static void log_err(const char *fmt, ...) {
  char buf[512];
  int pos = 0;

  time_t now = time(NULL);
  if (now != (time_t)-1)
    pos = snprintf(buf, sizeof(buf), "[%lld] ", (long long)now);

  va_list args;
  va_start(args, fmt);
  int n = vsnprintf(buf + pos, sizeof(buf) - pos, fmt, args);
  va_end(args);
  if (n > 0)
    pos += n;
  if (pos >= (int)sizeof(buf))
    pos = (int)sizeof(buf) - 1;
  buf[pos++] = '\n';

  write_all(2, buf, (size_t)pos);
}

static void log_access(int status, const char *method, const char *path) {
  char buf[384];
  int pos = 0;

  time_t now = time(NULL);
  if (now != (time_t)-1)
    pos = snprintf(buf, sizeof(buf), "[%lld] ", (long long)now);

  int n = snprintf(buf + pos, sizeof(buf) - pos, "%d %s %s", status, method, path);
  if (n > 0)
    pos += n;
  if (pos >= (int)sizeof(buf))
    pos = (int)sizeof(buf) - 1;
  buf[pos++] = '\n';

  write_all(1, buf, (size_t)pos);
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
  if (strcmp(dot, ".png") == 0)
    return "image/png";
  if (strcmp(dot, ".jpg") == 0 || strcmp(dot, ".jpeg") == 0)
    return "image/jpeg";
  if (strcmp(dot, ".svg") == 0)
    return "image/svg+xml; charset=utf-8";
  if (strcmp(dot, ".ico") == 0)
    return "image/x-icon";
  if (strcmp(dot, ".txt") == 0)
    return "text/plain; charset=utf-8";
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

static void build_fs_path(char *out, size_t out_cap, const char *k_root, const char *url_path) {
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

// Stream a file to the client. Intentionally omits Content-Length because the
// kernel's file offset/seek semantics can vary; closing the connection is the
// end-of-body signal.
static void send_file(int cfd, int code, const char *reason,
                      const char *fs_path, const char *ctype) {
  int f = open(fs_path, O_RDONLY);
  if (f < 0) {
    return;
  }

  char hdr[512];
  int hn = snprintf(hdr, sizeof(hdr),
                    "HTTP/1.1 %d %s\r\n"
                    "Connection: close\r\n"
                    "Content-Type: %s\r\n"
                    "\r\n",
                    code, reason, ctype);
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

static void try_send_404(int cfd, const char *k_root) {
  char p[256];
  snprintf(p, sizeof(p), "%s/404.html", k_root);
  const char *ctype = "text/html; charset=utf-8";
  int f = open(p, O_RDONLY);
  if (f >= 0) {
    close(f);
    send_file(cfd, 404, "Not Found", p, ctype);
    return;
  }

  send_simple(
      cfd, 404, "Not Found", ctype,
      "<!doctype html><html><body><h1>404 Not Found</h1></body></html>\n");
}

static void handle_client(int cfd, const char *k_root) {
  int status = 200;
  char method[16] = {0};
  char path[1024] = {0};
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
    status = 400;
    goto log;
  }
  *line_end = 0;

  // Parse: METHOD SP PATH SP VERSION
  char version[16] = {0};
  if (sscanf(req, "%15s %1023s %15s", method, path, version) != 3) {
    send_simple(cfd, 400, "Bad Request", "text/plain; charset=utf-8",
                "bad request\n");
    status = 400;
    goto log;
  }

  if (strcmp(method, "GET") != 0) {
    send_simple(cfd, 405, "Method Not Allowed", "text/plain; charset=utf-8",
                "only GET is supported\n");
    status = 405;
    goto log;
  }

  if (!is_safe_path(path)) {
    send_simple(cfd, 404, "Not Found", "text/plain; charset=utf-8",
                "not found\n");
    status = 404;
    goto log;
  }

  char fs_path[1400];
  build_fs_path(fs_path, sizeof(fs_path), k_root, path);
  if (fs_path[0] == 0) {
    try_send_404(cfd, k_root);
    status = 404;
    goto log;
  }

  const char *ctype = content_type_for_path(fs_path);
  if (!ctype) {
    // If no extension, try appending /index.html and retry
    size_t len = strlen(fs_path);
    if (len + 12 < sizeof(fs_path)) {
       strncat(fs_path, "/index.html", sizeof(fs_path) - len - 1);
       ctype = content_type_for_path(fs_path);
    }
    
    if (!ctype) {
        try_send_404(cfd, k_root);
        status = 404;
        goto log;
    }
  }

  int test = open(fs_path, O_RDONLY);
  if (test < 0) {
    try_send_404(cfd, k_root);
    status = 404;
    goto log;
  }
  close(test);

  send_file(cfd, 200, "OK", fs_path, ctype);

log:
  log_access(status, method, path);
}

int main(int argc, char const *argv[]) {
  int s = socket(AF_INET, SOCK_STREAM, 0);
  if (s < 0) {
    log_err("httpd: socket failed");
    return 1;
  }

  // Keep accept non-blocking so each readiness notification can be drained
  // completely before returning to poll.
  int fl = fcntl(s, F_GETFL, 0);
  if (fl >= 0) {
    (void)fcntl(s, F_SETFL, fl | O_NONBLOCK);
  }

  // best-effort reuseaddr; kernel currently treats it as a no-op but returns
  // success.
  int one = 1;
  (void)setsockopt(s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));

  struct sockaddr_in addr;
  memset(&addr, 0, sizeof(addr));
  addr.sin_family = AF_INET;
  addr.sin_port = htons(80);
  addr.sin_addr.s_addr = htonl(0); // 0.0.0.0
  int ret_val =  bind(s, (struct sockaddr *)&addr, sizeof(addr));

  if (ret_val != 0) {
    log_err("httpd: bind: %s", strerror(errno));
    close(s);
    return 1;
  }

  if (listen(s, 16) != 0) {
    log_err("httpd: listen failed");
    close(s);
    return 1;
  }

  const char *root;
  if (argc <= 1) {
    root = k_root_default;
  } else {
    root = argv[1];
  }
  uint16_t port = 80;

  log_err("httpd: serving %s on port %u", root, port);

  for (;;) {
    struct pollfd fds[1];
    fds[0].fd = s;
    fds[0].events = POLLIN;
    fds[0].revents = 0;

    int pr = poll(fds, 1, -1);
    if (pr < 0) {
      if (errno == EINTR)
        continue;
      continue;
    }

    // Drain all pending connections.
    while (fds[0].revents & POLLIN) {
      int cfd = accept(s, NULL, NULL);
      if (cfd < 0) {
        if (errno == EAGAIN || errno == EINTR)
          break;
        break;
      }

      // Ensure client sockets are blocking; the listen socket is O_NONBLOCK so
      // accept can be drained.
      int cfl = fcntl(cfd, F_GETFL, 0);
      if (cfl >= 0 && (cfl & O_NONBLOCK)) {
        (void)fcntl(cfd, F_SETFL, cfl & ~O_NONBLOCK);
      }

      handle_client(cfd, root);
      close(cfd);
    }
  }
}
