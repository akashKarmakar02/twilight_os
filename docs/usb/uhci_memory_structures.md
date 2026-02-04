# UHCI Memory Structures

The UHCI controller uses main memory to store the schedule of transfers. The controller accesses these structures via **Physical Addresses**.

> **Important**: The hardware does not see CPU caches. All structures shared with the controller must be in **Uncacheable** memory or manually flushed from the cache before setting the Active bit.

## 1. Frame List
The Frame List is the root of the schedule.
- **Size**: 4KB (1 page).
- **Alignment**: 4KB aligned.
- **Content**: 1024 entries (32-bit pointers).
- **Usage**: The controller increments `FRNUM` every 1ms and reads the entry at `FrameList[FRNUM & 0x3FF]`.

### Entry Format
Bit 0 suggests if the pointer is invalid.
- **Bit 0 (T)**: Terminate. 1 means "frame empty".
- **Bit 1 (Q)**: Queue Head Select. 1 if pointing to a QH, 0 if pointing to a TD.
- **Bits 31:4**: Physical Address of the next structure (16-byte aligned).

---

## 2. Transfer Descriptor (TD)
Represents a single USB transaction (Packet).

**Alignment**: 16-byte aligned.

| Offset | Field | Description |
| :--- | :--- | :--- |
| `0x00` | **Link Pointer** | Points to the next TD or QH. <br>Bit 0 (T): Terminate. <br>Bit 1 (Q): 1=QH, 0=TD. <br>Bit 2 (VF): Depth First. |
| `0x04` | **Control & Status** | Status execution bits. |
| `0x08` | **Token** | Packet Header information (PID, Address, Endpoint). |
| `0x0C` | **Buffer Pointer** | Physical Address of the data buffer. |

### Control & Status (Word 1)
| Bits | Name | Description |
| :--- | :--- | :--- |
| 17 | **SPD** | Short Packet Detect. 1=Enable. |
| 18 | **LS** | Low Speed Device. 1=Low Speed. |
| 19 | **ISO** | Isochronous Select. |
| 23 | **Active** | 1=Hardware owns this TD. 0=Software owns it (Done). |
| 24 | **IOC** | Interrupt On Completion. 1=Fire interrupt when done. |
| 27-28 | **Error Counter** | Retries left (Startup value usually 3). |

### Token (Word 2)
| Bits | Name | Description |
| :--- | :--- | :--- |
| 0-7 | **PID** | Packet ID. `0x2D`=SETUP, `0x69`=IN, `0xE1`=OUT. |
| 8-14 | **Device Addr** | 7-bit USB Device Address. |
| 15-18 | **Endpoint** | 4-bit Endpoint Number. |
| 19 | **Data Toggle** | 0=DATA0, 1=DATA1. Must toggle 0->1->0 for bulk/control. |
| 21-31 | **Max Len** | Maximum bytes to transfer (0x7FF = 2047 bytes schedule limit). |

---

## 3. Queue Head (QH)
Used to organize TDs into queues (Control, Bulk, Interrupt).

**Alignment**: 16-byte aligned.

| Offset | Field | Description |
| :--- | :--- | :--- |
| `0x00` | **Head Link** | **Horizontal** pointer to the next QH in the schedule (or next TD). <br>Bit 0 (T): Terminate. <br>Bit 1 (Q): Is QH. |
| `0x04` | **Element Link** | **Vertical** pointer to the first TD in this queue. <br>Bit 0 (T): Terminate (Empty Queue). <br>Bit 1 (Q): Is QH. |
| `0x08` | Software Usage | Available for driver use (e.g., storing virtual address of this QH). |
| `0x0C` | Software Usage | Available for driver use. |

### Scheduling Logic
1. The hardware reads the **Head Link** to find the next unit of work.
2. If it finds a QH, it checks the **Element Link**.
3. If Element Link is valid (T=0), it processes the TD at that address.
4. If that TD completes (Active bit clears), the hardware updates the Element Link to point to the TD's Link Pointer (advancing the queue).
