# XHCI Rings & TRBs

The mechanism for moving data and commands in XHCI is the **Transfer Request Block (TRB)** Ring.

## 1. Ring Concepts
-   **Circular Buffer**: A contiguous array of TRB structures.
-   **Producer/Consumer**:
    -   Software produces Command/Transfer TRBs. Hardware consumes them.
    -   Hardware produces Event TRBs. Software consumes them.
-   **Cycle Bit (C)**:
    -   Used to identify valid TRBs without overwriting memory with zeros.
    -   Producer toggles its internal "PCS" (Producer Cycle State) bit every time it wraps around the ring.
    -   Producer writes TRB with `TRB.C = PCS`.
    -   Consumer checks `TRB.C`. If `TRB.C == CCS` (Consumer Cycle State), the TRB is new/valid.
-   **Link TRB**: The last TRB in a ring segment. Contains pointer to the start (or next segment).
    -   **Toggle Cycle (TC)**: If set, the Producer toggles its PCS after processing this Link.

---

## 2. TRB Standard Format (16 Bytes)
All TRBs share a common size, though fields differ by type.

| Offset | Field | Description |
| :--- | :--- | :--- |
| 0x00 | **Parameter** | 64-bit Address or Data. |
| 0x08 | **Status** | Transfer Length, Residue, etc. |
| 0x0C | **Control** | Flags, Type, Cycle Bit. |

### Control Field (Word 3)
| Bit | Name | Description |
| :--- | :--- | :--- |
| 0 | **C** | Cycle Bit. |
| 10-15 | **TRB Type** | Code defining the TRB function (e.g., 1=Normal, 10=Enable Slot). |

---

## 3. Ring Types

### A. Command Ring
-   **Usage**: Driver issues commands to the Host Controller.
-   **Doorbell**: `DB[0]`.
-   **Common TRBs**:
    -   `No Op` (Type 23): Testing.
    -   `Enable Slot` (Type 9): Allocate a Device Slot ID. Returns `Slot ID` in Event.
    -   `Address Device` (Type 11): Assign address and read descriptors. Ptr = Input Context.
    -   `Configure Endpoint` (Type 12): Update contexts. Ptr = Input Context.
    -   `Evaluate Context` (Type 13): Check params (like MaxPacketSize).

### B. Event Ring
-   **Usage**: Hardware reports completion status. **Read-Only** for Driver.
-   **Pointer**: Managed via `ERDP` (Dequeue Pointer). Driver writes `ERDP` to clear events.
-   **Common TRBs**:
    -   `Transfer Event` (Type 32): Completion of a Transfer TRB. Status (Success, Stall).
    -   `Command Completion Event` (Type 33): Completion of a command. Contains `Slot ID` and Pointer to original Command TRB.
    -   `Port Status Change Event` (Type 34): Root hub connect/disconnect.
-   **Handling**:
    1.  Read `TRB[DequeuePtr]`.
    2.  Check if `TRB.C == ExpectedC`. If not, ring empty.
    3.  Process Event.
    4.  Increment DequeuePtr.
    5.  Update `ERDP` register (Write DequeuePtr | Bit 3 to clear EHB).

### C. Transfer Rings
-   **Usage**: Data movement for endpoints.
-   **Doorbell**: `DB[SlotID]`, Target = `Endpoint Index`.
-   **Common TRBs**:
    -   `Setup Stage` (Type 2): For Control transfers.
    -   `Data Stage` (Type 3): Data buffer.
    -   `Status Stage` (Type 4): Handshake.
    -   `Normal` (Type 1): Bulk/Interrupt data.
    -   `Link` (Type 6): Ring wrap-around.

---

## 4. Transfer Logic (Control Transfer)
Similar to UHCI, but explicitly typed.

1.  **Setup TRB**:
    -   Type: 2.
    -   Param: 8 bytes of Request Data (embedded, usually using "Immediate Data" flag) OR Pointer. **Note**: XHCI Setup TRB usually embeds the 8 bytes in Parameter Low/High.
    -   (Correction: Setup TRB Parameter is strictly immediate data of the 8 setup bytes).
    -   Flags: `IDT` (Immediate Data Type) = 1. `TRT` (Transfer Type) = 2 (IN) or 3 (OUT).
2.  **Data TRB** (Optional):
    -   Type: 3.
    -   Param: Buffer Ptr.
    -   Status: Length.
    -   Flags: `DIR` (Direction). `Chain` = 1 (if more data or status follows).
3.  **Status TRB**:
    -   Type: 4.
    -   Flags: `DIR` (Invert of Data). `IOC` (Interrupt On Completion) = 1.
4.  **Doorbell**:
    -   Write `EndpointID` (usually 1 for default pipe) to `DB[SlotID]`.
