#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#define WAYLAND_SOCKET "/run/user/0/wayland-0"

#define WL_DISPLAY_GET_REGISTRY 1
#define WL_REGISTRY_GLOBAL 0
#define WL_REGISTRY_BIND 0
#define WL_COMPOSITOR_CREATE_SURFACE 0
#define WL_SHM_CREATE_POOL 0
#define WL_SHM_POOL_CREATE_BUFFER 0
#define WL_SURFACE_ATTACH 1
#define WL_SURFACE_DAMAGE 2
#define WL_SURFACE_COMMIT 6
#define WL_BUFFER_RELEASE 0
#define WL_SHM_FORMAT 0

#define WL_SHM_FORMAT_ARGB8888 0
#define WL_SHM_FORMAT_XRGB8888 1

enum {
    REGISTRY_ID = 2,
    COMPOSITOR_ID = 3,
    SHM_ID = 4,
    POOL_ID = 5,
    BUFFER_ID = 6,
    SURFACE_ID = 7,
    WIDTH = 200,
    HEIGHT = 120,
    STRIDE = WIDTH * 4,
};

typedef struct {
    uint32_t name;
    uint32_t version;
    int seen;
} Global;

typedef struct {
    Global compositor;
    Global shm;
    Global seat;
    Global output;
    Global xdg_wm_base;
} Globals;

/**
 * Writes a 32-bit value to a buffer in little-endian order.
 * @param buf Destination buffer.
 * @param offset Current write offset; advanced by 4 bytes.
 * @param value Value to write.
 */
static void put_u32(uint8_t *buf, size_t *offset, uint32_t value) {
    buf[(*offset)++] = (uint8_t)(value & 0xff);
    buf[(*offset)++] = (uint8_t)((value >> 8) & 0xff);
    buf[(*offset)++] = (uint8_t)((value >> 16) & 0xff);
    buf[(*offset)++] = (uint8_t)((value >> 24) & 0xff);
}

/**
 * Reads a 32-bit little-endian value from a byte buffer.
 * @param buf Source buffer.
 * @param offset Byte offset of the first value byte.
 * @return The decoded 32-bit value.
 */
static uint32_t get_u32(const uint8_t *buf, size_t offset) {
    return (uint32_t)buf[offset] |
           ((uint32_t)buf[offset + 1] << 8) |
           ((uint32_t)buf[offset + 2] << 16) |
           ((uint32_t)buf[offset + 3] << 24);
}

/**
 * Writes a NUL-terminated string in Wayland wire format.
 * @param buf Destination buffer.
 * @param offset Byte offset updated after writing.
 * @param value String to write.
 */
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

/**
 * Reads an exact number of bytes from a file descriptor.
 * @param fd File descriptor to read from.
 * @param buf Destination buffer.
 * @param len Number of bytes to read.
 * @return 0 on success, -1 on error.
 */
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

/**
 * Sends a Wayland request or event message.
 * @param fd Connected file descriptor.
 * @param object_id Target object identifier.
 * @param opcode Message opcode.
 * @param payload Message payload bytes.
 * @param payload_len Payload length in bytes.
 * @returns 0 on success, -1 on failure.
 */
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
    return write(fd, message, size) == (ssize_t)size ? 0 : -1;
}

/**
 * Sends a Wayland request and passes a file descriptor with it.
 * @param sock Connected socket used to send the message.
 * @param pass_fd File descriptor to attach to the request.
 * @return 0 on success, -1 on failure.
 */
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

    return sendmsg(sock, &msg, 0) == (ssize_t)size ? 0 : -1;
}

/**
 * Receives a Wayland message and extracts its header and payload.
 * @param fd File descriptor to read from.
 * @param object_id Receives the message object ID.
 * @param opcode Receives the message opcode.
 * @param payload Buffer that receives the message payload.
 * @param payload_cap Size of @p payload in bytes.
 * @param payload_len Receives the payload length.
 * @return 0 on success, or -1 on failure.
 */
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

/**
 * Connects to the Wayland compositor socket.
 * @return A connected socket file descriptor on success, or -1 on failure.
 */
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

/**
 * Requests the Wayland registry and records the globals needed by the client.
 * @param fd Connected Wayland socket.
 * @param globals Storage for the discovered global interfaces.
 * @returns 0 on success, -1 on failure.
 */
static int get_registry(int fd, Globals *globals) {
    uint8_t payload[8];
    size_t offset = 0;
    put_u32(payload, &offset, REGISTRY_ID);
    if (send_message(fd, 1, WL_DISPLAY_GET_REGISTRY, payload, offset) < 0) return -1;

    for (int i = 0; i < 5; ++i) {
        uint32_t object = 0;
        uint16_t opcode = 0;
        uint8_t event[256];
        size_t event_len = 0;
        if (recv_message(fd, &object, &opcode, event, sizeof(event), &event_len) < 0) return -1;
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
        } else if (strcmp(interface, "wl_shm") == 0) {
            globals->shm = (Global){name, version, 1};
        } else if (strcmp(interface, "wl_seat") == 0) {
            globals->seat = (Global){name, version, 1};
        } else if (strcmp(interface, "wl_output") == 0) {
            globals->output = (Global){name, version, 1};
        } else if (strcmp(interface, "xdg_wm_base") == 0) {
            globals->xdg_wm_base = (Global){name, version, 1};
        }
    }

    if (!globals->compositor.seen || !globals->shm.seen ||
        !globals->seat.seen || !globals->output.seen ||
        !globals->xdg_wm_base.seen) {
        errno = ENOENT;
        return -1;
    }
    puts("twland_shm_client: globals received");
    return 0;
}

/**
 * Binds a registry global to a new object ID.
 * @param global Registry global to bind.
 * @param interface Interface name to bind.
 * @param new_id Object ID to assign to the bound interface.
 * @return 0 on success, -1 on failure.
 */
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

/**
 * Reads the supported shared-memory pixel formats.
 * @returns `0` when both `WL_SHM_FORMAT_ARGB8888` and `WL_SHM_FORMAT_XRGB8888` are received, `-1` otherwise.
 */
static int read_shm_formats(int fd) {
    int argb = 0;
    int xrgb = 0;

    for (int i = 0; i < 2; ++i) {
        uint32_t object = 0;
        uint16_t opcode = 0;
        uint8_t payload[32];
        size_t payload_len = 0;
        if (recv_message(fd, &object, &opcode, payload, sizeof(payload), &payload_len) < 0) {
            return -1;
        }
        if (object != SHM_ID || opcode != WL_SHM_FORMAT || payload_len < 4) {
            errno = EPROTO;
            return -1;
        }
        uint32_t format = get_u32(payload, 0);
        if (format == WL_SHM_FORMAT_ARGB8888) argb = 1;
        if (format == WL_SHM_FORMAT_XRGB8888) xrgb = 1;
    }

    if (!argb || !xrgb) {
        errno = ENOENT;
        return -1;
    }
    return 0;
}

/**
 * Fills the pixel buffer with a fixed color pattern.
 * @param pixels Destination buffer for the image pixels.
 */
static void fill_pattern(uint32_t *pixels) {
    for (int y = 0; y < HEIGHT; ++y) {
        for (int x = 0; x < WIDTH; ++x) {
            uint32_t color = 0xffff2020u;
            if (x < 5 || y < 5 || x >= WIDTH - 5 || y >= HEIGHT - 5) {
                color = 0xff20ff40u;
            }
            if (x == (y * WIDTH) / HEIGHT || x == (y * WIDTH) / HEIGHT + 1) {
                color = 0xff2060ffu;
            }
            pixels[y * WIDTH + x] = color;
        }
    }
}

/**
 * Requests creation of a shared-memory pool.
 * @param fd Wayland connection file descriptor.
 * @param memfd File descriptor for the shared-memory backing store.
 * @param size Pool size in bytes.
 * @return 0 on success, or -1 on failure.
 */
static int create_pool(int fd, int memfd, size_t size) {
    uint8_t payload[16];
    size_t offset = 0;
    put_u32(payload, &offset, POOL_ID);
    put_u32(payload, &offset, (uint32_t)size);
    return send_message_with_fd(fd, SHM_ID, WL_SHM_CREATE_POOL, payload, offset, memfd);
}

/**
 * Requests a shared-memory buffer from the pool.
 * @param fd Wayland connection file descriptor.
 */
static int create_buffer(int fd) {
    uint8_t payload[32];
    size_t offset = 0;
    put_u32(payload, &offset, BUFFER_ID);
    put_u32(payload, &offset, 0);
    put_u32(payload, &offset, WIDTH);
    put_u32(payload, &offset, HEIGHT);
    put_u32(payload, &offset, STRIDE);
    put_u32(payload, &offset, WL_SHM_FORMAT_XRGB8888);
    return send_message(fd, POOL_ID, WL_SHM_POOL_CREATE_BUFFER, payload, offset);
}

/**
 * Creates a compositor surface.
 * @param fd Wayland connection socket.
 * @returns 0 on success, -1 on failure.
 */
static int create_surface(int fd) {
    uint8_t payload[8];
    size_t offset = 0;
    put_u32(payload, &offset, SURFACE_ID);
    return send_message(fd, COMPOSITOR_ID, WL_COMPOSITOR_CREATE_SURFACE, payload, offset);
}

/**
 * Attaches the shared buffer to the surface, damages the full frame, and commits it.
 * @param fd Wayland connection socket.
 */
static int attach_damage_commit(int fd) {
    uint8_t payload[32];
    size_t offset = 0;

    offset = 0;
    put_u32(payload, &offset, BUFFER_ID);
    put_u32(payload, &offset, 0);
    put_u32(payload, &offset, 0);
    if (send_message(fd, SURFACE_ID, WL_SURFACE_ATTACH, payload, offset) < 0) return -1;

    offset = 0;
    put_u32(payload, &offset, 0);
    put_u32(payload, &offset, 0);
    put_u32(payload, &offset, WIDTH);
    put_u32(payload, &offset, HEIGHT);
    if (send_message(fd, SURFACE_ID, WL_SURFACE_DAMAGE, payload, offset) < 0) return -1;

    return send_message(fd, SURFACE_ID, WL_SURFACE_COMMIT, NULL, 0);
}

/**
 * Waits for the buffer release event.
 * @return 0 on success, -1 on error.
 */
static int wait_buffer_release(int fd) {
    uint32_t object = 0;
    uint16_t opcode = 0;
    uint8_t payload[32];
    size_t payload_len = 0;
    if (recv_message(fd, &object, &opcode, payload, sizeof(payload), &payload_len) < 0) return -1;
    if (object != BUFFER_ID || opcode != WL_BUFFER_RELEASE) {
        errno = EPROTO;
        return -1;
    }
    return 0;
}

/**
 * Runs the Wayland SHM client demo.
 * @returns Exit code indicating success or the stage at which initialization or rendering failed.
 */
int main(void) {
    int fd = connect_wayland();
    if (fd < 0) {
        perror("twland_shm_client: connect");
        return 1;
    }
    puts("twland_shm_client: connected");

    Globals globals;
    memset(&globals, 0, sizeof(globals));
    if (get_registry(fd, &globals) < 0) {
        perror("twland_shm_client: registry");
        return 2;
    }

    if (bind_global(fd, &globals.compositor, "wl_compositor", COMPOSITOR_ID) < 0 ||
        bind_global(fd, &globals.shm, "wl_shm", SHM_ID) < 0 ||
        read_shm_formats(fd) < 0) {
        perror("twland_shm_client: bind globals");
        return 3;
    }

    size_t size = (size_t)STRIDE * HEIGHT;
    int memfd = memfd_create("twland-shm-client", 0);
    if (memfd < 0 || ftruncate(memfd, (off_t)size) < 0) {
        perror("twland_shm_client: memfd");
        return 4;
    }
    uint32_t *pixels = mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, memfd, 0);
    if (pixels == MAP_FAILED) {
        perror("twland_shm_client: mmap");
        return 5;
    }
    fill_pattern(pixels);

    if (create_pool(fd, memfd, size) < 0) {
        perror("twland_shm_client: create_pool");
        return 6;
    }
    puts("twland_shm_client: shm pool created");

    if (create_buffer(fd) < 0) {
        perror("twland_shm_client: create_buffer");
        return 7;
    }
    puts("twland_shm_client: buffer created");

    if (create_surface(fd) < 0) {
        perror("twland_shm_client: create_surface");
        return 8;
    }
    puts("twland_shm_client: surface created");

    if (attach_damage_commit(fd) < 0) {
        perror("twland_shm_client: commit");
        return 9;
    }
    puts("twland_shm_client: committed buffer");

    if (wait_buffer_release(fd) < 0) {
        perror("twland_shm_client: buffer release");
        return 10;
    }
    puts("twland_shm_client: buffer released");
    puts("twland_shm_client: PASS");

    munmap(pixels, size);
    close(memfd);
    close(fd);
    return 0;
}
