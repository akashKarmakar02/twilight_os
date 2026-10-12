# twinit

`twinit` is Twilight OS's small Unix-style PID 1. It starts configured services, records their PIDs, reaps all children, and applies simple restart policies. Its design is intentionally closer to runit or a small OpenRC supervisor than to systemd.

The kernel reserves PID 0 for its internal bootstrap context and starts `/sbin/twinit` as userspace PID 1. For development, `/bin/twinit` is also installed and the program remains runnable under another PID with a warning. The kernel falls back to `/bin/twinit` and then the legacy `/bin/init` if the preferred path is absent.

At startup, `twinit` creates `/run/twinit`. Runtime sockets and other transient init state belong beneath this directory. As on Linux, binding a pathname Unix socket creates the socket inode itself but does not create missing parent directories; such a bind fails with `ENOENT`.

## Foreground services

Services should stay in the foreground. `twinit` must retain the PID that represents the service so `waitpid()` can report its actual exit status and restart policy can be applied reliably. Classic double-forking daemons lose this direct supervision relationship; a future `type = "forking"` may support them explicitly.

Each service child gets a new session with `setsid()` before `execve()`. Console services inherit the console descriptors, including stdin so an interactive shell remains usable. A service with either output directed to `null` receives `/dev/null` on stdin as well. Output configured as `null` is redirected to `/dev/null` with `dup2()`.

## Service files

Service files live in `/etc/twinit/services/` and use a deliberately small TOML subset. Files must end in `.toml`; names such as `logger.toml.disabled` are ignored.

```toml
name = "shell"
description = "Twilight interactive shell"
exec = "/bin/oksh"
args = []
enabled = true
runlevel = "default"
restart = "always"
type = "foreground"
stdout = "console"
stderr = "console"
```

`name` and the absolute `exec` path are required. `args` is an array of quoted strings. Defaults are: empty description and args, enabled, `default` runlevel, `never` restart, `foreground` type, and console output.

Supported values are:

- `restart`: `never`, `on-failure`, or `always`;
- `type`: `foreground`;
- `runlevel`: `boot`, `default`, `single`, or `shutdown`;
- `stdout` and `stderr`: `console`, `null`, or `log`.

Unsupported top-level fields and invalid supported values invalidate only that
service file; boot continues. The parser is not a full TOML validator.

For `log` output, twinit captures the service's pipe output and forwards lines
to `twilogd`. Services without console output receive `/dev/null` on stdin.
Logging starts first through an explicit bootstrap rule, not a dependency
solver. See [twilogd.md](twilogd.md).

## Packaged development services

`network.toml` is a disabled placeholder for `/sbin/twnetd`, which is not yet
provided. Kernel DHCP and protocol processing do not depend on this file.

`txpc-client.toml` starts the TXPC smoke-test client once at the default
runlevel. Its request for `dev.twilight.test` activates the server configured
in `txpc-echo.toml`, which is otherwise not started at boot. Output goes to the
logging service and `restart = "never"` keeps the client from looping.

The bootstrap broker listens on `/run/twinit/bootstrap.sock`, separately from
the read-only control socket. It handles named registration, on-demand startup,
policy checks, and `SCM_RIGHTS` connection handoff. The small config parser only
constructs TXPC metadata when `[txpc] name` exists; client capabilities alone
in `[txpc.client]` are ignored. The test client therefore has its own unique
name, distinct from the server's name:

```toml
[txpc]
name = "dev.twilight.test-client"

[txpc.client]
capabilities = ["gui"]
```

Server policy can use `allow_all`, `allow_uids`, or required client
capabilities under `[txpc.policy]`. This is a development broker, not a general
dependency graph or a complete service-security model.

## Restart and reaping

PID 1 repeatedly drains `waitpid(-1, ..., WNOHANG)`. Known child PIDs update their service state; unknown adopted children are still reaped and logged. `never` leaves any exit stopped, `on-failure` restarts nonzero or signaled exits, and `always` restarts every exit.

A service is disabled after five restarts in a ten-second window. Running longer than the window resets its restart counter. This is intentionally simple crash-loop protection, not a general rate-limiting framework.

## Runlevels and fallback

The normal active runlevel is `default`. `twinit --single` selects `single` when invoked as PID 1. Runtime switching is not implemented. If the service directory is missing or contains no valid enabled service for the active runlevel, `twinit` supervises a built-in `/bin/oksh` fallback shell with `restart = "always"`.

`twinit --shutdown` and `twinit --reboot` currently provide command hooks only. Under PID 1 they announce that orderly handling is still TODO; under another PID they describe the action that would be requested.

## Test service

`/sbin/test_service [delay_seconds] [exit_status]` prints its PID, sleeps, and exits with the requested status. Temporary service files can use it to exercise `never`, `always`, `on-failure`, and crash-loop behavior.

## Control socket

After creating `/run/twinit`, twinit binds a Unix-domain socket at:

```text
/run/twinit/control.sock
```

The socket is set to nonblocking mode. Each iteration of the supervision loop drains all pending client connections before sleeping. Each client sends one newline-terminated text request and receives one text response before the connection is closed.

Supported read-only commands:

| Command    | Response                                |
|------------|-----------------------------------------|
| `STATUS`   | `OK` followed by all service lines      |
| `LIST`     | Same as `STATUS`                        |
| `SHOW N`   | `OK` followed by one service line       |
| `PING`     | `OK pong`                               |
| `HELP`     | `OK` followed by command list           |

Unknown or mutation commands return `ERR unsupported command`. A missing service returns `ERR service not found`.

If the socket cannot be bound (e.g. AF_UNIX is incomplete), twinit prints a warning and continues booting. Service supervision is never affected by control socket availability.

## Current limitations and future work

There is no dependency solver, runtime runlevel switching, shutdown ordering,
signal-driven control plane, general socket activation, or forking-service
support. TXPC provides named on-demand activation and twilogd provides logging.
Service configuration uses a constrained TOML parser. The control socket
remains read-only; start/stop/restart/enable/disable commands are not implemented.

Future stages may add service mutation commands, dependency ordering, general
socket activation, orderly shutdown, and richer user-session management.
