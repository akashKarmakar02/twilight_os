# ATA DMA (Bus Master IDE) Driver Implementation Guide

This document describes how to implement a polling-based **ATA DMA** (Bus Master IDE) driver for PATA/SATA-in-compat devices. It mirrors the approach used by `ata_dma.rs`, which layers DMA on top of the legacy ATA I/O port interface.

## 1. Device Discovery (PCI)

ATA DMA uses the PCI **IDE controller** with Bus Master IDE (BMIDE) registers.

- **Class Code**: `0x01` (Mass Storage Controller)
- **Subclass**: `0x01` (IDE controller)
- **BAR4**: Bus Master IDE I/O base (must be I/O space; low bit = 1).

After locating an IDE controller:

- Enable **Bus Mastering** in PCI command register.
- Use `BAR4` as the BMIDE base.
- Primary channel BMIDE regs: `BAR4 + 0`.
- Secondary channel BMIDE regs: `BAR4 + 8`.

If no BMIDE controller is found, fall back to PIO.

---

## 2. Hardware Interface (Legacy ATA + BMIDE)

### 2.1 Legacy ATA I/O Ports
Standard primary/secondary channels:

- **Primary**: IO base `0x1F0`, control base `0x3F6`
- **Secondary**: IO base `0x170`, control base `0x376`

Common ATA registers (IO base):

| Offset | Name | Access |
| :--- | :--- | :--- |
| `+0` | Data | R/W |
| `+1` | Error / Features | R / W |
| `+2` | Sector Count | R/W |
| `+3` | LBA0 | R/W |
| `+4` | LBA1 | R/W |
| `+5` | LBA2 | R/W |
| `+6` | Drive/Head | R/W |
| `+7` | Status / Command | R / W |

Control base:

- `+0`: Alternate Status (R) / Device Control (W)

### 2.2 Bus Master IDE (BMIDE) Registers

BMIDE register block (per channel):

| Offset | Name | Access | Notes |
| :--- | :--- | :--- | :--- |
| `+0` | BM Command | R/W | Bit 0 = Start/Stop, Bit 3 = Direction (1=Read) |
| `+2` | BM Status | R/W | Bit 0 = Active, Bit 1 = Error, Bit 2 = IRQ |
| `+4` | BM PRDT | R/W | Physical address of PRDT (32-bit) |

---

## 3. DMA Data Structures

### 3.1 PRDT Entry

PRDT (Physical Region Descriptor Table) entries describe DMA buffers. Each entry:

```c
struct PRD {
    uint32_t addr;   // Physical address (32-bit)
    uint16_t count;  // Byte count; 0 = 64KiB
    uint16_t flags;  // Bit 15 = EOT (end of table)
};
```

Constraints:

- Entries must not cross a **64KiB boundary**.
- `count=0` encodes **64KiB**.
- Last entry must set **EOT** (bit 15).
- PRDT must be in **physically contiguous** memory and DMA-addressable (32-bit in this driver).

### 3.2 DMA Buffers

The driver uses a **bounce buffer** for cases where the caller’s virtual buffer cannot be mapped into a suitable PRDT:

- `DMA_BUF_BYTES = 256KiB` (512 * 512).
- PRDT buffer size: `PRDT_BYTES = 4096`.

---

## 4. Initialization Sequence

1.  **Register IRQ Handlers**:
    - IRQ 14 = Primary ATA.
    - IRQ 15 = Secondary ATA.
2.  **Find IDE Controller**:
    - PCI class `0x01`, subclass `0x01`.
    - Use BAR4 to compute BMIDE bases.
    - Enable bus mastering.
3.  **Initialize Buses**:
    - Primary: IO base `0x1F0`, CTRL base `0x3F6`, BMIDE base `BAR4 + 0`.
    - Secondary: IO base `0x170`, CTRL base `0x376`, BMIDE base `BAR4 + 8`.
4.  **Identify Drives**:
    - Issue IDENTIFY (`0xEC`) and read 256 words.
    - Determine LBA48 support (word 83 bit 10).
    - Select the best DMA/PIO transfer mode based on IDENTIFY.
5.  **Enable Features** (best-effort):
    - Read look-ahead (`SET FEATURES`, `0xAA`).
    - Write cache (`SET FEATURES`, `0x02`).

If no DMA-capable drives are found, fall back to the PIO driver.

---

## 5. Command Programming (LBA28 / LBA48)

### 5.1 LBA28

- Sector count is 8-bit.
- `0` encodes **256 sectors**.

Registers:

- `Sector Count`: count
- `LBA0..2`: low 24 bits of LBA
- `Drive/Head`: `0xE0 | (drive << 4) | (LBA >> 24)`

### 5.2 LBA48

- Sector count is 16-bit.
- `0` encodes **65536 sectors**.
- Write high bytes first, then low bytes.

Registers:

1. High-order:
   - `Sector Count` (hi)
   - `LBA0` = LBA[24..31]
   - `LBA1` = LBA[32..39]
   - `LBA2` = LBA[40..47]
2. Low-order:
   - `Sector Count` (lo)
   - `LBA0` = LBA[0..7]
   - `LBA1` = LBA[8..15]
   - `LBA2` = LBA[16..23]
   - `Drive/Head` = `0x40 | (drive << 4)`

---

## 6. DMA Read/Write Flow

### 6.1 PRDT Setup

1.  **Stop bus master** (clear BM Command bit 0).
2.  **Clear BM Status** (write `st | 0x06` to clear IRQ + Error).
3.  **Build PRDT** for target buffer:
    - Prefer **direct** PRDT from caller buffer.
    - If not possible, use **bounce buffer** and PRDT for that buffer.
4.  **Write PRDT physical address** to BM PRDT register.

### 6.2 Read DMA

1.  Select drive.
2.  Program LBA and sector count.
3.  Program BM Command:
    - Direction bit 3 = **1** (disk -> memory).
    - Start bit 0 = 0 (stopped).
4.  Issue **READ DMA** (`0xC8`) or **READ DMA EXT** (`0x25`).
5.  Start BM (set bit 0).
6.  Wait for completion:
    - IRQ or BM Status Active bit clears.
    - Error bit set => failure.
7.  Stop BM, clear status, and copy from bounce buffer if used.

### 6.3 Write DMA

1.  Select drive.
2.  Program LBA and sector count.
3.  Program BM Command:
    - Direction bit 3 = **0** (memory -> disk).
4.  Issue **WRITE DMA** (`0xCA`) or **WRITE DMA EXT** (`0x35`).
5.  Start BM (set bit 0).
6.  Wait for completion.
7.  Stop BM, clear status.

---

## 7. Interrupt Handling

DMA completion is detected using both IRQs and BM status:

- IRQ 14 => primary channel
- IRQ 15 => secondary channel

The driver:

1.  Tracks an IRQ flag per channel.
2.  During DMA wait, polls BM Status:
    - Bit 1 set => error.
    - Bit 0 cleared => DMA engine completed.
3.  Stops BM and clears IRQ/Error bits.
4.  Reads ATA Status to acknowledge the device interrupt.

---

## 8. Notes and Practical Limits

- **Max sectors per command**:
  - LBA28: 256 sectors.
  - LBA48: 65536 sectors.
- **PRDT entry limit**:
  - `PRDT_BYTES / sizeof(PRD)` entries, each up to 64KiB.
- **32-bit DMA**:
  - This driver requires DMA buffers below 4GiB.
- **Fallback path**:
  - If IDENTIFY or DMA setup fails, the PIO driver is used.
