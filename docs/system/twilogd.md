# Twilight logging service

`twilogd` is Twilight OS's first userspace logging daemon. It is deliberately
small: services send text datagrams, and the daemon appends one readable line
per datagram. It is not a binary journal and does not replace the kernel log.

## Runtime interface

`twilogd` runs in the foreground under `twinit` and listens on the Unix-domain
datagram socket:

```text
/run/twilight/log.sock
```

It first attempts to append logs to `/var/log/twilight.log`. If that location
cannot be created or opened, it uses `/run/twilight/twilight.log`.

At boot the daemon reports:

```text
twilogd: starting
twilogd: listening on /run/twilight/log.sock
twilogd: writing to /var/log/twilight.log
```

The fallback path is printed instead when `/var/log` is unavailable.

## Text protocol

One datagram is one log entry. A sender may send an unstructured message:

```text
hello from service
```

or the supported fields:

```text
LEVEL=INFO SOURCE=httpd MESSAGE=request GET /
```

Missing levels and sources default to `INFO` and `unknown`. Entries use a
monotonic sequence number until a reliable wall clock is available:

```text
[000001] level=INFO source=httpd message=request GET /
```

Embedded newlines are flattened so a datagram cannot create multiple records.

## twilogctl

The command-line client does not manage the daemon. It sends datagrams or reads
the current text log:

```sh
twilogctl help
twilogctl send hello from shell
twilogctl status
twilogctl show
twilogctl tail
```

`tail` currently prints the whole file, just like `show`.

## Service output forwarding

`twinit` accepts `stdout = "log"` and `stderr = "log"`. It connects the selected
stream to a pipe, reads that pipe without blocking its child-reaping loop, and
forwards complete lines to `twilogd`. Standard output uses `LEVEL=INFO`; standard
error uses `LEVEL=ERROR`. The service name becomes `SOURCE`. If the datagram
cannot be delivered, `twinit` prints the line to the console as a fallback.

The logging service itself remains attached to the console:

```toml
name = "twilogd"
description = "Twilight logging daemon"
exec = "/sbin/twilogd"
args = []
enabled = true
runlevel = "default"
restart = "always"
type = "foreground"
stdout = "console"
stderr = "console"
```

The HTTP server is the first real logged service:

```toml
name = "httpd"
description = "Twilight HTTP server"
exec = "/bin/httpd"
args = []
enabled = true
runlevel = "default"
restart = "on-failure"
type = "foreground"
stdout = "log"
stderr = "log"
```

## Verification

After boot, the service table should include both daemons:

```text
name=twilogd state=running ...
name=httpd state=running ...
```

Send and inspect a direct entry:

```sh
twilogctl send hello from shell
twilogctl show
```

The log should contain `source=twilogctl message=hello from shell`. To exercise
service forwarding, make an HTTP request and inspect the log again:

```sh
curl http://10.0.2.15/
twilogctl show
```

`httpd` writes startup failures to stderr and access records such as
`200 GET /` to stdout, so they appear with `source=httpd`.

## Current limitations

- No log rotation.
- No binary journal.
- No kernel log bridge.
- No per-service log files.
- No logging permission model.
- No structured fields beyond `LEVEL`, `SOURCE`, and `MESSAGE`.
