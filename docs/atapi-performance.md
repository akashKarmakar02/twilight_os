# ATAPI performance and benchmarking

Twilight's legacy IDE ATAPI path supports two transfer engines:

- BMIDE DMA when the PCI IDE controller, PRDT memory, and the drive's
  `IDENTIFY PACKET DEVICE` response all advertise a safe DMA mode.
- `rep insw` PIO as the compatibility path and permanent fallback after a DMA
  command failure.

Both paths batch up to 2 MiB in one `READ(12)` command. PIO still observes the
16-bit ATAPI byte-count limit per data phase; it does not incorrectly treat
that limit as the size of the whole SCSI command. Reads of at most 128 KiB use
a one-window read-ahead cache to amortize optical seeks and PACKET setup.

## Reproducible QEMU benchmark

Build and install the benchmark, rebuild the image, and boot it:

```sh
make -C userspace/apps/diskbench install
make all
make run-blk-bios
```

Inside Twilight, run a large sequential test and a small-I/O/read-ahead test:

```sh
atapibench
diskbench /dev/cdrom0 8 256
diskbench /dev/cdrom0 8 2
```

Arguments are `device`, total MiB, and I/O KiB. `/dev/cdrom0` is read-only, so
the benchmark never writes or restores media. It reports MiB/s and a sampled
checksum so the read buffer is observably consumed. `atapibench` is the same
program built with safe defaults of `/dev/cdrom0`, 8 MiB, and 256 KiB; use it on
shell builds that do not yet propagate command arguments correctly. Keep the ISO, QEMU version,
CPU configuration, and command line fixed when comparing results; the host page
cache means this measures the guest driver path rather than physical-disc speed.

## Upstream design references

The implementation was independently written in Rust using these upstream
state machines and policies as design references; Linux GPL source was not
copied into Twilight's BSD-3-Clause source:

- Linux `libata-sff.c`: repeated PIO transfers, ATAPI phase validation, and
  starting BMIDE only after the CDB is sent.
  <https://github.com/torvalds/linux/blob/master/drivers/ata/libata-sff.c>
- Linux `libata-scsi.c`: bounded/even transfer hints, ATAPI DMA feature setup,
  and DMA filtering.
  <https://github.com/torvalds/linux/blob/master/drivers/ata/libata-scsi.c>
- FreeBSD `ata-lowlevel.c`: PACKET PIO/DMA sequencing and conservative DMA
  capability quirks.
  <https://github.com/freebsd/freebsd-src/blob/releng/9.3/sys/dev/ata/ata-lowlevel.c>
- FreeBSD `atapi-cd.c`: large block requests, 65,534-byte PIO phase hints,
  retries, and enabling DMA only after mode negotiation.
  <https://github.com/freebsd/freebsd-src/blob/releng/9.3/sys/dev/ata/atapi-cd.c>
- Current FreeBSD `ata_xpt.c`: separate ATAPI DMA policy and negotiated
  transfer-mode setup.
  <https://github.com/freebsd/freebsd-src/blob/main/sys/cam/ata/ata_xpt.c>
