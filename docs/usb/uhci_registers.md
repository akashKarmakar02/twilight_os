# UHCI Hardware Interface & Registers

The UHCI controller communicates primarily through **I/O Ports** (PIO). The base address for these ports is determined by reading **BAR4** from the PCI Configuration Space of the device (Class `0x0C`, Subclass `0x03`, ProgIF `0x00`).

## Port Map
All offsets are relative to the I/O base address.

| Offset | Name | Size | Access | Description |
| :--- | :--- | :--- | :--- | :--- |
| `0x00` | `USBCMD` | 2 bytes | R/W | **Command Register**. Controls the global state of the Host Controller. |
| `0x02` | `USBSTS` | 2 bytes | R/WC | **Status Register**. Reports interrupt status and execution state. Write 1 to clear bits. |
| `0x04` | `USBINTR` | 2 bytes | R/W | **Interrupt Enable**. Enables specific interrupts (Timeout, Resume, Complete). |
| `0x06` | `FRNUM` | 2 bytes | R/W | **Frame Number**. High 5 bits are the index into the Frame List (0-1023). |
| `0x08` | `FLBASEADD` | 4 bytes | R/W | **Frame List Base Address**. Upper 20 bits correspond to the physical address of the Frame List (4KB aligned). |
| `0x0C` | `SOFMOD` | 1 byte | R/W | **Start of Frame Modify**. Adjusts the length of a frame slightly (usually left at default `0x40`). |
| `0x10` | `PORTSC1` | 2 bytes | R/W | **Port 1 Status/Control**. Controls the connection state of Port 1. |
| `0x12` | `PORTSC2` | 2 bytes | R/W | **Port 2 Status/Control**. Controls the connection state of Port 2. |

---

## Detailed Register Descriptions

### USBCMD (Command Register) - Offset `0x00`

| Bit | Name | Description |
| :--- | :--- | :--- |
| 0 | **RS** (Run/Stop) | 1 = Run. 0 = Stop. When stopped, the HC does not process the schedule. |
| 1 | **HCRESET** | Host Controller Reset. Write 1 to reset internal state. Clears itself when done. |
| 2 | **GRESET** | Global Reset. Resets the bus (all ports). |
| 4 | **EGSM** | Enter Global Suspend Mode. |
| 6 | **CF** | Configure Flag. Write 1 to signal that drivers are configured. |
| 7 | **MAXP** | Max Packet (64 bytes if set, 32 bytes if clear). |

### USBSTS (Status Register) - Offset `0x02`

| Bit | Name | Description |
| :--- | :--- | :--- |
| 0 | **USBINT** | USB Interrupt. An interrupt occurred (IOC bit in TD was set). |
| 1 | **USBZE** | USB Error Interrupt. An error occurred. |
| 2 | **RD** | Resume Detect. |
| 3 | **HSE** | Host System Error. Serious PCI error. |
| 4 | **HCPE** | Host Controller Process Error. Schedule bug. |
| 5 | **HCH** | Halted. 1 = Schedule execution stopped (RS=0 or error). |

### PORTSC (Port Status/Control) - Offset `0x10` / `0x12`

| Bit | Name | Description |
| :--- | :--- | :--- |
| 0 | **CCS** | Current Connect Status. 1 = Device connected. |
| 1 | **CSC** | Connect Status Change. 1 = Connect/Disconnect event occurred. Write 1 to clear. |
| 2 | **PED** | Port Enabled/Disabled. 1 = Enabled. |
| 3 | **PEDC** | Port Enable/Disable Change. |
| 8 | **LS** | Low Speed Device Attached. 1 = Low Speed, 0 = Full Speed. |
| 9 | **PR** | Port Reset. Write 1 to reset port. Write 0 to end reset. |
