# Twilight OS Documentation

Pages describe the current `master` implementation unless explicitly marked
as development work.

## Build and development

- [Getting started](getting-started.md)
- [Build targets and prerequisites](build-system.md)
- [Repository layout](repository-layout.md)
- [Development, QEMU, and host file transfers](development.md)
- [Userspace applications](userspace.md)
- [Port sources and Git submodules](ports.md)
- [Hardware and ABI specifications](specifications.md)

## Kernel

- [Timekeeping and deadlines](timekeeping.md)
- [IRQ-shared locking audit](kernel-locking.md)
- [Framebuffer TTY](console.md)
- [Networking and socket waits](networking.md)
- [Process management](process-management.md)
- [Memory management](memory-management.md)
- [Syscall layer](syscall-layer.md)
- [TwilightFS internals](filesystem/)
- [Unix sockets and descriptor passing](kernel/unix_sockets_ipc.md)
- [Shared memory IPC](kernel/shared_memory_ipc.md)

## Services and desktop

- [twinit and service configuration](system/twinit.md)
- [twinitctl](system/twinitctl.md)
- [twilogd](system/twilogd.md)
- [Daemon processes](system/daemon_processes.md)
- [Wayland transport](system/twland_wayland_transport.md)
- [SHM rendering](system/twland_shm_rendering.md)
- [xdg-shell](system/twland_xdg_shell.md)
- [Layer-shell](system/twland_layer_shell.md)
- [Window management](system/twland_window_management.md)
- [Input](system/twland_input.md)

## Drivers and regression tools

- [USB xHCI](usb-xhci.md)
- [USB implementation guide](usb/implementation_guide.md)
- [Gamepad development design (not in master)](usb/gamepads.md)
- [ATA DMA](disk/ata_dma_implementation.md)
- [VirtIO block](disk/virtio_blk_implementation.md)
- [Timing regression harness](../tools/time-regression/README.md)
- [Networking regression harness](../tools/net-regression/README.md)
- [TTY regression tests](console.md#verification)
