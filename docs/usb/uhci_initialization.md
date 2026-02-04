# UHCI Initialization Sequence

Initializing the UHCI controller requires careful sequencing of the Global Reset and the configuration of the Frame List.

## 1. Discovery & resources
1.  **PCI Scanning**: Find device with Class `0x0C`, Subclass `0x03`, ProgIF `0x00`.
2.  **Disable Legacy Emulation**: Check the `LEGSUP` register in PCI Configuration Space (Offset `0xC0`). Write `0x8F00` to it to disable BIOS legacy support and claim ownership.
3.  **Read BAR4**: Get the I/O Base Address. Enables the I/O ports in the Command Register (`0x04`) of PCI Config if not enabled.

## 2. Reset Controller
1.  **Global Reset**: Write `0x0004` (GRESET) to `USBCMD`. Wait 10ms-100ms.
2.  **End Global Reset**: Write `0x0000` to `USBCMD`.
3.  **Host Controller Reset**: Write `0x0002` (HCRESET) to `USBCMD`.
4.  **Wait**: Poll `USBCMD` until Bit 1 (HCRESET) clears. This usually takes a few microseconds.

## 3. Frame List Setup
1.  **Allocate Memory**: Allocate a single 4KB page. **Must be 4KB aligned**.
2.  **Initialize**: Fill all 1024 entries with the `T` (Terminate) bit set (`0x00000001`).
3.  **Set Base Address**: Write the physical address of the page to `FLBASEADD` (Offset `0x08`).

## 4. Async Queue Setup
To allow processing control transfers even when no devices are active:
1.  **Allocate QH**: Create a "Skeleton" QH (Async QH).
2.  **Link**: Set all 1024 Frame List entries to point to this QH.
    -   `Entry = Ptr(AsyncQP) | 0x02` (Bit 1 = Q-Select).
3.  **Terminate**: Set the Async QH's Head and Element links to Terminate (`0x00000001`).

## 5. Enable Interrupters
1.  **Clear Status**: Write `0x003F` to `USBSTS` to clear any pending status bits (Write-Clear).
2.  **Enable Interrupts**: Write `0x000F` (Timeout, Resume, ICO, Short Packet) to `USBINTR` (Offset `0x04`).

## 6. Start Controller
1.  **Run**: Write `0x0001` (Run/Stop) to `USBCMD`.
2.  **Verify**: Read `USBSTS`. Bit 5 (Halted) should be 0.
3.  **Live**: The controller is now fetching from the Frame List every 1ms.

---

## 7. Port Handling
Once running, the Root Hub ports must be powered and reset.

### For Each Port (`PORTSC1`, `PORTSC2`):
1.  **Power On**: Unlike EHCI/XHCI, UHCI ports are usually powered if the controller is running.
2.  **Check Connection**: Read `PORTSC`. If Bit 0 (Current Connect Status) is 1:
3.  **Reset Port**:
    -   Write `PORTSC` with Bit 9 (Port Reset) = 1.
    -   Wait 50ms.
    -   Write `PORTSC` with Bit 9 = 0.
    -   Wait 10ms (Recovery).
4.  **Enable Port**:
    -   Write `PORTSC` with Bit 2 (Port Enable) = 1.
    -   **Important**: You must read-modify-write `PORTSC` carefully to avoid clearing "Change" bits (Bits 1, 3, 5). Write 1s to them to clear them only if you handled them.
5.  **Device Attached**: The device is now on the bus and at Address 0. Proceed to Enumeration.
