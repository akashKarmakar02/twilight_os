# XHCI Initialization Sequence

Initializing an XHCI controller is significantly more involved than UHCI due to the setup of shared memory structures.

## Step 1: Controller Reset
1.  **Map MMIO**: Locate BAR0 and map it to virtual memory.
2.  **Read Capabilities**:
    -   Read `CAPLENGTH` to find Operational Base.
    -   Read `HCSPARAMS1` to find `MaxSlots` and `MaxPorts`.
    -   Read `RTSOFF` and `DBOFF`.
3.  **Stop Controller**: Write `USBCMD.RS` = 0. Wait for `USBSTS.HCH` = 1.
4.  **Reset**: Write `USBCMD.HCRST` = 1.
5.  **Wait**: Poll `USBCMD.HCRST` until it clears to 0. Then wait for `USBSTS.CNR` (Controller Not Ready) to clear to 0.

## Step 2: Memory setup
1.  **Max Slots**: Read `HCSPARAMS1`. Valid slots are 1 to `MaxSlots`.
2.  **DCBAA**:
    -   Allocate `(MaxSlots + 1) * 8` bytes.
    -   Must be **64-byte aligned**.
    -   Write physical address to `DCBAAP` register.
3.  **Command Ring**:
    -   Allocate a 1KB-4KB page for the ring.
    -   Initialize memory to 0.
    -   Write physical address to `CRCR` register.
    -   Set `RCS` bit (Ring Cycle State) in `CRCR` to 1 (Producer/Consumer start at cycle 1).
4.  **Interrupters**:
    -   **ERST**: Allocate Event Ring Segment Table (16 bytes * 1 segment).
    -   **Event Ring**: Allocate ring memory (e.g., 4KB).
    -   **Fill ERST**: Set `ERST[0].Base` = EventRingPhys, `ERST[0].Size` = RingSize.
    -   **Set Registers**:
        -   Write ERST Phys Addr to `ERSTBA` (Runtime Offset `0x00 + 0x10`).
        -   Write `1` to `ERSTSZ`.
        -   Write Event Ring Start Address to `ERDP`.
        -   Enable Interrupter: Write `3` (Review IE + IP bits) to `IMAN`.

## Step 3: Enable Controller
1.  **Set Config**: Write `MaxSlots` to `CONFIG` register.
2.  **Start**: Write `USBCMD.RS` = 1.
3.  **Verify**: Read `USBSTS.HCH` -> Should be 0.

## Step 4: Protocol Initialization (USB 2.0 vs 3.0 Ports)
XHCI controllers often manage both USB 2.0 and USB 3.0 ports.
-   Walk the `Extended Capabilities` in MMIO.
-   Look for `Supported Protocol` capability.
-   Check `Major Revision`.
-   If USB 2.0/3.0, you might need to toggle specific port routing logic if legacy support is active (BIOS handover).
-   **BIOS Handoff**: Check for `USBLEGSUP` capability. If present, claim ownership from BIOS to prevent conflicts.

## Step 5: Port Enumeration
1.  **Poll PORTSC**: Iterate `PORTSC` registers (Offset `0x400`).
2.  **Connect Status**: Check Bit 0 (CCS).
3.  **Reset**:
    -   If CCS=1, write Bit 4 (PR - Port Reset) = 1.
    -   Wait for `Port Enabled` event (or poll Status Change bit).
4.  **Slot Allocation**:
    -   Issue `Enable Slot` command on Command Ring.
    -   Wait for Command Completion Event.
    -   Read `Slot ID` from event TRB.
5.  **Context Operations**:
    -   Allocate Device Context structure.
    -   Put Pointer into `DCBAA[SlotID]`.
    -   Initialize `Input Context` with `Slot Context` (Speed, Root Hub Port) and `Endpoint 0 Context` (MPS, type Control).
    -   Issue `Address Device` command.
