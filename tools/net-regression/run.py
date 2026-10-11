#!/usr/bin/env python3
"""Snapshot QEMU regression for HTTP liveness and Internet socket waits (#83)."""
import argparse
import concurrent.futures
import os
from pathlib import Path
import re
import socket
import statistics
import subprocess
import tempfile
import threading
import time
import tty
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


class Guest:
    def __init__(self, iso, output):
        self.output = output
        self.serial = bytearray()
        self.stop = threading.Event()
        self.fd = None
        self.proc = None
        self.stderr = (output / "qemu.log").open("wb")
        self.transcript = (output / "serial.log").open("wb")
        args = ["qemu-system-x86_64", "-m", "1024", "-smp", "4", "-boot", "d",
                "-accel", "tcg", "-device", "qemu-xhci", "-vga", "std",
                "-cdrom", str(iso), "-display", "none", "-serial", "pty",
                "-monitor", f"unix:{output}/monitor.sock,server=on,wait=off",
                "-no-reboot", "-netdev",
                "user,id=net0,hostfwd=tcp:127.0.0.1:18000-:80,hostfwd=tcp:127.0.0.1:18083-:18083",
                "-device", "rtl8139,netdev=net0"]
        disk = ROOT / "hdd.img"
        if disk.exists():
            args += ["-drive", f"file={disk},if=none,format=raw,id=vd0,snapshot=on",
                     "-device", "virtio-blk-pci,drive=vd0"]
        self.proc = subprocess.Popen(args, stdout=self.stderr, stderr=self.stderr)
        try:
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                match = re.search(rb"char device redirected to (\S+)", (output / "qemu.log").read_bytes())
                if match:
                    self.fd = os.open(match[1].decode(), os.O_RDWR | os.O_NONBLOCK)
                    tty.setraw(self.fd)
                    break
                assert self.proc.poll() is None, "QEMU exited before serial setup"
                time.sleep(.05)
            assert self.fd is not None, "No serial PTY"
            self.reader = threading.Thread(target=self.read, daemon=True)
            self.reader.start()
            self.wait(b"twilight#", timeout=45)
        except BaseException:
            self.close()
            raise

    def read(self):
        while not self.stop.is_set():
            try:
                data = os.read(self.fd, 65536)
                if data:
                    self.serial.extend(data)
                    self.transcript.write(data)
                    self.transcript.flush()
            except BlockingIOError:
                pass
            except OSError:
                return
            time.sleep(.005)

    def wait(self, marker, offset=0, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            data = bytes(self.serial[offset:])
            assert b"NET_FAIL" not in data, data.decode(errors="replace")
            assert b"panicked" not in data, data.decode(errors="replace")
            if (marker.search(data) if hasattr(marker, "search") else marker in data):
                return data
            assert self.proc.poll() is None, "QEMU exited unexpectedly"
            time.sleep(.01)
        raise AssertionError(f"Guest timeout waiting for {marker!r}; see {self.output}/serial.log")

    def command(self, command):
        offset = len(self.serial)
        os.write(self.fd, command.encode() + b"\n")
        return offset

    def clockcheck(self):
        offset = self.command("clockcheck rel 10000000 100")
        data = self.wait(b"DONE", offset)
        assert data.count(b"result=ok") == 100, data.decode(errors="replace")
        return data

    def close(self):
        if self.proc is not None and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.stop.set()
        if hasattr(self, "reader"):
            self.reader.join(timeout=1)
        if self.fd is not None:
            os.close(self.fd)
        self.stderr.close()
        self.transcript.close()


class Echo:
    def __enter__(self):
        self.tcp = socket.socket()
        self.tcp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.tcp.bind(("127.0.0.1", 18081))
        self.tcp.listen()
        self.tcp.settimeout(.1)
        self.udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.udp.bind(("127.0.0.1", 18081))
        self.udp.settimeout(.1)
        self.stop = threading.Event()
        self.pool = concurrent.futures.ThreadPoolExecutor()
        self.futures = [self.pool.submit(self.accept), self.pool.submit(self.datagrams)]
        return self

    def connection(self, conn):
        with conn:
            conn.settimeout(15)
            count = 0
            while count < 4096:
                data = conn.recv(4096 - count)
                if not data:
                    return
                conn.sendall(data)
                count += len(data)
            conn.shutdown(socket.SHUT_WR)

    def accept(self):
        while not self.stop.is_set():
            try:
                conn, _ = self.tcp.accept()
            except socket.timeout:
                continue
            self.futures.append(self.pool.submit(self.connection, conn))

    def datagrams(self):
        while not self.stop.is_set():
            try:
                data, endpoint = self.udp.recvfrom(65536)
            except socket.timeout:
                continue
            self.udp.sendto(data, endpoint)

    def __exit__(self, *args):
        self.stop.set()
        self.pool.shutdown(wait=True)
        self.tcp.close()
        self.udp.close()
        for future in self.futures:
            future.result()


def http_checks(opener, cycles):
    elapsed = []
    for _ in range(cycles):
        for path in ("/", "/style.css", "/app.js", "/img.png"):
            started = time.monotonic()
            with opener.open("http://127.0.0.1:18000" + path, timeout=5) as response:
                body = response.read()
            expected = ROOT / "rootfs/var/www" / ("index.html" if path == "/" else path[1:])
            assert body == expected.read_bytes(), f"Corrupt/truncated HTTP body: {path}"
            elapsed.append(time.monotonic() - started)
    print(f"PASS http requests={len(elapsed)} median_s={statistics.median(elapsed):.6f} max_s={max(elapsed):.6f}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("iso", nargs="?", type=Path, default=ROOT / "twilight-os.iso")
    parser.add_argument("--cycles", type=int, default=40)
    args = parser.parse_args()
    output = Path(tempfile.mkdtemp(prefix="twilight-net-"))
    print(f"Evidence: {output}", flush=True)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with Echo():
        guest = Guest(args.iso.resolve(), output)
        try:
            # No serial/keyboard input after boot and throughout this window.
            time.sleep(2)
            http_checks(opener, args.cycles)
            offset = guest.command("net_io_test")
            data = guest.wait(b"NET_DONE client", offset)
            assert data.count(b"NET_PASS") == 4, data.decode(errors="replace")
            print("PASS guest TCP/UDP, partial I/O, EOF, EAGAIN and EINTR", flush=True)
            guest.wait(b"twilight#", offset)
            offset = guest.command("net_io_test server &")
            guest.wait(b"NET_SERVER_READY", offset)
            with socket.socket() as client:
                client.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 4096)
                client.settimeout(15)
                client.connect(("127.0.0.1", 18083))
                guest.wait(b"NET_ACCEPTED_WITHOUT_DATA", offset)
                # A socket read is blocked, with no request payload. Other tasks
                # must still sleep/wake and run to completion.
                guest.clockcheck()
                client.sendall(b"x")
                time.sleep(.5)  # hold the receive window to force TX backpressure
                assert b"NET_PASS accept_no_payload" not in bytes(guest.serial[offset:]), "Slow reader did not hold the writer"
                guest.clockcheck()
                http_checks(opener, 1)
                count = 0
                while True:
                    data = client.recv(4096)
                    if not data:
                        break  # FIN, rather than reset or retention timeout
                    assert data == bytes((count + i) % 256 for i in range(len(data))), "Corrupt TCP payload"
                    count += len(data)
                    time.sleep(.002)
                assert count == 8 * 1024 * 1024, f"Truncated slow-reader response: {count}"
            guest.wait(b"NET_PASS accept_no_payload_slow_read_graceful_close", offset)
            print("PASS accept without payload, blocking read/write responsiveness, 8 MiB and FIN", flush=True)
            # Let retained TIME-WAIT state and the independent timer driver run.
            time.sleep(2)
            http_checks(opener, 1)
            guest.clockcheck()
            for probe in ("pipe_ipc_test", "unix_bind_test", "fdpass_file_test",
                          "fdpass_pipe_test", "fdpass_socketpair_test", "memfd_basic_test",
                          "memfd_fdpass_test", "memfd_mmap_test", "poll_test"):
                offset = guest.command(probe + "; echo NET_PROBE_EXIT=$?")
                status = re.compile(rb"NET_PROBE_EXIT=(\d+)\r?\n")
                data = guest.wait(status, offset)
                exit_code = int(status.search(data)[1])
                if probe == "fdpass_file_test" and exit_code != 0 and b"fdpass_file_test: open: I/O error" in data:
                    # This existing binary hardcodes /var/fdpass.txt. /var is
                    # read-only in a live-ISO boot; do not relabel it as a pass.
                    print("SKIP fdpass_file_test: live ISO /var is read-only (fixed /var/fdpass.txt fixture)", flush=True)
                else:
                    assert exit_code == 0 and (probe + ": PASS").encode() in data, data.decode(errors="replace")
                    print(f"PASS existing {probe}", flush=True)
                guest.wait(b"twilight#", offset)
            print("PASS network regression", flush=True)
        finally:
            guest.close()


if __name__ == "__main__":
    main()
