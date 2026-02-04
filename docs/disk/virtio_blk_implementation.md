# VirtIO Block Driver Implementation Guide

This document describes how to implement a driver for the **VirtIO Block Device** (disk), a high-performance virtual I/O device standard commonly used in virtualization (QEMU, KVM, VirtualBox).

## 1. Device Discovery (PCI)

VirtIO devices are found via the PCI bus.

-   **Vendor ID**: `0x1AF4` (Qumranet/Red Hat).
-   **Device ID**:
    -   Legacy: `0x1001` (Block Device).
    -   Modern: `0x1040` + Subsystem ID `2`.
-   **Subsystem ID**: `0x0002` (Block Device).
-   **Class Code**: `0x01` (Mass Storage Controller), Subclass `0x00` (SCSI).

**Note on Legacy vs. Modern**:
Many emulators default to the "Legacy" interface (VirtIO 0.9.5). It uses I/O ports specified in `BAR0`. Modern (VirtIO 1.0) uses Memory Mapped I/O and capability structures. **This guide focuses on the Legacy interface** as it is simpler and widely supported for OS development.

---

## 2. Hardware Interface (Legacy)

The device is controlled via I/O ports found in **BAR0**.

| Offset | Name | Access | Description |
| :--- | :--- | :--- | :--- |
| `0x00` | `Device Features` | R | Features supported by the Host (read-only). |
| `0x04` | `Guest Features` | W | Features accepted by the Guest (Driver). |
| `0x08` | `Queue Address` | W | Physical Page Frame Number (PFN) of the active queue. |
| `0x0C` | `Queue Size` | R | Max size of the active queue (e.g., 128). |
| `0x0E` | `Queue Select` | W | Select which queue to configure (Index). |
| `0x10` | `Queue Notify` | W | Write Queue Index here to notify Host of new work. |
| `0x12` | `Device Status` | R/W | Initialization status tracking. |
| `0x13` | `ISR Status` | R | Interrupt Status. Read to clear interrupt. |

### Config Space (Offset `0x14`+)
Specific to Block Devices.
-   `0x00`: Capacity (u64, in 512-byte sectors).
-   `0x08`: Size Max (u32).
-   `0x0C`: Seg Max (u32).
-   `0x10`: Cylinder/Head/Sector info.

---

## 3. The Virtqueue
The core communication mechanism. A Virtqueue consists of three parts physically contiguous in guest memory (Legacy requirement, though often relaxed).

### 3.1 Descriptor Table
Array of `16-byte` descriptors.
```rust
struct VirtqDesc {
    addr: u64,  // Physical Address of buffer
    len: u32,   // Length of buffer
    flags: u16, // NEXT(1), WRITE(2), INDIRECT(4)
    next: u16,  // Index of next descriptor (if NEXT flag set)
}
```

### 3.2 Available Ring (Driver -> Device)
Tells the device which descriptor chains are ready for processing.
```rust
struct VirtqAvail {
    flags: u16,       // NO_INTERRUPT(1)
    idx: u16,         // Index of the next entry to be written
    ring: [u16; QueueSize], // Array of descriptor indices
    used_event: u16,  // (Only if VIRTIO_F_EVENT_IDX)
}
```

### 3.3 Used Ring (Device -> Driver)
Tells the driver which requests have completed.
```rust
struct VirtqUsedElem {
    id: u32,  // Index of start of descriptor chain
    len: u32, // Bytes written into buffer
}
struct VirtqUsed {
    flags: u16,      // NO_NOTIFY(1)
    idx: u16,        // Index of next entry device will write
    ring: [VirtqUsedElem; QueueSize],
    avail_event: u16,
}
```

**Memory Alignment**: The detailed alignment rule for Legacy interface is:
-   Descriptor Table: 16 bytes.
-   Available Ring: 2 bytes.
-   Used Ring: 4096 bytes (Page aligned).

---

## 4. Initialization Sequence

1.  **Reset**: Write `0` to `Device Status` (`0x12`).
2.  **Acknowledge**: Set bit `ACKNOWLEDGE (1)` in `Device Status`.
3.  **Driver**: Set bit `DRIVER (2)` in `Device Status`.
4.  **Features**:
    -   Read `Device Features` (`0x00`).
    -   Negotiate: Write supported subset to `Guest Features` (`0x04`).
    -   **Important Features**:
        -   `VIRTIO_BLK_F_RO (5)`: Disk is read-only.
        -   `VIRTIO_BLK_F_FLUSH (9)`: Cache flush support.
        -   `VIRTIO_BLK_F_BARRIER (0)`: Legacy barrier (deprecated).
5.  **Features OK**: Set bit `FEATURES_OK (8)` if using Modern; Legacy skips this or implicitly accepts.
6.  **Queue Setup**:
    -   Select Queue 0: Write `0` to `Queue Select` (`0x0E`).
    -   Read `Queue Size` (`0x0C`). If 0, queue not available.
    -   Allocate memory for Desc + Avail + Padding + Used.
    -   Write Physical Address / 4096 to `Queue Address` (`0x08`).
7.  **Device Ready**: Set bit `DRIVER_OK (4)` in `Device Status`.

---

## 5. Sending a Request

A Block Request is a chained sequence of 3 descriptors (or more):
**Header** -> **Data** -> **Status**.

### Step 1: Request Header
Create a structure in memory (Stack or Heap):
```c
struct VirtioBlkReqHeader {
    uint32_t type; // 0=IN(Read), 1=OUT(Write), 4=FLUSH
    uint32_t priority;
    uint64_t sector;
};
```
-   **Descriptor 0**: Points to this header. `flags = NEXT`. `len = 16`.

### Step 2: Data Buffer
-   **Descriptor 1**: Points to the data sector buffer (e.g., 512 bytes).
-   If Reading (Type 0): `flags = NEXT | WRITE` (Device writes to this buffer).
-   If Writing (Type 1): `flags = NEXT` (Device reads from this buffer).

### Step 3: Status Byte
Create a 1-byte buffer for status.
-   **Descriptor 2**: Points to the u8 status byte.
-   `flags = WRITE` (Device writes status here).
-   `next = 0`.

### Step 4: Submission
1.  **Fill Avail Ring**:
    -   `Avail.ring[Avail.idx % Size] = IndexOf(Descriptor0)`.
    -   `Avail.idx++`.
2.  **Notify**: Write `0` (Queue Index) to `Queue Notify` port (`0x10`).

---

## 6. Interrupt Handling
When the device finishes:
1.  It raises an interrupt (IRQ).
2.  **Read ISR**: Read `ISR Status` port (`0x13`).
    -   Check Bit 0: Queue Interrupt.
    -   Check Bit 1: Config Change (Create/Resize).
3.  Reading the ISR automatically clears the interrupt line.
4.  **Check Used Ring**:
    -   Compare internal `last_used_idx` with `Used.idx`.
    -   Process all new entries.
