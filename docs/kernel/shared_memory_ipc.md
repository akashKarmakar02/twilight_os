# Shared memory IPC foundation

Twilight now has the memory-sharing pieces needed before a serious
Wayland-style transport:

- `memfd_create`
- `ftruncate`
- `mmap(..., MAP_SHARED, memfd, offset)`
- `SCM_RIGHTS` fd passing for memfd descriptors

This does not implement Wayland or a compositor. It only provides the kernel
foundation for sharing buffers between processes.

## `memfd_create`

`memfd_create(const char *name, unsigned int flags)` creates an anonymous
in-memory file and returns a normal file descriptor.

Supported flags:

- `MFD_CLOEXEC`
- `MFD_ALLOW_SEALING` is accepted, but seals are not enforced yet

Unknown flags return `EINVAL`.

The file has no filesystem path. The name is kept only for debugging, using the
form:

```text
memfd:<name>
```

Memfd objects support:

- `read`
- `write`
- `lseek`
- `fstat`
- `ftruncate`
- `mmap`
- descriptor passing through `SCM_RIGHTS`

## `ftruncate`

`ftruncate(fd, length)` grows or shrinks memfd storage.

Growing allocates zero-filled physical pages. Shrinking releases whole pages past
the new logical end. Negative lengths return `EINVAL`.

## `mmap MAP_SHARED`

For memfd descriptors, Twilight supports:

```c
mmap(NULL, size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0)
```

Mappings point at the memfd backing pages directly. Two mappings of the same
memfd therefore observe the same bytes. Writes through one mapping are visible
through the other mapping and through mappings created after passing the memfd fd
to another process.

Offsets must be page-aligned. Mapping beyond the current memfd size returns
`EINVAL`.

`MAP_SHARED` for other regular files remains limited to existing device-specific
providers such as framebuffer mappings.

## `msync`

`msync` is currently a no-op that returns success for page-aligned ranges. Memfd
storage is memory-resident, so no disk flush is required.

## SCM_RIGHTS + memfd workflow

A future compositor/client pair can use the existing Unix socket fd-passing path:

1. Client creates a memfd.
2. Client resizes it with `ftruncate`.
3. Client maps and writes pixels.
4. Client sends the memfd fd over AF_UNIX using `sendmsg` + `SCM_RIGHTS`.
5. Server receives a new fd using `recvmsg`.
6. Server maps the fd with `MAP_SHARED`.
7. Both processes see the same buffer contents.

## Future Wayland path

```text
/run/user/0/wayland-0
AF_UNIX SOCK_STREAM
sendmsg/recvmsg
SCM_RIGHTS
memfd_create
mmap MAP_SHARED
wl_shm buffers
```

## Current limitations

- Memfd seals are accepted but not implemented.
- No POSIX shm namespace yet.
- No hugepage memfd support.
- No compositor or Wayland protocol implementation yet.
- Shared regular-file mmap is not generally implemented.
