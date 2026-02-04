# UHCI Transaction Lifecycle & Driver Logic

This document details how to orchestrate transfers using the UHCI structures.

## 1. Setup Phase: The Frame List
Because UHCI requires a pointer for every millisecond (Frame), but we want to process asynchronous transfers (like Control and Bulk) whenever possible, the standard implementation is:

1.  Allocate a single **Async Queue Head (QH)**.
2.  Set all 1024 entries in the **Frame List** to point to this Async QH.
3.  Mark the Async QH's **Head Link** as Terminate (T=1) initially.
4.  Mark the Async QH's **Element Link** as Terminate (T=1) initially.

This ensures that no matter which frame the controller is currently processing, it eventually looks at our Async QH.

---

## 2. Managing Control Transfers
A Control Transfer consists of three stages: **Setup**, **Data** (Optional), and **Status**.

### Step-by-Step Construction

#### A. Setup Stage
1.  **Buffer**: Create an 8-byte buffer in memory containing the `USB_DEVICE_REQUEST` structure.
2.  **TD Setup**: Allocate a new Transfer Descriptor.
    -   `PID`: `0x2D` (TD_TOKEN_PID_SETUP).
    -   `Device Addr/Endpoint`: Target device.
    -   `Data Toggle`: 0 (DATA0).
    -   `Buffer`: Physical address of the request buffer.
    -   `MaxLen`: 7 (Encodes `Length - 1`. For 8 bytes, value is 7).
    -   `Active`: 1.
    -   `T (Terminate)`: 0 (We will link to Data/Status).

#### B. Data Stage (If Request.Length > 0)
1.  **Buffer**: Allocate buffer for data (input or output).
2.  **TD Generation**:
    -   Break data into chunks of `MaxPacketSize` (e.g., 8, 16, 32, or 64 bytes).
    -   Create a TD for each chunk.
    -   `PID`: `0x69` (IN) for reading, `0xE1` (OUT) for writing.
    -   `Data Toggle`: Start with 1 (DATA1), then toggle 0->1->0...
    -   `Active`: 1.
3.  **Linking**: Link the Setup TD to the first Data TD, and subsequent Data TDs in a chain.

#### C. Status Stage
1.  **Direction**: Opposite of Data Stage.
    -   If Data was IN, Status is OUT (`0xE1`).
    -   If Data was OUT, Status is IN (`0x69`).
    -   If no Data stage, Status is IN (`0x69`).
2.  **TD Status**:
    -   `PID`: As above.
    -   `Data Toggle`: Always 1 (DATA1).
    -   `Buffer`: NULL (or dummy). `MaxLen`: `0x7FF` (Null Packet).
    -   `Active`: 1.
    -   `IOC`: 1 (Interrupt On Completion). This tells the driver the whole transfer is done.
    -   `T (Terminate)`: 1.

---

## 3. Submission & Execution

1.  **Chain the TDs**:
    -   `SetupTD -> Link` = `Addr(DataTD_0)`
    -   `DataTD_0 -> Link` = `Addr(DataTD_1)`
    -   ...
    -   `LastTD -> Link` = `Terminate`
2.  **Queue Insertion**:
    -   Write the physical address of the **Setup TD** into the `Element Link` of the **Async QH**.
    -   **Important**: Ensure `Q` bit is 0 (it's a TD) and `T` bit is 0.
3.  **Wait**:
    -   **Polling**: Read storage of the Status TD. Check `Active` bit. If 0, transfer is done.
    -   **Interrupt**: If interrupts enabled, wait for `USBSTS` bit 0.

---

## 4. Common Pitfalls

### Toggle Bits
-   **Setup** is always DATA0.
-   **First Data** packet is always DATA1.
-   **Status** is always DATA1.
-   Subsequent Bulk transfers must maintain the toggle state from where the last one left off.

### Low Speed Devices
-   If the port detects a Low Speed device (`PORTSC` bit 8 is 1), **ALL** TDs for that device must have the `LS` (Low Speed) bit set in the Control Word (Bit 18).
-   If you forget this, the packet will be sent at Full Speed and the device will ignore it.

### Max Packet Size
-   Control endpoints (Endpoint 0) typically have a MPS of 8 bytes initially.
-   You must read the first 8 bytes of the Device Descriptor to find the real `bMaxPacketSize0`.
-   Update your segmentation logic after finding this value.
