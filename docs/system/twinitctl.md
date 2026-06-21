# twinitctl

`twinitctl` is the command-line client for Twilight's `twinit` service manager. It parses a user request, encodes one line of the control protocol, sends it to PID 1, and prints PID 1's response.

`twinitctl` does not manage services itself. It only talks to `twinit`. Service configuration, process ownership, runtime state, supervision, and restart policy remain exclusively in PID 1.

## Commands

### Supported (read-only)

```text
twinitctl status              show all services
twinitctl list                list services
twinitctl show NAME           show one service
twinitctl ping                health check
twinitctl help                show help
```

`status` and `list` currently request equivalent service-table information. `ping` confirms the control socket is reachable and returns `OK pong`.

### Reserved (not yet implemented)

```text
twinitctl start NAME
twinitctl stop NAME
twinitctl restart NAME
twinitctl reload NAME
twinitctl enable NAME
twinitctl disable NAME
```

These commands are parsed by the client and sent over the protocol, but `twinit` currently returns `ERR unsupported command` for all of them. Service mutation will be implemented in a future PR.

## Control socket

All requests use the Unix-domain socket:

```text
/run/twinit/control.sock
```

The client writes one newline-terminated request and reads a text response until the server closes its response side. If the socket is absent or unavailable, it prints an error and exits nonzero:

```text
twinitctl: cannot connect to /run/twinit/control.sock: ...
```

## Text protocol

Requests are human-readable command lines:

```text
STATUS
LIST
SHOW shell
PING
HELP
```

Single-action responses begin with `OK` or `ERR`:

```text
OK pong
ERR service not found
ERR unsupported command
```

Service tables are line-based. Each service produces one key=value line:

```text
OK
name=shell state=running pid=5 restart=always runlevel=default enabled=true disabled=false restarts=0 exec=/bin/tsh
name=network state=running pid=7 restart=on-failure runlevel=default enabled=true disabled=false restarts=0 exec=/sbin/netd
```

For a multiline successful response, `twinitctl` omits the initial `OK` line when printing the body. Error responses go to stderr and produce a nonzero exit status.

### Protocol examples

Query all services:

```text
client: STATUS
server:
OK
name=shell state=running pid=5 restart=always runlevel=default enabled=true disabled=false restarts=0 exec=/bin/tsh
```

Query one service:

```text
client: SHOW shell
server:
OK
name=shell state=running pid=5 restart=always runlevel=default enabled=true disabled=false restarts=0 exec=/bin/tsh
```

Health check:

```text
client: PING
server: OK pong
```

Unknown command:

```text
client: FROBNICATE
server: ERR unsupported command
```

Missing service:

```text
client: SHOW nonexistent
server: ERR service not found
```

## Development fallback

Use `--encode` to test command parsing without a running control socket:

```text
twinitctl --encode status        → STATUS
twinitctl --encode show shell    → SHOW shell
twinitctl --encode ping          → PING
```

## Current limitations

The protocol has no authentication, version negotiation, streaming events, or binary/JSON representation. If AF_UNIX is incomplete in the kernel, `twinit` will boot without the control socket and `twinitctl` will report that it cannot connect.
