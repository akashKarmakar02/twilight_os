# XHCI Register Interface

The XHCI controller uses **MMIO** (Memory Mapped I/O) for almost all communication. The base address is found in standard PCI configuration (BAR0).

## Register Sets
XHCI defines multiple register sets. The offsets for these are not fixed but are found relative to the MMIO Base.

1.  **Capability Registers**: Located at Offset 0. Defines limits and offsets to other regions.
2.  **Operational Registers**: Located at `Base + CAPLENGTH`. Controls run/stop, rings.
3.  **Runtime Registers**: Located at `Base + RTSOFF`. Controls interrupts.
4.  **Doorbell Registers**: Located at `Base + DBOFF`. Triggers hardware work.

---

## 1. Capability Registers (Read-Only)
Located at the very beginning of the MMIO region.

| Offset | Name | Size | Description |
| :--- | :--- | :--- | :--- |
| `0x00` | **CAPLENGTH** | 1 byte | Length of this capability register section. Used to find Operational Registers. |
| `0x02` | **HCIVERSION** | 2 bytes | Interface Version Number (e.g., `0x0100` for 1.0). |
| `0x04` | **HCSPARAMS1** | 4 bytes | Structural Parameters 1. Max Slots, Max Interrupters, Max Ports. |
| `0x08` | **HCSPARAMS2** | 4 bytes | Structural Parameters 2. Max Scratchpad Buffers, ERST Max. |
| `0x10` | **HCCPARAMS1** | 4 bytes | Capability Parameters 1. 64-bit addressing flag, Context Size (32/64 byte). |
| `0x14` | **DBOFF** | 4 bytes | Doorbell Offset. |
| `0x18` | **RTSOFF** | 4 bytes | Runtime Register Space Offset. |

---

## 2. Operational Registers
Located at `MMIO Base + CAPLENGTH`.

| Offset | Name | Size | Description |
| :--- | :--- | :--- | :--- |
| `0x00` | **USBCMD** | 4 bytes | **USB Command**. Run/Stop, Reset, Interrupter Enable. |
| `0x04` | **USBSTS** | 4 bytes | **USB Status**. Halted, Error, Event Interrupt. |
| `0x08` | **PAGESIZE** | 4 bytes | Supported page size (usually 4KB). |
| `0x14` | **DNCTRL** | 4 bytes | Notification Control. |
| `0x18` | **CRCR** | 8 bytes | **Command Ring Control Register**. Physical Pointer to Command Ring. |
| `0x30` | **DCBAAP** | 8 bytes | **Device Context Base Address Array Pointer**. Phys ptr to DCBAA. |
| `0x38` | **CONFIG** | 4 bytes | Max Device Slots Enabled. |
| `0x400`+ | **PORTSC** | 4 bytes | Port Status & Control (Array, one per port). |

### USBCMD Bits
-   **Bit 0 (R/S)**: Run/Stop. 1=Run.
-   **Bit 1 (HCRST)**: Reset. Write 1 to reset controller.
-   **Bit 2 (INTE)**: Interrupter Enable. Global switch for specific interrupters.

### USBSTS Bits
-   **Bit 0 (HCH)**: Halted. 1 = Stopped.
-   **Bit 3 (EINT)**: Event Interrupt. An interrupter has pending events.
-   **Bit 11 (CNR)**: Controller Not Ready. If 1, controller cannot be written to.

---

## 3. Runtime Registers
Located at `MMIO Base + RTSOFF`.
Primarily used for the **Interrupters**.

| Offset | Name | Size | Description |
| :--- | :--- | :--- | :--- |
| `0x00` | **MFINDEX** | 4 bytes | Microframe Index. |
| `0x20` | **IR** (Array) | 32 bytes | Interrupter Register Sets (0..1023). |

### Interrupter Register Set (IR[0])
| Offset | Name | Size | Description |
| :--- | :--- | :--- | :--- |
| `0x00` | **IMAN** | 4 bytes | **Interrupt Management**. Bit 0: Interrupt Pending. Bit 1: Interrupt Enable. |
| `0x04` | **IMOD** | 4 bytes | Interrupt Moderation. |
| `0x08` | **ERSTSZ** | 4 bytes | **Event Ring Segment Table Size**. Number of segments (usually 1). |
| `0x10` | **ERSTBA** | 8 bytes | **Event Ring Segment Table Base Address**. Ptr to ERST array. |
| `0x18` | **ERDP** | 8 bytes | **Event Ring Dequeue Pointer**. Where software is currently reading. |

---

## 4. Doorbell Registers
Located at `MMIO Base + DBOFF`.
Array of 32-bit registers.

-   `DB[0]`: Host Controller Doorbell (Used for Command Ring).
-   `DB[1..MaxSlots]`: Device Doorbells (Used for Transfer Rings).

**Usage**: Write a `Target` value (Endpoint index) to the DB register to notify hardware that a new TRB is available.
-   Write `0` to `DB[0]` after adding to Command Ring.
-   Write `Endpoint ID` to `DB[SlotID]` after adding to Transfer Ring.
