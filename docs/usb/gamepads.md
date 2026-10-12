# USB HID gamepads

**Development status:** This page describes work on the local
`feat/smolnes-frame-pacing-72` branch, including commit `6ea2539`.
The gamepad driver, shared HID/joydev crates, and `/dev/input/js0` ABI described
below are not present on current `master`. The remaining sections describe
the development design. The merged Smolnes port currently uses keyboard input.

Twilight exposes a standards-compliant HID Game Pad or Joystick collection as
`/dev/input/js0`. Binding is based on the HID report descriptor, not a USB
vendor/product allow-list. `2563:0527` is present only in the USB name table as
the fallback name `ShenZhen ShanWan Technology Co., Ltd. PC Gamepad`.

## Architecture

The stack follows FreeBSD's separation of responsibilities:

- `compat/freebsd_kpi/hid.rs` exposes the descriptor parser as
  `hid_start_parse`, `hid_get_item`, `hid_report_size`, `hid_locate`, and
  `hid_get_data`.
- `compat/freebsd_kpi/usbhid.rs` handles USB configuration, the HID report
  descriptor, non-fatal `SET_IDLE`, and interrupt-IN transport.
- `compat/freebsd_kpi/hidbus.rs` matches top-level Game Pad and Joystick
  application collections.
- `driver/gamepad/hgame.rs` adapts the generic FreeBSD-style mapping to the
  kernel state object.
- `driver/gamepad/joydev.rs` implements the Linux joystick userspace ABI.

The descriptor parser and hgame decoder live in the host-testable
`kernel/twilight_hid` crate. The fixed event history, INIT cursors, event byte
layout, and ioctl maps live in `kernel/twilight_joydev`. Adding another normal
HID controller should require no kernel source change. A new HID usage mapping,
when needed, belongs in the shared hgame layout rather than in a VID/PID quirk.

## ABI

Each read returns one or more eight-byte Linux `js_event` records. A new open
file receives button INIT records followed by axis INIT records. Reads block
when empty unless the descriptor was opened with `O_NONBLOCK`; `poll` and
`select` wake when events arrive.

The supported queries are `JSIOCGVERSION`, `JSIOCGAXES`, `JSIOCGBUTTONS`,
`JSIOCGNAME`, `JSIOCGAXMAP`, and `JSIOCGBTNMAP`. Map writes, calibration,
force feedback, and deprecated pre-1.0 operations return `ENOTTY`.

IRQ-time report processing is allocation-free. A fixed 64-event history is
shared by all open files, each of which has its own sequence cursor. If a
reader falls behind the history, that reader restarts with a fresh INIT
snapshot instead of continuing from desynchronized state.

## QEMU passthrough

Use the UHCI controller and select the pad by stable USB identity:

```text
-device piix3-usb-uhci,id=uhci
-device usb-host,bus=uhci.0,vendorid=0x2563,productid=0x0527
```

The host user running QEMU needs read/write permission on the USB device node.
The current UHCI scope enumerates devices present during boot; hotplug,
unplug, suspend/resume, rumble, and interrupt-OUT are not implemented.

## Upstream references

- FreeBSD `hgame`: <https://cgit.freebsd.org/src/tree/sys/dev/hid/hgame.c>
- FreeBSD HID parser: <https://cgit.freebsd.org/src/tree/sys/dev/hid/hid.c>
- FreeBSD `usbhid`: <https://cgit.freebsd.org/src/tree/sys/dev/usb/input/usbhid.c>
- Linux joystick ABI: <https://github.com/torvalds/linux/blob/master/include/uapi/linux/joystick.h>
