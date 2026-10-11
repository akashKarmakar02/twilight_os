# Network regression (#83)

Build the live ISO with `make`, then run `make test-net`. Python 3, QEMU,
Rust stable (for two small host tests), and the staged `net_io_test` guest app
are required. Ports 127.0.0.1:18000, :18081 (TCP/UDP) and :18083 must be free.
No packages are needed for the Python harness.

Run against an existing image without rebuilding:

```sh
make -o twilight-os.iso test-net
# or just the VM checks:
python3 tools/net-regression/run.py twilight-os.iso
```

The runner uses the run-blk-bios device configuration (RTL8139, xHCI,
virtio-blk, BIOS/TCG, 1 GiB, four vCPUs). If hdd.img exists, it is attached with
`snapshot=on`; the original disk is not written. If absent, no disk is created.
QEMU and all host echo sockets are closed on success or failure. Evidence is
retained in the printed `/tmp/twilight-net-...` directory.

Checks include:

- 160 sequential HTTP requests from an idle boot, with byte-for-byte HTML/CSS/JS/
  image comparison and no serial or keyboard input during that window. This
  exceeds the 128-slot detached close table and exercises TIME-WAIT reuse.
- Blocking and nonblocking TCP connect and a 4096-byte echo, partial buffer I/O
  and EOF readiness; UDP connect after bind, EAGAIN and truncated datagrams;
  interruption of a blocking TCP read by SIGUSR1.
- A listener that accepts a completed handshake without payload, and a read
  waiting for its first byte while clockcheck completes 100 ten-ms sleeps.
- An 8 MiB response to a deliberately stalled/slow reader while another 100
  sleeps and HTTP requests complete, followed by an integrity check and FIN.
- More HTTP and timing checks after teardown, plus the existing pipe, Unix
  socket, fd-passing, memfd and poll probes. The existing fdpass_file_test
  hardcodes /var/fdpass.txt; its open failure is explicitly reported as SKIP on
  a read-only live ISO. Run that probe on an installed writable system.

The C probe builds automatically with the ordinary single-file userspace app
builder. For a targeted build:

```sh
musl-gcc -static -fno-pie -no-pie -Wall -Wextra -Werror \
  userspace/apps/net_io_test/main.c -o rootfs/bin/net_io_test
```

The runner also leaves a controllable Unix HMP monitor while it runs. From
another terminal, use the directory printed by the runner:

```sh
socat - UNIX-CONNECT:/tmp/twilight-net-EXAMPLE/monitor.sock
# HMP: info status
# HMP: sendkey ret
# HMP: screendump /tmp/twilight-screen.ppm
```

The harness sends shell commands through the serial PTY; HMP sendkey is useful
for manual keyboard tests. Do not inject input during the initial HTTP liveness
window, since that masked the original bug. Guest transcripts and host QEMU
errors are in `serial.log` and `qemu.log` in the evidence directory.
