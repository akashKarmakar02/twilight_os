# Development Notes

## QEMU logging
Most QEMU run targets write logs to `qemu.log` to aid debugging. Remove the file between runs if you want a clean log.

## Rebuilding the root filesystem
Edit files under `rootfs/`, then run:
```bash
make cpio
```
or rebuild the ISO/HDD image.

## Userspace rebuild
`make userspace` runs the userspace builder, compiles apps, and stages them into `rootfs/bin`.

## Boot assets
The build downloads Limine and OVMF firmware automatically when needed. Remove `limine/` or `ovmf/` and rerun `make` to re-fetch them.

## Cleanups
- `make clean` removes build outputs.
- `make distclean` additionally removes downloaded boot assets.
