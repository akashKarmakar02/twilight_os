# Timezone Data and Setup

Twilight OS configures timezone using Linux-compatible `TZif` files and
`/etc/localtime`.

## Build-time zoneinfo import

`make cpio` now stages `rootfs/` in a temporary directory and runs:

`scripts/sync_zoneinfo.sh <staging-rootfs-dir>`

Behavior:

- If host zoneinfo exists (default `/usr/share/zoneinfo`), it is copied into
  `<staging>/usr/share/zoneinfo`.
- Symlinks are resolved during copy so the staged tree contains regular files.
- You can override host path with:
  `ZONEINFO_HOST_DIR=/custom/path make cpio`
- If host zoneinfo is missing, a UTC-only stub dataset is generated at:
  `<staging>/usr/share/zoneinfo/Etc/UTC`

The source `rootfs/` directory in the repo is not modified by this process.

## User setup flow

During `logind -u <username>`, timezone setup is mandatory after account
creation:

1. Select continent from a numbered, terminal-width-aware multi-column menu.
2. Select location within that continent from a numbered multi-column menu.
3. Selected zone file is copied to `/etc/localtime` atomically.

Selection uses a single global numeric range for each menu (`1..N`) and no
page navigation keys.

Example: `Asia -> Kolkata` installs
`/usr/share/zoneinfo/Asia/Kolkata` as `/etc/localtime`.

## `date` behavior

`/bin/date` uses `tzset()` + `localtime_r()` and displays timezone from
`/etc/localtime`.

If `/etc/localtime` is missing or invalid, `date` falls back to UTC.
