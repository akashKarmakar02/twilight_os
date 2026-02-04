
# Deep Dive: USB Driver Implementation in Twilight OS

This document serves as a comprehensive reference for implementing the Universal Serial Bus (USB) stack in Twilight OS. It details the architecture, hardware interfaces, memory structures, and specific implementation steps for both **UHCI** (USB 1.1) and **XHCI** (USB 3.0) controllers.

---

## 1. High-Level Architecture

The USB stack follows a **Polymorphic architecture** to abstract hardware differences from device logic.

### 1.1 The Manager (`mod.rs`)
The generic manager scans the PCI bus for devices with Class `0x0C` (Serial Bus Controller) and Subclass `0x03` (USB). It differentiates controllers by Programming Interface (ProgIF):
-   `0x00`: **UHCI** (Universal Host Controller Interface) - USB 1.1
-   `0x10`: **OHCI** (Open Host Controller Interface) - USB 1.1 (Not implemented)
-   `0x20`: **EHCI** (Enhanced Host Controller Interface) - USB 2.0 (Not implemented)
-   `0x30`: **XHCI** (eXtensible Host Controller Interface) - USB 3.0+

### 1.2 The Abstraction (`HostController` Trait)
Hardware specifics are hidden behind the `HostController` trait (`interfaces.rs`).

```rust
pub trait HostController {
    // Synchronous control transfer (Setup -> Data -> Status)
    fn control_transfer(
        &mut self, addr: u8, endp: u8, setup: [u8; 8],
        data: Option<&mut [u8]>, low_speed: bool
    ) -> Result<usize, UsbError>;

    // Asynchronous interrupt transfer (Periodic polling)
    fn schedule_interrupt(
        &mut self, addr: u8, endp: u8, mps: u8, interval: u8,
        buf_phys: u64, len: usize, low_speed: bool
    ) -> Result<Box<dyn InterruptTransfer>, UsbError>;
}
```

---

## 2. UHCI Implementation (USB 1.1)

UHCI is the simplest controller, relying heavily on software scheduling. It communicates via **I/O Ports** (PIO) rather than MMIO.

### 2.1 Hardware Interface (I/O Ports)
The controller exposes a set of 16-bit ports relative to a base address (BAR4 in PCI config).

| Offset | Name | Description |
| :--- | :--- | :--- |
| `0x00` | `USBCMD` | Command Register (Run/Stop, Reset) |
| `0x02` | `USBSTS` | Status Register (Interrupts, Halted) |
| `0x06` | `FRNUM` | Frame Number (Current 1ms frame index) |
| `0x08` | `FLBASEADD` | Frame List Base Address (Phys Pointer to 4KB page) |
| `0x10` | `PORTSC1` | Port 1 Status/Control |
| `0x12` | `PORTSC2` | Port 2 Status/Control |

### 2.2 Memory Structures
UHCI uses two main structures located in main memory, accessed by the controller via physical addresses.

#### A. Frame List
A single 4KiB page containing **1024 32-bit pointers**.
-   Hardware reads `FrameList[CurrentFrame % 1024]` every 1ms.
-   Each entry points to a Request Descriptor (QH or TD).
-   **Twilight Implementation**: All 1024 entries point to a single "Async QH" to allow processing control transfers in any frame.

#### B. Queue Head (QH)
Groups transfers together.
-   `Head Link`: Pointer to the next QH (horizontal).
-   `Element Link`: Pointer to the first TD in this queue (vertical).

#### C. Transfer Descriptor (TD)
Represents a single packet transaction.

| Bits | Field | Description |
| :--- | :--- | :--- |
| 0-31 | `Link Ptr` | Address of next TD/QH. Bit 0: Terminate, Bit 1: Is QH, Bit 2: Depth First. |
| 32-63| `Control` | Status bits. Bit 23: Active (1=HW owns, 0=SW owns). Bit 29: Short Packet Detect. |
| 64-95| `Token` | Packet definition. Bits 0-7: PID (Setup/In/Out). Bits 8-14: Address. Bits 19: Data Toggle. |
| 96-127| `Buffer` | Physical address of data buffer. |

### 2.3 Transaction Lifecycle (Control Transfer)
To perform a standard control transfer (e.g., `GET_DESCRIPTOR`):

1.  **Setup Stage**: Create a TD with PID `0x2D` (SETUP). `Active=1`. Points to existing Setup Request buffer.
2.  **Data Stage**: Create N TDs with PID `0x69` (IN) or `0xE1` (OUT). Toggle `DATA0`/`DATA1` bits. `Active=1`.
3.  **Status Stage**: Create 1 TD with PID opposite to data stage (or IN if no data). `Active=1`, `IOC=1` (Interrupt On Completion).
4.  **Chain**: Set `Link Ptr` of TD[0] -> TD[1] ... -> TD[Last].
5.  **Submit**: Write physical address of TD[0] to `async_qh.element_link`.
6.  **Wait**: Poll TD[0]..TD[N]. If `Active` bit clears, it's done. If `Stalled` bit sets, device error.

---

## 3. XHCI Implementation (USB 3.0)

XHCI is designed for virtualization and efficiency, using asynchronous **Rings** and **Doorbells**. It communicates via **MMIO**.

### 3.1 Register Spaces
XHCI splits registers into three regions (offsets found in Capability Registers):
1.  **Capability Registers** (Read-Only): Capabilities (Max Ports, Max Slots).
2.  **Operational Registers** (Read/Write): `USBCMD` (Start/Stop), `CRCR` (Command Ring Ptr), `DCBAAP`.
3.  **Runtime Registers**: Interrupter configuration (`ERSTBA`, `ERDP`, `IMAN`).
4.  **Doorbell Registers**: Array of 32-bit registers, one per Slot.

### 3.2 Data Structures (The Contexts)
XHCI uses "Contexts" to track device state.
-   **DCBAA (Device Context Base Address Array)**: An array of pointers (indices 1..255) to Device Contexts.
-   **Device Context**: Large structure (variable size, usually 2KB+) containing:
    -   **Slot Context**: Device-wide info (Hub address, Root/Port num, Max Exit Latency).
    -   **Endpoint Contexts (0..31)**: Per-endpoint state (Ring Pointer, Max Packet Size, Interval).

### 3.3 The Rings
XHCI replaces lists of TDs with circular queues called Rings.
-   **Link TRB**: Special block at the end of a ring segment connecting it to the start (or next segment).
-   **Cycle Bit**: The "Validity" bit. The producer (Driver) toggles the Cycle Bit every time it wraps around the ring. The consumer (Hardware) only processes TRBs where `TRB.Cycle == ExpectedCycle`.

#### A. Command Ring
-   Driver writes **Command TRBs** (e.g., `Enable Slot`, `Address Device`).
-   Controlled by `CRCR` register.
-   Host Controller (HC) consumes them.

#### B. Event Ring
-   Hardware writes **Event TRBs** (e.g., `Transfer Event`, `Command Completion Event`).
-   Driver consumes them.
-   Managed via **ERST** (Event Ring Segment Table) and **ERDP** (Event Ring Dequeue Pointer).

#### C. Transfer Rings
-   One per Endpoint.
-   Driver writes **Transfer TRBs** (Normal, Setup, Data, Status).

### 3.4 Initialization Sequence
1.  **Reset Controller**: Write `USBCMD.HCRST`. Wait for `USBSTS.CNR` (Controller Not Ready) to become 0.
2.  **config DCBAA**: Allocate `u64` array. Write address to `DCBAAP`.
3.  **Config Command Ring**: Allocate memory. Write address to `CRCR`. Set `RCS` (Ring Cycle State) = 1.
4.  **Config Event Ring**:
    -   Allocate segment.
    -   Create ERST Entry (Base + Size).
    -   Write ERST array address to `ERSTBA`.
    -   Write Dequeue Pointer to `ERDP`.
5.  **Enable Interrupter**: Set `IMAN.IE` = 1.
6.  **Start**: Set `USBCMD.Run` = 1.

---

## 4. Generic Enumeration Process

Regardless of the underlying controller (UHCI or XHCI), the enumeration flow logic remains similar at the high level, though the specific commands differ.

### Step 1: Port Reset
-   **UHCI**: Set `Port Reset` bit in port status. Wait 50ms. Clear it. Enable port.
-   **XHCI**: Write `reset` bit. Wait for `Port Enabled` change event.

### Step 2: Get Descriptor (Prefix)
-   Read just 8 bytes of the Device Descriptor (`wLength=8`).
-   This reveals `bMaxPacketSize0` (MPS0).
-   **UHCI**: Use default address 0.
-   **XHCI**: Requires `Enable Slot` command -> `Address Device` command (which implicitly reads descriptors).

### Step 3: Set Address
-   **UHCI**: Send `SET_ADDRESS` control transfer. Device now responds to new address.
-   **XHCI**: Accomplished via `Address Device` command TRB. Hardware handles the USB packet.

### Step 4: Get Full Descriptor
-   Read full 18 bytes of Device Descriptor.
-   Read Configuration Descriptor (first 9 bytes to get total length, then full length).

### Step 5: Parse & Set Configuration
-   Iterate descriptors to find desired Interfaces (e.g., Class 3 = HID).
-   Send `SET_CONFIGURATION` control transfer.

### Step 6: Driver Handoff
-   Identify endpoint attributes (Interrupt IN).
-   Create a device instance (`UsbDevice`).
-   Pass to specific driver (e.g., `KeyboardDriver::init`).

---

## 5. Implementation Tips & Pitfalls

1.  **Memory Alignment**:
    -   UHCI: Frame list **must** be 4KiB aligned. TDs/QHs **must** be 16-byte aligned.
    -   XHCI: DCBAA 64-byte aligned. Rings 16/64-byte aligned depending on segment size.
2.  **Cache Coherency**:
    -   The CPU caches writes to memory. The USB controller reads strictly from RAM.
    -   **Always** flush cache or use `Uncacheable` memory pages for structures shared with hardware.
    -   When polling, use `read_volatile`.
3.  **Toggle Bits**:
    -   Legacy USB (UHCI) requires manual management of `DATA0/DATA1` toggle bits in TDs.
    -   XHCI manages this internally but requires correct Cycle Bit management in Rings.
4.  **Endianness**: USB data on the wire is Little Endian. x86 is Little Endian, so direct mapping works, but standard mandates explicit conversion (e.g., `to_le_bytes`).

## 6. Resources

-   **UHCI Spec 1.1**: Intel Universal Host Controller Interface Specification.
-   **XHCI Spec 1.0**: Intel eXtensible Host Controller Interface for Universal Serial Bus.
-   **OSDev Wiki**: Excellent practical examples for OS developers.
