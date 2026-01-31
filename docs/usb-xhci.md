# USB xHCI Driver

This page documents the current xHCI (USB 3.x) driver bring-up in Twilight OS. The implementation is in:
- `twilight_kernel/src/driver/usb/xhci/xhci.rs`
- `twilight_kernel/src/driver/usb/xhci/xhci_regs.rs`

## Device discovery
The USB subsystem scans PCI devices and looks for class code:
- Class: `0x0C` (Serial Bus)
- Subclass: `0x03` (USB)
- Programming interface: `0x30` (xHCI)

When a matching device is found, the driver reads BAR0 and uses it as the MMIO base.

## MMIO mapping
During `XhciDriver::new()`:
- A 4 KiB window is mapped via `map_mmio()`
- The base pointer is converted to a virtual address using the HHDM offset
- The capability registers pointer is set to the base
- The operational registers pointer is computed from `caplength`

If the MMIO base is zero or mapping fails, the device is skipped.

## Capability parsing
The driver parses capability registers to determine limits and features:

From `HCSPARAMS1`:
- `max_device_slots`
- `max_interrupters`
- `max_ports`

From `HCSPARAMS2`:
- `isochronous_scheduling_threshold`
- `erst_max`
- `max_scratchpad_buffers`

From `HCCPARAMS1`:
- 64-bit addressing capability
- context size (32 or 64 bytes)
- port power control
- port indicators
- light reset
- extended capability offset

## DMA structures (DCBAA and scratchpads)
The driver allocates DMA-safe memory using `PhysBuf`:

1) Device Context Base Address Array (DCBAA)
- Size: (max slots + 1) * 8 bytes
- Entry 0 is reserved for the scratchpad array pointer

2) Scratchpad array and buffers
- Allocated if the controller requests scratchpads
- Scratchpad array entries point to page-sized scratchpad buffers

Once allocated:
- `dcbaap` is programmed with the DCBAA address
- `config` is set to `max_device_slots`

## Port power and polling
If port power control is supported, the driver sets the PP bit in each PORTSC register.

Port polling:
- PORTSC registers start at offset `0x400`, stride `0x10`
- The driver reads each port, compares the CCS bit with the cached value, and logs connect/disconnect events
- Port speed is decoded from PORTSC bits [13:10]

Speed mapping:
- 1: full-speed
- 2: low-speed
- 3: high-speed
- 4: super-speed
- 5: super-speed-plus

## Current limitations
This driver is an early bring-up stub:
- No command ring / event ring
- No device context or endpoint setup
- No transfers or USB protocol stack integration
- Polling only logs connect/disconnect and speed

The existing scaffolding is a good base for implementing full xHCI rings and USB device enumeration.
