# Framebuffer TTY

`sys/console/tty.rs` owns termios, keyboard input, output buffering, and ANSI
parsing. Its framebuffer backend (`sys/console/framebuffer.rs`) owns UTF-8
decoding, glyph rendering, cursor drawing, and scrolling. Input editing in
noncanonical mode belongs to the application (including oksh), so output must
not insert cells that the application did not account for.

## Completion bells (#86)

oksh emits BEL (`0x07`) on failed completion and other editing errors. BEL is a
notification: it must not render a glyph, advance or wrap the cursor, erase the
drawn cursor, or scroll the display. The renderer consumes decoded BEL before
any framebuffer or cursor update. Serial mirroring still forwards the original
output bytes, allowing a connected terminal to handle its own bell.

This follows FreeBSD's [`sys/teken/teken.c::teken_input_char()`](https://github.com/freebsd/freebsd-src/blob/main/sys/teken/teken.c),
which dispatches BEL separately from glyph rendering. Twilight has no audible
or visual bell backend, so its framebuffer bell is intentionally silent. Other
terminal controls and escape-sequence support remain limited to the existing
TTY implementation; this does not implement a complete teken emulator.

## Verification

`make test-tty` compiles the production framebuffer renderer and font on the
host with memory-backed framebuffer/VFS substitutes. Tests compare pixel
buffers and cursor positions after repeated bells, after completion bells
followed by Backspace editing, and at the right margin of the bottom row.

For the complete path, build the ISO, boot QEMU with a controllable monitor,
and use oksh in its normal editing mode:

1. Type `echo ttyzzzz`, then press Tab four times. The line and cursor must
   stay unchanged; no `?` appears.
2. Press Backspace four times, type `ok`, and press Enter. The visible command
   must be `echo ttyok`, and its output must be `ttyok`.
3. Press Tab on a valid unique command/path prefix to confirm normal completion.

QEMU `sendkey` exercises keyboard input; `screendump /tmp/tty.ppm` captures the
framebuffer. A serial transcript alone cannot detect the original rendering
bug because it correctly contains BEL even when the framebuffer draws `?`.
