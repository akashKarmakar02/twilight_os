/*
 * twland_xdg_client - a minimal native Wayland client that creates a real
 * xdg-shell toplevel window and draws into it with wl_shm.
 *
 * This speaks the raw Wayland wire protocol (no libwayland-client) so it runs
 * under Twilight's twland compositor without any extra libraries.  It mirrors
 * the wire codec of twland_shm_client and adds the xdg-shell role handshake:
 *
 *   connect -> get_registry -> bind compositor/shm/xdg_wm_base
 *     -> create_surface -> get_xdg_surface -> get_toplevel -> commit
 *     -> wait for xdg_surface.configure -> ack_configure
 *     -> attach drawn shm buffer -> commit -> run until closed
 *
 * Build: see Makefile (static musl, no-pie).
 */

#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#define WAYLAND_SOCKET "/run/user/0/wayland-0"

/* --- Wayland core opcodes (client -> compositor) --- */
#define WL_DISPLAY_GET_REGISTRY 1
#define WL_REGISTRY_BIND 0
#define WL_COMPOSITOR_CREATE_SURFACE 0
#define WL_SHM_CREATE_POOL 0
#define WL_SHM_POOL_CREATE_BUFFER 0
#define WL_SURFACE_ATTACH 1
#define WL_SURFACE_DAMAGE 2
#define WL_SURFACE_COMMIT 6

/* --- xdg-shell opcodes (client -> compositor) --- */
#define XDG_WM_BASE_PONG 3
#define XDG_WM_BASE_GET_XDG_SURFACE 2
#define XDG_SURFACE_DESTROY 0
#define XDG_SURFACE_GET_TOPLEVEL 1
#define XDG_SURFACE_ACK_CONFIGURE 4
#define XDG_TOPLEVEL_SET_TITLE 2

/* --- Event opcodes (compositor -> client) --- */
#define WL_REGISTRY_GLOBAL 0
#define WL_SHM_FORMAT 0
#define WL_BUFFER_RELEASE 0
#define XDG_WM_BASE_PING 0
#define XDG_SURFACE_CONFIGURE 0
#define XDG_TOPLEVEL_CONFIGURE 0
#define XDG_TOPLEVEL_CLOSE 1

#define WL_SHM_FORMAT_XRGB8888 1

/* Object id allocation. 1 = wl_display, 2 = wl_registry; client objects below. */
enum {
    REGISTRY_ID = 2,
    COMPOSITOR_ID = 3,
    SHM_ID = 4,
    XDG_WM_BASE_ID = 5,
    SURFACE_ID = 6,
    POOL_ID = 7,
    BUFFER_ID = 8,
    XDG_SURFACE_ID = 9,
    TOPLEVEL_ID = 10,
};

#define WIDTH 320
#define HEIGHT 200
#define STRIDE (WIDTH * 4)

typedef struct {
    uint32_t name;
    uint32_t version;
    int seen;
} Global;

typedef struct {
    Global compositor;
    Global shm;
    Global xdg_wm_base;
} Globals;

/* --- wire codec --------------------------------------------------------- */

static void put_u32(uint8_t *buf, size_t *offset, uint32_t value) {
    buf[(*offset)++] = (uint8_t)(value & 0xff);
    buf[(*offset)++] = (uint8_t)((value >> 8) & 0xff);
    buf[(*offset)++] = (uint8_t)((value >> 16) & 0xff);
    buf[(*offset)++] = (uint8_t)((value >> 24) & 0xff);
}

static uint32_t get_u32(const uint8_t *buf, size_t offset) {
    return (uint32_t)buf[offset] |
           ((uint32_t)buf[offset + 1] << 8) |
           ((uint32_t)buf[offset + 2] << 16) |
           ((uint32_t)buf[offset + 3] << 24);
}

static void put_string(uint8_t *buf, size_t *offset, const char *value) {
    size_t len = strlen(value) + 1;
    put_u32(buf, offset, (uint32_t)len);
    memcpy(buf + *offset, value, len - 1);
    *offset += len - 1;
    buf[(*offset)++] = 0;
    while ((*offset) & 3) {
        buf[(*offset)++] = 0;
    }
}

static int read_exact(int fd, void *buf, size_t len) {
    uint8_t *out = buf;
    size_t done = 0;
    while (done < len) {
        ssize_t n = read(fd, out + done, len - done);
        if (n == 0) {
            errno = ECONNRESET;
            return -1;
        }
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        done += (size_t)n;
    }
    return 0;
}

/* Loop over short writes and retry on EINTR, mirroring read_exact.  A partial
 * send would leave a truncated Wayland message on the wire and desynchronize
 * framing for every subsequent request on this connection. */
static int write_exact(int fd, const void *buf, size_t len) {
    const uint8_t *out = buf;
    size_t done = 0;
    while (done < len) {
        ssize_t n = write(fd, out + done, len - done);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        done += (size_t)n;
    }
    return 0;
}

static int send_message(int fd, uint32_t object_id, uint16_t opcode,
                        const uint8_t *payload, size_t payload_len) {
    uint8_t message[256];
    size_t size = 8 + payload_len;
    if (size > sizeof(message) || size > UINT16_MAX) {
        errno = EMSGSIZE;
        return -1;
    }

    size_t offset = 0;
    put_u32(message, &offset, object_id);
    put_u32(message, &offset, ((uint32_t)size << 16) | opcode);
    if (payload_len) {
        memcpy(message + offset, payload, payload_len);
    }
    return write_exact(fd, message, size);
}

/* Send a request carrying one file descriptor via SCM_RIGHTS.
 *
 * The fd attaches to the first byte of the message and is consumed by the
 * kernel on the first successful sendmsg.  If that sendmsg accepts only part
 * of the message, the remaining bytes are flushed with plain write_exact (no
 * control message) — the fd has already been delivered. */
static int send_message_with_fd(int sock, uint32_t object_id, uint16_t opcode,
                                const uint8_t *payload, size_t payload_len,
                                int pass_fd) {
    uint8_t message[256];
    size_t size = 8 + payload_len;
    if (size > sizeof(message) || size > UINT16_MAX) {
        errno = EMSGSIZE;
        return -1;
    }

    size_t offset = 0;
    put_u32(message, &offset, object_id);
    put_u32(message, &offset, ((uint32_t)size << 16) | opcode);
    if (payload_len) {
        memcpy(message + offset, payload, payload_len);
    }

    struct iovec iov = {.iov_base = message, .iov_len = size};
    char control[CMSG_SPACE(sizeof(int))];
    memset(control, 0, sizeof(control));

    struct msghdr msg;
    memset(&msg, 0, sizeof(msg));
    msg.msg_iov = &iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control;
    msg.msg_controllen = sizeof(control);

    struct cmsghdr *cmsg = CMSG_FIRSTHDR(&msg);
    cmsg->cmsg_len = CMSG_LEN(sizeof(int));
    cmsg->cmsg_level = SOL_SOCKET;
    cmsg->cmsg_type = SCM_RIGHTS;
    memcpy(CMSG_DATA(cmsg), &pass_fd, sizeof(pass_fd));

    for (;;) {
        ssize_t sent = sendmsg(sock, &msg, 0);
        if (sent < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if ((size_t)sent == size) {
            return 0;
        }
        /* Partial send: the fd was delivered with the first byte.  Flush the
         * rest as plain bytes — no control message. */
        return write_exact(sock, message + sent, size - (size_t)sent);
    }
}

/* Read one message: header + payload.  Returns 0 on success, -1 on error/EOF. */
static int recv_message(int fd, uint32_t *object_id, uint16_t *opcode,
                        uint8_t *payload, size_t payload_cap,
                        size_t *payload_len) {
    uint8_t header[8];
    if (read_exact(fd, header, sizeof(header)) < 0) return -1;

    *object_id = get_u32(header, 0);
    uint32_t packed = get_u32(header, 4);
    *opcode = (uint16_t)(packed & 0xffff);
    uint16_t size = (uint16_t)(packed >> 16);
    if (size < 8 || (size_t)(size - 8) > payload_cap) {
        errno = EMSGSIZE;
        return -1;
    }

    *payload_len = size - 8;
    return read_exact(fd, payload, *payload_len);
}

/* --- connection + registry ---------------------------------------------- */

static int connect_wayland(void) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) return -1;

    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    strncpy(addr.sun_path, WAYLAND_SOCKET, sizeof(addr.sun_path) - 1);
    if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static int get_registry(int fd, Globals *globals) {
    uint8_t payload[8];
    size_t offset = 0;
    put_u32(payload, &offset, REGISTRY_ID);
    if (send_message(fd, 1, WL_DISPLAY_GET_REGISTRY, payload, offset) < 0) return -1;

    /* twland advertises the globals we need plus a couple we ignore
     * (wl_seat, wl_output).  Bound the loop so a compositor that never
     * advertises a required global produces an error instead of hanging. */
    int remaining = 3;
    int max_events = 16;
    while (remaining > 0) {
        if (max_events-- == 0) {
            errno = EPROTO;
            return -1;
        }
        uint32_t object = 0;
        uint16_t opcode = 0;
        uint8_t event[256];
        size_t event_len = 0;
        if (recv_message(fd, &object, &opcode, event, sizeof(event), &event_len) < 0)
            return -1;
        if (object != REGISTRY_ID || opcode != WL_REGISTRY_GLOBAL || event_len < 12) {
            errno = EPROTO;
            return -1;
        }

        uint32_t name = get_u32(event, 0);
        uint32_t len = get_u32(event, 4);
        if (len == 0 || 8 + len > event_len) {
            errno = EPROTO;
            return -1;
        }
        const char *interface = (const char *)(event + 8);
        size_t version_offset = (8 + len + 3) & ~(size_t)3;
        if (version_offset + 4 > event_len) {
            errno = EPROTO;
            return -1;
        }
        uint32_t version = get_u32(event, version_offset);

        if (strcmp(interface, "wl_compositor") == 0) {
            globals->compositor = (Global){name, version, 1};
            remaining--;
        } else if (strcmp(interface, "wl_shm") == 0) {
            globals->shm = (Global){name, version, 1};
            remaining--;
        } else if (strcmp(interface, "xdg_wm_base") == 0) {
            globals->xdg_wm_base = (Global){name, version, 1};
            remaining--;
        }
    }

    puts("twland_xdg_client: globals received");
    return 0;
}

static int bind_global(int fd, const Global *global, const char *interface,
                       uint32_t new_id) {
    uint8_t payload[128];
    size_t offset = 0;
    put_u32(payload, &offset, global->name);
    put_string(payload, &offset, interface);
    put_u32(payload, &offset, global->version);
    put_u32(payload, &offset, new_id);
    return send_message(fd, REGISTRY_ID, WL_REGISTRY_BIND, payload, offset);
}

/* Drain the wl_shm.format events twland sends on bind. */
static int read_shm_formats(int fd) {
    for (int i = 0; i < 2; ++i) {
        uint32_t object = 0;
        uint16_t opcode = 0;
        uint8_t payload[32];
        size_t payload_len = 0;
        if (recv_message(fd, &object, &opcode, payload, sizeof(payload), &payload_len) < 0)
            return -1;
        if (object != SHM_ID || opcode != WL_SHM_FORMAT || payload_len < 4) {
            errno = EPROTO;
            return -1;
        }
    }
    return 0;
}

/* --- shm buffer --------------------------------------------------------- */

static void fill_pattern(uint32_t *pixels) {
    for (int y = 0; y < HEIGHT; ++y) {
        for (int x = 0; x < WIDTH; ++x) {
            uint32_t color = 0xff3366ccu; /* soft blue */
            if (x < 6 || y < 6 || x >= WIDTH - 6 || y >= HEIGHT - 6) {
                color = 0xffcc6633u; /* orange border */
            }
            /* a diagonal accent so the window is unmistakably rendered */
            if (x == (y * WIDTH) / HEIGHT || x == (y * WIDTH) / HEIGHT + 1) {
                color = 0xffffffffu;
            }
            pixels[y * WIDTH + x] = color;
        }
    }
}

static int create_pool(int fd, int memfd, size_t size) {
    uint8_t payload[16];
    size_t offset = 0;
    put_u32(payload, &offset, POOL_ID);
    put_u32(payload, &offset, (uint32_t)size);
    return send_message_with_fd(fd, SHM_ID, WL_SHM_CREATE_POOL, payload, offset, memfd);
}

static int create_buffer(int fd) {
    uint8_t payload[32];
    size_t offset = 0;
    put_u32(payload, &offset, BUFFER_ID);
    put_u32(payload, &offset, 0); /* offset */
    put_u32(payload, &offset, WIDTH);
    put_u32(payload, &offset, HEIGHT);
    put_u32(payload, &offset, STRIDE);
    put_u32(payload, &offset, WL_SHM_FORMAT_XRGB8888);
    return send_message(fd, POOL_ID, WL_SHM_POOL_CREATE_BUFFER, payload, offset);
}

/* --- xdg-shell handshake ----------------------------------------------- */

static int create_surface(int fd) {
    uint8_t payload[8];
    size_t offset = 0;
    put_u32(payload, &offset, SURFACE_ID);
    return send_message(fd, COMPOSITOR_ID, WL_COMPOSITOR_CREATE_SURFACE, payload, offset);
}

static int get_xdg_surface(int fd) {
    uint8_t payload[16];
    size_t offset = 0;
    put_u32(payload, &offset, XDG_SURFACE_ID);
    put_u32(payload, &offset, SURFACE_ID);
    return send_message(fd, XDG_WM_BASE_ID, XDG_WM_BASE_GET_XDG_SURFACE, payload, offset);
}

static int get_toplevel(int fd) {
    uint8_t payload[8];
    size_t offset = 0;
    put_u32(payload, &offset, TOPLEVEL_ID);
    return send_message(fd, XDG_SURFACE_ID, XDG_SURFACE_GET_TOPLEVEL, payload, offset);
}

static int set_title(int fd, const char *title) {
    uint8_t payload[128];
    size_t offset = 0;
    put_string(payload, &offset, title);
    return send_message(fd, TOPLEVEL_ID, XDG_TOPLEVEL_SET_TITLE, payload, offset);
}

static int ack_configure(int fd, uint32_t serial) {
    uint8_t payload[8];
    size_t offset = 0;
    put_u32(payload, &offset, serial);
    return send_message(fd, XDG_SURFACE_ID, XDG_SURFACE_ACK_CONFIGURE, payload, offset);
}

static int pong(int fd, uint32_t serial) {
    uint8_t payload[8];
    size_t offset = 0;
    put_u32(payload, &offset, serial);
    return send_message(fd, XDG_WM_BASE_ID, XDG_WM_BASE_PONG, payload, offset);
}

static int attach_damage_commit(int fd) {
    uint8_t payload[32];

    size_t offset = 0;
    put_u32(payload, &offset, BUFFER_ID);
    put_u32(payload, &offset, 0); /* x */
    put_u32(payload, &offset, 0); /* y */
    if (send_message(fd, SURFACE_ID, WL_SURFACE_ATTACH, payload, offset) < 0) return -1;

    offset = 0;
    put_u32(payload, &offset, 0); /* x */
    put_u32(payload, &offset, 0); /* y */
    put_u32(payload, &offset, WIDTH);
    put_u32(payload, &offset, HEIGHT);
    if (send_message(fd, SURFACE_ID, WL_SURFACE_DAMAGE, payload, offset) < 0) return -1;

    return send_message(fd, SURFACE_ID, WL_SURFACE_COMMIT, NULL, 0);
}

/* --- event loop --------------------------------------------------------- */

/*
 * Wait for the initial xdg_surface.configure, ack it, then attach the buffer
 * and commit.  A toplevel is not mapped until the configure is acked.
 */
static int wait_for_configure_and_map(int fd) {
    int configured = 0;
    while (!configured) {
        uint32_t object = 0;
        uint16_t opcode = 0;
        uint8_t payload[256];
        size_t payload_len = 0;
        if (recv_message(fd, &object, &opcode, payload, sizeof(payload), &payload_len) < 0)
            return -1;

        if (object == XDG_WM_BASE_ID && opcode == XDG_WM_BASE_PING) {
            if (payload_len < 4) { errno = EPROTO; return -1; }
            if (pong(fd, get_u32(payload, 0)) < 0) return -1;
            continue;
        }
        if (object == TOPLEVEL_ID && opcode == XDG_TOPLEVEL_CONFIGURE) {
            /* width, height, states-array — we accept whatever twland sends. */
            continue;
        }
        if (object == XDG_SURFACE_ID && opcode == XDG_SURFACE_CONFIGURE) {
            if (payload_len < 4) { errno = EPROTO; return -1; }
            uint32_t serial = get_u32(payload, 0);
            if (ack_configure(fd, serial) < 0) return -1;
            configured = 1;
            continue;
        }
        /* Ignore other events (e.g. wl_buffer.release) during the handshake. */
    }

    puts("twland_xdg_client: configured, attaching buffer");
    return attach_damage_commit(fd);
}

/*
 * After mapping, keep the connection alive so the window stays on screen.
 * Respond to pings, exit on close.  This blocks until the compositor closes
 * the toplevel or the socket disconnects.
 */
static int run_until_close(int fd) {
    for (;;) {
        struct pollfd pfd = {.fd = fd, .events = POLLIN};
        int pr = poll(&pfd, 1, -1);
        if (pr < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (!(pfd.revents & POLLIN)) {
            continue;
        }

        uint32_t object = 0;
        uint16_t opcode = 0;
        uint8_t payload[256];
        size_t payload_len = 0;
        if (recv_message(fd, &object, &opcode, payload, sizeof(payload), &payload_len) < 0)
            return -1;

        if (object == XDG_WM_BASE_ID && opcode == XDG_WM_BASE_PING) {
            if (payload_len < 4) { errno = EPROTO; return -1; }
            if (pong(fd, get_u32(payload, 0)) < 0) return -1;
            continue;
        }
        if (object == TOPLEVEL_ID && opcode == XDG_TOPLEVEL_CLOSE) {
            puts("twland_xdg_client: close requested");
            return 0;
        }
        if (object == BUFFER_ID && opcode == WL_BUFFER_RELEASE) {
            /* Buffer released by the compositor; safe to reuse. */
            continue;
        }
        if (object == XDG_SURFACE_ID && opcode == XDG_SURFACE_CONFIGURE) {
            /* Ack every configure, not just the first — the compositor sends
             * these on resize/maximize/focus changes and requires an ack. */
            if (payload_len < 4) { errno = EPROTO; return -1; }
            if (ack_configure(fd, get_u32(payload, 0)) < 0) return -1;
            continue;
        }
        /* Ignore other events. */
    }
}

/* --- main --------------------------------------------------------------- */

int main(void) {
    int fd = connect_wayland();
    if (fd < 0) {
        perror("twland_xdg_client: connect");
        return 1;
    }
    puts("twland_xdg_client: connected");

    Globals globals;
    memset(&globals, 0, sizeof(globals));
    if (get_registry(fd, &globals) < 0) {
        perror("twland_xdg_client: registry");
        return 2;
    }

    if (bind_global(fd, &globals.compositor, "wl_compositor", COMPOSITOR_ID) < 0 ||
        bind_global(fd, &globals.shm, "wl_shm", SHM_ID) < 0 ||
        bind_global(fd, &globals.xdg_wm_base, "xdg_wm_base", XDG_WM_BASE_ID) < 0 ||
        read_shm_formats(fd) < 0) {
        perror("twland_xdg_client: bind globals");
        return 3;
    }

    /* shm pool + buffer with a drawn pattern. */
    size_t size = (size_t)STRIDE * HEIGHT;
    int memfd = memfd_create("twland-xdg-client", 0);
    if (memfd < 0 || ftruncate(memfd, (off_t)size) < 0) {
        perror("twland_xdg_client: memfd");
        return 4;
    }
    uint32_t *pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, memfd, 0);
    if (pixels == MAP_FAILED) {
        perror("twland_xdg_client: mmap");
        return 5;
    }
    fill_pattern(pixels);

    if (create_pool(fd, memfd, size) < 0) {
        perror("twland_xdg_client: create_pool");
        return 6;
    }
    if (create_buffer(fd) < 0) {
        perror("twland_xdg_client: create_buffer");
        return 7;
    }

    /* xdg-shell role handshake. */
    if (create_surface(fd) < 0) {
        perror("twland_xdg_client: create_surface");
        return 8;
    }
    if (get_xdg_surface(fd) < 0) {
        perror("twland_xdg_client: get_xdg_surface");
        return 9;
    }
    if (get_toplevel(fd) < 0) {
        perror("twland_xdg_client: get_toplevel");
        return 10;
    }
    if (set_title(fd, "twland xdg window") < 0) {
        perror("twland_xdg_client: set_title");
        return 11;
    }

    /*
     * First commit is empty: it triggers the configure.  Then we wait for
     * xdg_surface.configure, ack it, and attach+commit the buffer to map.
     */
    if (send_message(fd, SURFACE_ID, WL_SURFACE_COMMIT, NULL, 0) < 0) {
        perror("twland_xdg_client: initial commit");
        return 12;
    }

    if (wait_for_configure_and_map(fd) < 0) {
        perror("twland_xdg_client: configure");
        return 13;
    }
    puts("twland_xdg_client: window mapped");

    int rc = run_until_close(fd);
    if (rc < 0) {
        perror("twland_xdg_client: event loop");
    }

    munmap(pixels, size);
    close(memfd);
    close(fd);
    puts("twland_xdg_client: done");
    return rc < 0 ? 14 : 0;
}
