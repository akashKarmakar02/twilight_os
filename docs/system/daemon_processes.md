# Daemon processes and local IPC

Twilight OS exposes a Linux-compatible process and descriptor model sufficient for the first stage of background services. This is infrastructure only: there is no service manager, logging daemon, or desktop protocol in this stage.

## Process lifecycle

`fork()` creates a child process with a private address space and inherited open file descriptions, descriptor flags, working directory, signal dispositions, process group, and session. The child receives zero and the parent receives the child PID. `execve()` replaces the process image while preserving PID, parentage, process group, session, working directory, and descriptors not marked `FD_CLOEXEC`.

`exit()` and `exit_group()` close descriptors and leave the process leader in the process table as a zombie. Normal exit status is encoded in the Linux wait status high byte; signal termination is encoded in the low seven bits. `wait4()`, and therefore libc `wait()`/`waitpid()`, returns and reaps a matching direct child. It supports exact PIDs, any child (`-1`), children in the caller's process group (`0`), a specified process group (`< -1`), `WNOHANG`, and stopped-child reporting through `WUNTRACED`.

When a parent terminates, its direct children are reparented to PID 1. Running children continue independently, and already-dead children remain zombies for PID 1 to reap. This is the key lifecycle rule that allows a daemon child to outlive the shell or helper that launched it.

`getpid()` and `getppid()` report process identity and current parentage. After adoption, `getppid()` reports PID 1.

## Daemonization

The supported first-stage pattern is:

```c
pid_t pid = fork();
if (pid < 0)
    /* handle error */;
if (pid > 0)
    exit(0);
if (setsid() < 0)
    /* handle error */;

/* background process continues */
```

`setsid()` fails with `EPERM` if the caller is already a process-group leader. Otherwise it creates a session whose ID and initial process-group ID equal the caller PID. A daemon in this new session is outside the launching shell's process group, so group-directed shell signals do not select it. Closing or redirecting inherited terminal descriptors remains the daemon's responsibility.

## Process groups and sessions

Each process stores a process-group ID (`pgid`) and session ID (`sid`). Fork inherits both. Exec preserves both.

- `setsid()` creates a new session and process group.
- `getsid(pid)` returns a process's session ID.
- `getpgid(pid)` and `getpgrp()` return process-group identity.
- `setpgid(pid, pgid)` operates on the caller or a direct child, does not move a process across sessions, does not move a session leader, and only joins an existing group in the same session unless creating a group whose ID is the target PID.
- Group-directed `kill()` selection uses these process-group IDs.

This is enough for a future service manager to place supervised processes into groups. Controlling terminals, foreground terminal process groups, orphaned-process-group signaling, credentials/permission checks between users, and complete shell job control are not implemented yet.

## Pipes and readiness

`pipe()` and `pipe2()` create a unidirectional byte stream with a 4096-byte capacity and `PIPE_BUF` of 4096 bytes. Writes no larger than `PIPE_BUF` are atomic. Larger writes may complete partially. The implementation provides:

- blocking read while the pipe is empty and a writer exists;
- blocking write while insufficient capacity exists;
- `EAGAIN` for operations that would block on an `O_NONBLOCK` endpoint;
- EOF (a zero-length read) after all write-side references close and buffered data is consumed;
- `EPIPE` plus `SIGPIPE` when writing without a reader;
- wakeups when data, capacity, or peer-open state changes.

`pipe2()` accepts `O_NONBLOCK` and `O_CLOEXEC`. Status flags belong to the shared open file description, while `FD_CLOEXEC` is per descriptor.

`poll()`, `ppoll()`, and `select()` use the same readiness state. A read endpoint is readable when data exists, reports hangup when no writer remains, and is considered readable by `select()` on hangup so a subsequent read can observe EOF. A write endpoint is writable when capacity and a reader exist, and reports an error when all readers close. Pipe state changes notify the common poll wait queue.

## Descriptor handling

`dup()`, `dup2()`, `dup3()`, `close()`, and the principal descriptor operations of `fcntl()` are available. Duplicates share an open file description and therefore share file offset and status flags. New descriptors from `dup()`/`dup2()` clear close-on-exec; `dup3(..., O_CLOEXEC)` and `F_DUPFD_CLOEXEC` set it atomically. `F_GETFD`, `F_SETFD`, `F_GETFL`, and `F_SETFL` support daemon redirection and nonblocking event loops. `execve()` closes descriptors marked `FD_CLOEXEC`.

A daemon can therefore open `/dev/null` and use `dup2()` to replace descriptors 0, 1, and 2. Future logging services can replace stdout and stderr with pipe or local-socket endpoints without changing this model.

## Current limitations

- PIDs, process-group IDs, and session IDs are currently 16-bit kernel values.
- `fork()` copies private resident pages rather than using copy-on-write.
- `wait4()` does not yet provide resource usage, full `WCONTINUED` event accounting, `WNOWAIT`, or Linux's complete ptrace/thread selection behavior.
- `setpgid()` does not yet track the Linux rule that a child may not be regrouped after that child has executed a new image.
- `setsid()` tracks session identity but there is no complete controlling-terminal acquire/release implementation.
- PID 1 must continuously reap adopted children; the current minimal init program is not yet a production service manager.
- Poll timeout and signal-mask behavior is still less complete than Linux, especially `ppoll()`'s temporary signal mask and interruption semantics.
- Named Unix-domain sockets exist separately, but this stage standardizes only anonymous pipes as the required local IPC primitive.

## Direction

```text
kernel
  ↓
daemon processes
  ↓
service manager / init
  ↓
logging daemon
  ↓
desktop services
```

The kernel owns process lifecycle, descriptors, scheduling, and IPC readiness. Policy—service definitions, restart behavior, dependency ordering, and logging routes—belongs in userspace.
