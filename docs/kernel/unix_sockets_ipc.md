# Unix sockets IPC

Twilight supports the first Unix IPC primitives needed by service infrastructure
and future desktop transports.

## Supported AF_UNIX operations

- `socket(AF_UNIX, SOCK_STREAM, 0)`
- `socket(AF_UNIX, SOCK_DGRAM, 0)`
- pathname `bind`, `connect`, `listen`, `accept`, `accept4`
- `send`, `recv`, `sendto`, `recvfrom`
- `socketpair(AF_UNIX, SOCK_STREAM, 0, sv)`
- `socketpair(AF_UNIX, SOCK_DGRAM, 0, sv)`
- `sendmsg`, `recvmsg`
- `SCM_RIGHTS` file descriptor passing
- `poll`/`select` readiness for readable, writable, hangup, and error states

Pathname sockets follow Linux-like filesystem semantics: binding a socket below a
missing parent directory fails with `ENOENT`; `bind` creates a socket node at the
requested path.

## `socketpair`

`socketpair` creates two connected local sockets and returns two new file
descriptors. `SOCK_NONBLOCK` sets `O_NONBLOCK` on both open file descriptions,
and `SOCK_CLOEXEC` sets close-on-exec on both descriptors.

For stream sockets, data written to one side is readable from the other as a byte
stream. For datagram sockets, each send is delivered as one queued datagram.

## `sendmsg` and `recvmsg`

The kernel parses the x86_64 Linux/musl `msghdr`, `iovec`, and `cmsghdr`
layouts. Payload data is copied through `msg_iov`. Datagram `msg_name` can name a
Unix peer when sending and can receive the source address when reading.

Unsupported ancillary records are ignored safely. Malformed ancillary lengths
return `EINVAL`.

## `SCM_RIGHTS`

`SOL_SOCKET` + `SCM_RIGHTS` passes file references over Unix sockets.

The sender keeps its original fd. The socket message stores cloned references to
the underlying open file descriptions. The receiver gets fresh fd numbers when
`recvmsg` installs those references into the receiving process.

If `MSG_CMSG_CLOEXEC` is passed to `recvmsg`, received descriptors are installed
with close-on-exec. If the receiver's control buffer is too small, the kernel
sets `MSG_CTRUNC` and drops the pending descriptor references instead of leaking
them.

Plain `read` on a stream socket consumes bytes and discards any attached
ancillary data. Programs that expect fd passing must use `recvmsg`.

## Poll readiness

AF_UNIX sockets report:

- `POLLIN` when data or accepted connections are queued
- `POLLOUT` when send capacity exists
- `POLLHUP` when the peer has closed
- `POLLERR` for broken socket state

This keeps `twinit`, `twilogd`, and future event loops on one readiness model.

## Why Wayland needs this

Wayland compositors and clients communicate over Unix sockets. For serious
buffer transport, clients pass shared-memory file descriptors to the compositor.
That requires `sendmsg`, `recvmsg`, and `SCM_RIGHTS`; without fd passing, a
Wayland transport can exchange text-like protocol bytes but cannot efficiently
share render buffers.

## Current limitations

- Ancillary support is limited to `SCM_RIGHTS`.
- Credentials passing is not implemented.
- Abstract namespace Unix sockets are not implemented.
- Datagram queue sizing is simple and not yet fully Linux-compatible.
- Stream fd passing is associated with Twilight's internal message boundaries.
