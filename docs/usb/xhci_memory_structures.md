# XHCI Memory Structures: Contexts

XHCI uses a hierarchical memory structure to manage device state. Unlike UHCI where software builds the schedule, XHCI software configures **Contexts**, and the hardware manages scheduling optimization.

## 1. DCBAA (Device Context Base Address Array)
The root structure pointed to by the `DCBAAP` register.

-   **Structure**: Array of 64-bit physical pointers.
-   **Size**: (MaxSlots + 1) * 8 bytes.
-   **Alignment**: 64 bytes.

| Index | Content |
| :--- | :--- |
| 0 | Scratchpad Buffer Array Pointer (if required by HCSPARAMS2). |
| 1 | Pointer to Device Context for Slot 1. |
| 2 | Pointer to Device Context for Slot 2. |
| ... | ... |

---

## 2. Device Context
A large structure describing a single connected device. It is an array of smaller contexts.
The Base Address in DCBAA[SlotID] points *directly* to the **Slot Context** (Index 0). However, technically the structure is indexed 0..31.

-   **Index 0**: Slot Context.
-   **Index 1**: Endpoint Context 0 (Default Control Endpoint).
-   **Index 2**: Endpoint Context 1.
-   **Index 3**: Endpoint Context 2.
-   ...
-   **Index 31**: Endpoint Context 30.

**Input Context**: When executing the `Enable Slot` or `Configure Endpoint` commands, the driver supplies an **Input Context**. This is slightly different: It has an extra "Control Context" at the beginning (Index depends on Context Size bit, usually before Slot Context) to indicate *which* parameters are changing.

---

## 3. Slot Context (Index 0)
Describes the device as a whole.

| Byte Offset | Bits | Field | Description |
| :--- | :--- | :--- | :--- |
| 0x00 | 20-23 | **Speed** | 1=Full, 2=Low, 3=High, 4=Super. |
| 0x00 | 27-31 | **Route String** | Route to device (for hubs). |
| 0x04 | 0-15 | **Max Exit Latency** | For power management. |
| 0x04 | 16-23 | **Root Hub Port** | Port number of the root hub. |
| 0x04 | 24-31 | **Context Entries** | Number of endpoint contexts following this (e.g., 1 for just EP0). |
| 0x08 | 0-19 | **Interrupter Target** | Which interrupter (0-1023) receives events for this slot. |
| 0x0C | 0-7 | **USB Device Address** | Assigned by hardware during `Address Device`. |
| 0x0C | 16-19 | **Slot State** | 0=Enabled, 1=Default, 2=Addressed, 3=Configured. |

---

## 4. Endpoint Context (Index 1..31)
Describes a specific endpoint (Pipe).

| Byte Offset | Bits | Field | Description |
| :--- | :--- | :--- | :--- |
| 0x04 | 0-2 | **EP State** | 0=Disabled, 1=Running, 2=Halted (Stalled). |
| 0x04 | 8-15 | **MaxPStreams** | Max Primary Streams (for Bulk Streams). |
| 0x04 | 16-23 | **Interval** | Polling interval (logarithmic). |
| 0x04 | 24-31 | **Max ESIT Payload** | High bandwidth setting. |
| 0x08 | 0-33 | **TR Dequeue Pointer** | Physical Pointer to the Transfer Ring. |
| 0x08 | 0 | **DCS** | Dequeue Cycle State (Initial Cycle Bit for ring). |
| 0x0C | 0-15 | **Average Look Length** | Scheduling hint. |
| 0x0C | 16-31 | **Max Packet Size** | e.g., 64, 512, 1024. |

---

## 5. ERST (Event Ring Segment Table)
Used by the Interrupter to find the Event Ring.

-   **Structure**: Array of 128-bit entries (16 bytes).
-   **Usage**: Pointed to by `ERSTBA`.

### ERST Entry Format
| Offset | Field | Description |
| :--- | :--- | :--- |
| 0x00 | **Base Address** | 64-bit Physical Address of the Event Ring memory block. |
| 0x08 | **Size** | 32-bit Number of TRBs in this block. |
| 0x0C | **Reserved** | Must be 0. |

Most implementations (including Twilight OS) use a single segment.
