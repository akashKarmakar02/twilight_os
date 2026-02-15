use crate::driver::disk::AtaImpl;
use crate::driver::disk::mount_ata_with_impl;
use crate::println;
use crate::sys::memory::phys::PhysBuf;
use crate::sys::memory::virt_to_phys;
use crate::sys::pci::DeviceConfig;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use bit_field::BitField;
use core::convert::TryInto;
use core::fmt;
use core::hint::spin_loop;
use core::slice;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::VirtAddr;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

// ATA DMA (Bus Master IDE) driver.
// Minimal, polling-based implementation to keep the existing PIO driver intact.

pub const BLOCK_SIZE: usize = 512;
// Moderate bump to reduce per-command overhead without being too memory-aggressive.
// LBA28 remains capped at 256 sectors; LBA48 can use bigger transfers.
const DMA_BUF_BYTES: usize = 2 * 1024 * 1024; // 2MiB
const PRDT_BYTES: usize = 8192; // 8KiB => 1024 PRDT entries
const POLL_SPINS: usize = 5_000_000;
const DMA_POLL_SPINS_BEFORE_HALT: usize = 4096;
const MAX_MERGED_BYTES: usize = DMA_BUF_BYTES;
const MAX_MERGED_REQS: usize = 32;
const DMA_RETRY_COUNT: usize = 1;
const ENABLE_COOP_QUEUE: bool = true;

// If set, caps negotiated UDMA mode to this value (0..=6) regardless of controller heuristics.
// Leave as `None` for auto.
const FORCE_MAX_UDMA_MODE: Option<u8> = None;

// Drive-side performance features (best-effort; failures are ignored).
const ENABLE_WRITE_CACHE: bool = true;
const ENABLE_READ_LOOKAHEAD: bool = true;

const REQ_PENDING: u8 = 0;
const REQ_DONE: u8 = 1;
const REQ_FAILED: u8 = 2;

static IDE_IRQ_PRIMARY: AtomicBool = AtomicBool::new(false);
static IDE_IRQ_SECONDARY: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AtaReqOp {
    Read,
    Write,
}

struct AtaIoRequest {
    op: AtaReqOp,
    drive: u8,
    lba_start: u32,
    sectors: usize,
    buf_ptr: *mut u8,
    byte_len: usize,
    status: AtomicU8,
}

impl AtaIoRequest {
    fn new_read(drive: u8, lba_start: u32, buf: &mut [u8]) -> Self {
        Self {
            op: AtaReqOp::Read,
            drive,
            lba_start,
            sectors: buf.len() / BLOCK_SIZE,
            buf_ptr: buf.as_mut_ptr(),
            byte_len: buf.len(),
            status: AtomicU8::new(REQ_PENDING),
        }
    }

    fn new_write(drive: u8, lba_start: u32, buf: &[u8]) -> Self {
        Self {
            op: AtaReqOp::Write,
            drive,
            lba_start,
            sectors: buf.len() / BLOCK_SIZE,
            buf_ptr: buf.as_ptr() as *mut u8,
            byte_len: buf.len(),
            status: AtomicU8::new(REQ_PENDING),
        }
    }
}

struct AtaQueueState {
    active: bool,
    pending: VecDeque<usize>,
}

struct AtaChannelQueue {
    inner: Mutex<AtaQueueState>,
}

struct AtaPerfCounters {
    read_bytes: AtomicU64,
    write_bytes: AtomicU64,
    dma_cmd_read: AtomicU64,
    dma_cmd_write: AtomicU64,
    dma_cmd_retry: AtomicU64,
    dma_cmd_fail: AtomicU64,
    pio_fallback_count: AtomicU64,
    queue_enqueued: AtomicU64,
    queue_merged_groups: AtomicU64,
    queue_merged_reqs: AtomicU64,
    dma_wait_iterations_total: AtomicU64,
    prdt_histogram: [AtomicU64; 8],
}

impl AtaPerfCounters {
    const fn new() -> Self {
        Self {
            read_bytes: AtomicU64::new(0),
            write_bytes: AtomicU64::new(0),
            dma_cmd_read: AtomicU64::new(0),
            dma_cmd_write: AtomicU64::new(0),
            dma_cmd_retry: AtomicU64::new(0),
            dma_cmd_fail: AtomicU64::new(0),
            pio_fallback_count: AtomicU64::new(0),
            queue_enqueued: AtomicU64::new(0),
            queue_merged_groups: AtomicU64::new(0),
            queue_merged_reqs: AtomicU64::new(0),
            dma_wait_iterations_total: AtomicU64::new(0),
            prdt_histogram: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
        }
    }

    fn prdt_bucket(entries: usize) -> usize {
        if entries <= 1 {
            0
        } else if entries <= 2 {
            1
        } else if entries <= 4 {
            2
        } else if entries <= 8 {
            3
        } else if entries <= 16 {
            4
        } else if entries <= 32 {
            5
        } else if entries <= 64 {
            6
        } else {
            7
        }
    }
}

#[allow(dead_code)]
struct PerfLogState {
    last_ts: f64,
    last_read_bytes: u64,
    last_write_bytes: u64,
}

fn on_irq_primary() {
    IDE_IRQ_PRIMARY.store(true, Ordering::Release);
}

fn on_irq_secondary() {
    IDE_IRQ_SECONDARY.store(true, Ordering::Release);
}

// Keep track of the last selected bus and drive pair to speed up operations
pub static LAST_SELECTED: Mutex<Option<(u8, u8)>> = Mutex::new(None);

#[repr(u16)]
#[derive(Debug, Clone, Copy)]
enum Command {
    ReadDma = 0xC8,
    WriteDma = 0xCA,
    ReadDmaExt = 0x25,
    WriteDmaExt = 0x35,
    Identify = 0xEC,
    SetFeatures = 0xEF,
}

enum IdentifyResponse {
    Ata([u16; 256]),
    None,
}

#[allow(dead_code)]
#[repr(usize)]
#[derive(Debug, Clone, Copy)]
enum Status {
    ERR = 0,  // Error
    DRQ = 3,  // Data Request
    DRDY = 6, // Device Ready
    BSY = 7,  // Busy
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct Prd {
    addr: u32,
    count: u16, // 0 means 64KiB
    flags: u16, // bit15 = EOT
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct Bus {
    id: u8,

    // legacy IO ports (compat mode)
    data_register: Port<u16>,
    error_register: PortReadOnly<u8>,
    features_register: PortWriteOnly<u8>,
    sector_count_register: Port<u8>,
    lba0_register: Port<u8>,
    lba1_register: Port<u8>,
    lba2_register: Port<u8>,
    drive_register: Port<u8>,
    status_register: PortReadOnly<u8>,
    command_register: PortWriteOnly<u8>,
    alternate_status_register: PortReadOnly<u8>,
    control_register: PortWriteOnly<u8>,

    // bus master IDE ports
    bm_cmd: Port<u8>,    // +0
    bm_status: Port<u8>, // +2
    bm_prdt: Port<u32>,  // +4

    prdt: PhysBuf,
    dma_buf: PhysBuf,
    drive_lba48: [bool; 2],
    max_udma_mode: u8,
}

impl Bus {
    pub fn new(id: u8, io_base: u16, ctrl_base: u16, bm_base: u16, max_udma_mode: u8) -> Self {
        Self {
            id,
            data_register: Port::new(io_base + 0),
            error_register: PortReadOnly::new(io_base + 1),
            features_register: PortWriteOnly::new(io_base + 1),
            sector_count_register: Port::new(io_base + 2),
            lba0_register: Port::new(io_base + 3),
            lba1_register: Port::new(io_base + 4),
            lba2_register: Port::new(io_base + 5),
            drive_register: Port::new(io_base + 6),
            status_register: PortReadOnly::new(io_base + 7),
            command_register: PortWriteOnly::new(io_base + 7),
            alternate_status_register: PortReadOnly::new(ctrl_base + 0),
            control_register: PortWriteOnly::new(ctrl_base + 0),

            bm_cmd: Port::new(bm_base + 0),
            bm_status: Port::new(bm_base + 2),
            bm_prdt: Port::new(bm_base + 4),

            prdt: PhysBuf::new_dma32(PRDT_BYTES),
            dma_buf: PhysBuf::new_dma32(DMA_BUF_BYTES),
            drive_lba48: [false; 2],
            max_udma_mode,
        }
    }

    fn wait(&mut self, ns: u64) {
        crate::driver::timer::wait(ns);
    }

    fn status(&mut self) -> u8 {
        unsafe { self.alternate_status_register.read() }
    }

    fn poll(&mut self, bit: Status, val: bool) -> Result<(), ()> {
        for _ in 0..POLL_SPINS {
            if self.status().get_bit(bit as usize) == val {
                return Ok(());
            }
            spin_loop();
        }
        Err(())
    }

    fn select_drive(&mut self, drive: u8) -> Result<(), ()> {
        self.poll(Status::BSY, false)?;
        self.poll(Status::DRQ, false)?;

        if *LAST_SELECTED.lock() == Some((self.id, drive)) {
            return Ok(());
        }
        *LAST_SELECTED.lock() = Some((self.id, drive));

        unsafe {
            self.drive_register.write(0xA0 | (drive << 4));
        }
        self.wait(400);
        self.poll(Status::BSY, false)?;
        self.poll(Status::DRQ, false)?;
        Ok(())
    }

    fn write_command_params(&mut self, drive: u8, block: u32, sectors: usize) -> Result<(), ()> {
        // LBA28 sector count is 8-bit; 0 encodes 256 sectors.
        let sc: u8 = match sectors {
            0 => return Err(()),
            1..=255 => sectors as u8,
            256 => 0,
            _ => return Err(()),
        };
        let lba = true;
        let mut bytes = block.to_le_bytes();
        bytes[3].set_bit(4, drive > 0);
        bytes[3].set_bit(5, true);
        bytes[3].set_bit(6, lba);
        bytes[3].set_bit(7, true);
        unsafe {
            self.sector_count_register.write(sc);
            self.lba0_register.write(bytes[0]);
            self.lba1_register.write(bytes[1]);
            self.lba2_register.write(bytes[2]);
            self.drive_register.write(bytes[3]);
        }
        Ok(())
    }

    fn write_command_params_lba48(
        &mut self,
        drive: u8,
        block: u64,
        sectors: usize,
    ) -> Result<(), ()> {
        if sectors == 0 || sectors > 0x10000 {
            return Err(());
        }
        // LBA48: 16-bit sector count, written high then low. 0 means 65536.
        let sc: u16 = if sectors == 0x10000 {
            0
        } else {
            sectors as u16
        };
        let lba = block;

        let sc_hi = (sc >> 8) as u8;
        let sc_lo = (sc & 0xFF) as u8;

        // High-order bytes first
        unsafe {
            self.sector_count_register.write(sc_hi);
            self.lba0_register.write(((lba >> 24) & 0xFF) as u8);
            self.lba1_register.write(((lba >> 32) & 0xFF) as u8);
            self.lba2_register.write(((lba >> 40) & 0xFF) as u8);
        }
        // Then low-order bytes
        unsafe {
            self.sector_count_register.write(sc_lo);
            self.lba0_register.write((lba & 0xFF) as u8);
            self.lba1_register.write(((lba >> 8) & 0xFF) as u8);
            self.lba2_register.write(((lba >> 16) & 0xFF) as u8);
            // For LBA48, the head field is ignored; still set LBA + drive.
            self.drive_register.write(0x40 | (drive << 4));
        }
        Ok(())
    }

    fn write_command(&mut self, cmd: Command) -> Result<(), ()> {
        unsafe { self.command_register.write(cmd as u8) }
        self.wait(120);
        let _ = self.status();
        self.poll(Status::BSY, false)?;
        Ok(())
    }

    fn identify_80_conductor_cable(id: &[u16; 256]) -> Option<bool> {
        // Word 93 contains various hardware results. Many PATA drives report
        // 80-conductor cable presence here; SATA devices often leave it 0.
        //
        // We treat "0" as unknown rather than "not present" to avoid
        // artificially capping SATA-in-IDE-compat devices.
        let w93 = id[93];
        if w93 == 0 {
            return None;
        }
        Some((w93 & (1 << 13)) != 0)
    }

    fn best_xfer_mode_from_identify(&self, id: &[u16; 256]) -> Option<u8> {
        // Word 88: Ultra DMA modes (bits 0..7 supported, 8..15 active)
        let udma = id[88];
        let supported_udma = (udma & 0x00FF) as u8;
        let mut max_udma = self.max_udma_mode.min(6);
        if let Some(forced) = FORCE_MAX_UDMA_MODE {
            max_udma = forced.min(6);
        } else if let Some(false) = Self::identify_80_conductor_cable(id) {
            // Without an 80-conductor cable, UDMA > 2 is not reliable.
            max_udma = max_udma.min(2);
        }

        if supported_udma != 0 {
            for mode in (0..=6u8).rev() {
                if (supported_udma & (1u8 << mode)) != 0 {
                    let mode = core::cmp::min(mode, max_udma);
                    return Some(0x40 | mode); // UDMA mode encoding
                }
            }
        }

        // Word 63: Multiword DMA modes (bits 0..2 supported, 8..10 active)
        let mwdma = id[63];
        let supported_mwdma = (mwdma & 0x0007) as u8;
        if supported_mwdma != 0 {
            for mode in (0..=2u8).rev() {
                if (supported_mwdma & (1u8 << mode)) != 0 {
                    return Some(0x20 | mode); // MWDMA mode encoding
                }
            }
        }

        // Word 64: Advanced PIO modes supported (bits 0..1 => PIO3/PIO4)
        let pio = id[64] as u8;
        if (pio & 0x02) != 0 {
            return Some(0x08 | 4); // PIO4
        }
        if (pio & 0x01) != 0 {
            return Some(0x08 | 3); // PIO3
        }

        None
    }

    fn set_transfer_mode(&mut self, drive: u8, mode: u8) -> Result<(), ()> {
        // ATA SET FEATURES: subcommand 0x03 ("Set transfer mode")
        self.select_drive(drive)?;
        self.poll(Status::BSY, false)?;
        self.poll(Status::DRQ, false)?;

        unsafe {
            self.features_register.write(0x03);
            self.sector_count_register.write(mode);
            self.lba0_register.write(0);
            self.lba1_register.write(0);
            self.lba2_register.write(0);
            self.drive_register.write(0xA0 | (drive << 4));
        }

        self.write_command(Command::SetFeatures)?;

        // Check for error.
        let st = self.status();
        if st.get_bit(Status::ERR as usize) {
            return Err(());
        }

        Ok(())
    }

    fn set_feature(&mut self, drive: u8, feature: u8) -> Result<(), ()> {
        self.select_drive(drive)?;
        self.poll(Status::BSY, false)?;
        self.poll(Status::DRQ, false)?;

        unsafe {
            self.features_register.write(feature);
            self.sector_count_register.write(0);
            self.lba0_register.write(0);
            self.lba1_register.write(0);
            self.lba2_register.write(0);
            self.drive_register.write(0xA0 | (drive << 4));
        }

        self.write_command(Command::SetFeatures)?;
        let st = self.status();
        if st.get_bit(Status::ERR as usize) {
            return Err(());
        }
        Ok(())
    }

    fn program_prdt(&mut self) {
        unsafe {
            // Stop bus master.
            let mut cmd = self.bm_cmd.read();
            cmd.set_bit(0, false);
            self.bm_cmd.write(cmd);

            // Clear interrupt/error bits (write 1 to clear).
            let st = self.bm_status.read();
            self.bm_status.write(st | 0x06);

            // Program PRDT pointer.
            self.bm_prdt.write(self.prdt.addr() as u32);
        }
    }

    fn setup_dma_prdt_for_phys(&mut self, phys_base: u64, len: usize) -> Result<(), ()> {
        if len == 0 || len > (PRDT_BYTES / core::mem::size_of::<Prd>()) * 0x10000 {
            return Err(());
        }
        if phys_base > u32::MAX as u64 {
            return Err(());
        }

        // Build PRDT entries, splitting at 64KiB boundaries if needed.
        let base = phys_base;
        let mut remaining = len;
        let mut offset = 0usize;
        let mut entries: usize = 0;

        let prd_slice: &mut [Prd] = unsafe {
            core::slice::from_raw_parts_mut(
                self.prdt.virt_addr().as_mut_ptr::<Prd>(),
                PRDT_BYTES / core::mem::size_of::<Prd>(),
            )
        };

        while remaining > 0 {
            if entries >= prd_slice.len() {
                return Err(());
            }

            let phys = base + offset as u64;
            if phys > u32::MAX as u64 {
                return Err(());
            }
            let boundary = ((phys + 0x10000) & !0xFFFF) as u64;
            let to_boundary = (boundary - phys) as usize;
            let chunk = core::cmp::min(remaining, to_boundary);

            let count16: u16 = if chunk == 0x10000 { 0 } else { chunk as u16 };
            prd_slice[entries] = Prd {
                addr: phys as u32,
                count: count16,
                flags: 0,
            };
            entries += 1;
            remaining -= chunk;
            offset += chunk;
        }

        if entries == 0 {
            return Err(());
        }
        ATA_PERF.prdt_histogram[AtaPerfCounters::prdt_bucket(entries)]
            .fetch_add(1, Ordering::Relaxed);
        if entries < prd_slice.len() {
            prd_slice[entries] = Prd::default();
        }
        prd_slice[entries - 1].flags = 1u16 << 15; // EOT
        self.program_prdt();
        Ok(())
    }

    fn setup_dma_prdt_for_virt(&mut self, buf_ptr: *const u8, len: usize) -> Result<(), ()> {
        if len == 0 || len > (PRDT_BYTES / core::mem::size_of::<Prd>()) * 0x10000 {
            return Err(());
        }

        let prd_slice: &mut [Prd] = unsafe {
            core::slice::from_raw_parts_mut(
                self.prdt.virt_addr().as_mut_ptr::<Prd>(),
                PRDT_BYTES / core::mem::size_of::<Prd>(),
            )
        };

        let mut remaining = len;
        let mut entries = 0usize;
        let mut cur_virt = VirtAddr::new(buf_ptr as u64);

        while remaining > 0 {
            if entries >= prd_slice.len() {
                return Err(());
            }

            let phys0 = virt_to_phys(cur_virt).ok_or(())?.as_u64();
            if phys0 > u32::MAX as u64 {
                return Err(());
            }

            // Coalesce physically-contiguous pages into up to 64KiB PRD entries, and
            // never allow a PRD to cross a 64KiB boundary.
            let mut seg_len = 0usize;
            let mut seg_virt = cur_virt;
            loop {
                if seg_len >= remaining || seg_len >= 0x10000 {
                    break;
                }

                let expected_phys = phys0 + seg_len as u64;
                let phys = virt_to_phys(seg_virt).ok_or(())?.as_u64();
                if phys != expected_phys || phys > u32::MAX as u64 {
                    break;
                }

                let page_off = (seg_virt.as_u64() & 0xFFF) as usize;
                let mut take = 0x1000 - page_off;
                take = take.min(remaining - seg_len);
                take = take.min(0x10000 - seg_len);

                let to_64k = 0x10000 - ((expected_phys as usize) & 0xFFFF);
                take = take.min(to_64k);

                if take == 0 {
                    break;
                }

                seg_len += take;
                seg_virt = seg_virt + take as u64;

                // If we stopped mid-page (because remaining ended or due to 64KiB boundary),
                // we can't coalesce further.
                if page_off + take != 0x1000 {
                    break;
                }
                // If we ended exactly at a 64KiB boundary, start a new PRD.
                if (expected_phys as usize + take) & 0xFFFF == 0 {
                    break;
                }
            }

            if seg_len == 0 {
                return Err(());
            }

            let count16: u16 = if seg_len == 0x10000 {
                0
            } else {
                seg_len as u16
            };
            prd_slice[entries] = Prd {
                addr: phys0 as u32,
                count: count16,
                flags: 0,
            };
            entries += 1;
            remaining -= seg_len;
            cur_virt = cur_virt + seg_len as u64;
        }

        if entries == 0 {
            return Err(());
        }
        ATA_PERF.prdt_histogram[AtaPerfCounters::prdt_bucket(entries)]
            .fetch_add(1, Ordering::Relaxed);
        if entries < prd_slice.len() {
            prd_slice[entries] = Prd::default();
        }
        prd_slice[entries - 1].flags = 1u16 << 15; // EOT
        self.program_prdt();
        Ok(())
    }

    fn dma_wait_done(&mut self) -> Result<(), ()> {
        // Wait primarily on IRQ, but also accept completion via ACTIVE clearing.
        let irq_flag = if self.id == 0 {
            &IDE_IRQ_PRIMARY
        } else {
            &IDE_IRQ_SECONDARY
        };
        irq_flag.store(false, Ordering::Release);

        let mut completed = false;
        let mut failed = false;
        let mut wait_iters = 0u64;
        for i in 0..POLL_SPINS {
            wait_iters += 1;
            let st = unsafe { self.bm_status.read() };
            if (st & 0x02) != 0 {
                failed = true;
                break;
            }
            // Completion: controller clears ACTIVE bit when the DMA engine is done.
            if (st & 0x01) == 0 {
                completed = true;
                break;
            }

            // If we have an IRQ pending, re-check immediately.
            if irq_flag.swap(false, Ordering::AcqRel) {
                continue;
            }

            // Interrupt status bit is a completion hint on many controllers:
            // immediately re-check ACTIVE/ERR in the next iteration.
            if (st & 0x04) != 0 {
                continue;
            }

            if i >= DMA_POLL_SPINS_BEFORE_HALT {
                // Sleep until next interrupt to avoid busy-waiting.
                crate::arch::x86_64::halt();
            } else {
                spin_loop();
            }
        }

        unsafe {
            // Stop bus master.
            let mut cmd = self.bm_cmd.read();
            cmd.set_bit(0, false);
            self.bm_cmd.write(cmd);

            // Clear interrupt/error.
            let st = self.bm_status.read();
            self.bm_status.write(st | 0x06);
        }

        // Acknowledge/clear the device interrupt status.
        let st = unsafe { self.status_register.read() };
        ATA_PERF
            .dma_wait_iterations_total
            .fetch_add(wait_iters, Ordering::Relaxed);
        if failed || !completed || st.get_bit(Status::ERR as usize) {
            return Err(());
        }
        Ok(())
    }

    fn run_dma_command_with_retry(
        &mut self,
        drive: u8,
        block: u64,
        sectors: usize,
        use_lba48: bool,
        read: bool,
    ) -> Result<(), ()> {
        for attempt in 0..=DMA_RETRY_COUNT {
            self.select_drive(drive)?;
            if use_lba48 {
                self.write_command_params_lba48(drive, block, sectors)?;
            } else {
                self.write_command_params(drive, block as u32, sectors)?;
            }

            unsafe {
                let mut cmd = self.bm_cmd.read();
                cmd.set_bit(3, read);
                cmd.set_bit(0, false);
                self.bm_cmd.write(cmd);
            }

            if read {
                ATA_PERF.dma_cmd_read.fetch_add(1, Ordering::Relaxed);
            } else {
                ATA_PERF.dma_cmd_write.fetch_add(1, Ordering::Relaxed);
            }

            self.write_command(if read {
                if use_lba48 {
                    Command::ReadDmaExt
                } else {
                    Command::ReadDma
                }
            } else if use_lba48 {
                Command::WriteDmaExt
            } else {
                Command::WriteDma
            })?;

            unsafe {
                let mut cmd = self.bm_cmd.read();
                cmd.set_bit(0, true);
                self.bm_cmd.write(cmd);
            }

            if self.dma_wait_done().is_ok() {
                return Ok(());
            }

            if attempt < DMA_RETRY_COUNT {
                ATA_PERF.dma_cmd_retry.fetch_add(1, Ordering::Relaxed);
            }
        }

        ATA_PERF.dma_cmd_fail.fetch_add(1, Ordering::Relaxed);
        Err(())
    }

    fn read_dma_bounce_window(&mut self, drive: u8, block: u32, bytes: usize) -> Result<(), ()> {
        if bytes == 0 || bytes > DMA_BUF_BYTES || (bytes % BLOCK_SIZE) != 0 {
            return Err(());
        }
        let mut remaining_sectors = bytes / BLOCK_SIZE;
        let mut current_block = block as u64;
        let mut off = 0usize;
        let use_lba48 = self
            .drive_lba48
            .get(drive as usize)
            .copied()
            .unwrap_or(false);
        let max_sectors = if use_lba48 {
            DMA_BUF_BYTES / BLOCK_SIZE
        } else {
            core::cmp::min(256, DMA_BUF_BYTES / BLOCK_SIZE)
        };

        while remaining_sectors > 0 {
            let sectors = remaining_sectors.min(max_sectors);
            let chunk = sectors * BLOCK_SIZE;
            self.setup_dma_prdt_for_phys(self.dma_buf.addr() + off as u64, chunk)?;
            self.run_dma_command_with_retry(drive, current_block, sectors, use_lba48, true)?;
            remaining_sectors -= sectors;
            current_block += sectors as u64;
            off += chunk;
        }
        Ok(())
    }

    fn write_dma_bounce_window(&mut self, drive: u8, block: u32, bytes: usize) -> Result<(), ()> {
        if bytes == 0 || bytes > DMA_BUF_BYTES || (bytes % BLOCK_SIZE) != 0 {
            return Err(());
        }
        let mut remaining_sectors = bytes / BLOCK_SIZE;
        let mut current_block = block as u64;
        let mut off = 0usize;
        let use_lba48 = self
            .drive_lba48
            .get(drive as usize)
            .copied()
            .unwrap_or(false);
        let max_sectors = if use_lba48 {
            DMA_BUF_BYTES / BLOCK_SIZE
        } else {
            core::cmp::min(256, DMA_BUF_BYTES / BLOCK_SIZE)
        };

        while remaining_sectors > 0 {
            let sectors = remaining_sectors.min(max_sectors);
            let chunk = sectors * BLOCK_SIZE;
            self.setup_dma_prdt_for_phys(self.dma_buf.addr() + off as u64, chunk)?;
            self.run_dma_command_with_retry(drive, current_block, sectors, use_lba48, false)?;
            remaining_sectors -= sectors;
            current_block += sectors as u64;
            off += chunk;
        }
        Ok(())
    }

    fn read_dma(&mut self, drive: u8, block: u32, buf: &mut [u8]) -> Result<(), ()> {
        if buf.is_empty() || (buf.len() % BLOCK_SIZE) != 0 {
            return Err(());
        }

        let mut remaining_sectors = buf.len() / BLOCK_SIZE;
        let mut current_block = block;
        let mut out_off = 0usize;
        let use_lba48 = self
            .drive_lba48
            .get(drive as usize)
            .copied()
            .unwrap_or(false);
        let max_sectors_direct = if use_lba48 { 0x10000 } else { 256 };
        let max_sectors_bounce = if use_lba48 {
            DMA_BUF_BYTES / BLOCK_SIZE
        } else {
            core::cmp::min(256, DMA_BUF_BYTES / BLOCK_SIZE)
        };
        // If direct PRDT setup fails once for this request, keep using bounce
        // to avoid repeatedly walking/validating the same fragmented mapping.
        let mut direct_setup_ok = true;

        while remaining_sectors > 0 {
            let mut use_bounce = false;
            let mut sectors = remaining_sectors.min(max_sectors_direct);
            let mut bytes = sectors * BLOCK_SIZE;

            if direct_setup_ok
                && self
                    .setup_dma_prdt_for_virt(unsafe { buf.as_mut_ptr().add(out_off) }, bytes)
                    .is_ok()
            {
                // direct PRDT ready
            } else {
                use_bounce = true;
                direct_setup_ok = false;
                sectors = remaining_sectors.min(max_sectors_bounce);
                bytes = sectors * BLOCK_SIZE;
                self.setup_dma_prdt_for_phys(self.dma_buf.addr(), bytes)?;
            }

            self.run_dma_command_with_retry(drive, current_block as u64, sectors, use_lba48, true)?;

            if use_bounce {
                buf[out_off..out_off + bytes].copy_from_slice(&self.dma_buf[..bytes]);
            }
            out_off += bytes;
            current_block += sectors as u32;
            remaining_sectors -= sectors;
        }
        Ok(())
    }

    fn write_dma(&mut self, drive: u8, block: u32, buf: &[u8]) -> Result<(), ()> {
        if buf.is_empty() || (buf.len() % BLOCK_SIZE) != 0 {
            return Err(());
        }

        let mut remaining_sectors = buf.len() / BLOCK_SIZE;
        let mut current_block = block;
        let mut in_off = 0usize;
        let use_lba48 = self
            .drive_lba48
            .get(drive as usize)
            .copied()
            .unwrap_or(false);
        let max_sectors_direct = if use_lba48 { 0x10000 } else { 256 };
        let max_sectors_bounce = if use_lba48 {
            DMA_BUF_BYTES / BLOCK_SIZE
        } else {
            core::cmp::min(256, DMA_BUF_BYTES / BLOCK_SIZE)
        };
        // If direct PRDT setup fails once for this request, keep using bounce
        // to avoid repeated virtual->physical walk overhead on fragmented buffers.
        let mut direct_setup_ok = true;

        while remaining_sectors > 0 {
            let mut sectors = remaining_sectors.min(max_sectors_direct);
            let mut bytes = sectors * BLOCK_SIZE;

            if direct_setup_ok
                && self
                    .setup_dma_prdt_for_virt(unsafe { buf.as_ptr().add(in_off) }, bytes)
                    .is_ok()
            {
                // direct PRDT ready
            } else {
                direct_setup_ok = false;
                sectors = remaining_sectors.min(max_sectors_bounce);
                bytes = sectors * BLOCK_SIZE;
                self.dma_buf[..bytes].copy_from_slice(&buf[in_off..in_off + bytes]);
                self.setup_dma_prdt_for_phys(self.dma_buf.addr(), bytes)?;
            }

            self.run_dma_command_with_retry(
                drive,
                current_block as u64,
                sectors,
                use_lba48,
                false,
            )?;

            in_off += bytes;
            current_block += sectors as u32;
            remaining_sectors -= sectors;
        }
        Ok(())
    }

    fn read_dma_resilient(&mut self, drive: u8, block: u32, buf: &mut [u8]) -> Result<(), ()> {
        match self.read_dma(drive, block, buf) {
            Ok(()) => {
                ATA_PERF
                    .read_bytes
                    .fetch_add(buf.len() as u64, Ordering::Relaxed);
                maybe_log_perf();
                Ok(())
            }
            Err(()) => {
                ATA_PERF.pio_fallback_count.fetch_add(1, Ordering::Relaxed);
                let res = crate::driver::disk::ata::read(self.id, drive, block, buf);
                if res.is_ok() {
                    ATA_PERF
                        .read_bytes
                        .fetch_add(buf.len() as u64, Ordering::Relaxed);
                }
                maybe_log_perf();
                res
            }
        }
    }

    fn write_dma_resilient(&mut self, drive: u8, block: u32, buf: &[u8]) -> Result<(), ()> {
        match self.write_dma(drive, block, buf) {
            Ok(()) => {
                ATA_PERF
                    .write_bytes
                    .fetch_add(buf.len() as u64, Ordering::Relaxed);
                maybe_log_perf();
                Ok(())
            }
            Err(()) => {
                ATA_PERF.pio_fallback_count.fetch_add(1, Ordering::Relaxed);
                let res = crate::driver::disk::ata::write(self.id, drive, block, buf);
                if res.is_ok() {
                    ATA_PERF
                        .write_bytes
                        .fetch_add(buf.len() as u64, Ordering::Relaxed);
                }
                maybe_log_perf();
                res
            }
        }
    }

    fn execute_merged_group(&mut self, reqs: &[usize]) -> Result<(), ()> {
        if reqs.is_empty() {
            return Err(());
        }

        let first = unsafe { &*(reqs[0] as *const AtaIoRequest) };
        if reqs.len() == 1 {
            return match first.op {
                AtaReqOp::Read => {
                    let buf = unsafe { slice::from_raw_parts_mut(first.buf_ptr, first.byte_len) };
                    self.read_dma_resilient(first.drive, first.lba_start, buf)
                }
                AtaReqOp::Write => {
                    let buf = unsafe {
                        slice::from_raw_parts(first.buf_ptr as *const u8, first.byte_len)
                    };
                    self.write_dma_resilient(first.drive, first.lba_start, buf)
                }
            };
        }

        let mut total_bytes = 0usize;
        for req_ptr in reqs {
            let req = unsafe { &*(*req_ptr as *const AtaIoRequest) };
            total_bytes += req.byte_len;
        }
        if total_bytes == 0 || total_bytes > DMA_BUF_BYTES || (total_bytes % BLOCK_SIZE) != 0 {
            return Err(());
        }

        match first.op {
            AtaReqOp::Write => {
                let mut off = 0usize;
                for req_ptr in reqs {
                    let req = unsafe { &*(*req_ptr as *const AtaIoRequest) };
                    let src =
                        unsafe { slice::from_raw_parts(req.buf_ptr as *const u8, req.byte_len) };
                    self.dma_buf[off..off + req.byte_len].copy_from_slice(src);
                    off += req.byte_len;
                }
                self.write_dma_bounce_window(first.drive, first.lba_start, total_bytes)?;
                ATA_PERF
                    .write_bytes
                    .fetch_add(total_bytes as u64, Ordering::Relaxed);
            }
            AtaReqOp::Read => {
                self.read_dma_bounce_window(first.drive, first.lba_start, total_bytes)?;
                let mut off = 0usize;
                for req_ptr in reqs {
                    let req = unsafe { &*(*req_ptr as *const AtaIoRequest) };
                    let dst = unsafe { slice::from_raw_parts_mut(req.buf_ptr, req.byte_len) };
                    dst.copy_from_slice(&self.dma_buf[off..off + req.byte_len]);
                    off += req.byte_len;
                }
                ATA_PERF
                    .read_bytes
                    .fetch_add(total_bytes as u64, Ordering::Relaxed);
            }
        }

        maybe_log_perf();
        Ok(())
    }

    fn identify_drive(&mut self, drive: u8) -> Result<IdentifyResponse, ()> {
        self.select_drive(drive)?;
        self.write_command_params(drive, 0, 1)?;
        self.write_command(Command::Identify)?;
        // If the drive doesn't exist, status often reads 0.
        if self.status() == 0 {
            return Ok(IdentifyResponse::None);
        }
        // If the command errored, don't try to read data.
        if self.status().get_bit(Status::ERR as usize) {
            return Ok(IdentifyResponse::None);
        }
        if self.poll(Status::DRQ, true).is_err() {
            // No DRQ -> no data; treat as unsupported so we can fall back to PIO.
            return Ok(IdentifyResponse::None);
        }
        let id = [(); 256].map(|_| unsafe { self.data_register.read() });

        // LBA48 support: word 83 bit 10.
        let lba48 = (id[83] & (1 << 10)) != 0;
        if (drive as usize) < self.drive_lba48.len() {
            self.drive_lba48[drive as usize] = lba48;
        }

        // Try to select the best supported transfer mode for performance.
        if let Some(mode) = self.best_xfer_mode_from_identify(&id) {
            let _ = self.set_transfer_mode(drive, mode);
        }

        if ENABLE_READ_LOOKAHEAD {
            // SET FEATURES: Enable read look-ahead.
            let _ = self.set_feature(drive, 0xAA);
        }
        if ENABLE_WRITE_CACHE {
            // SET FEATURES: Enable write cache.
            let _ = self.set_feature(drive, 0x02);
        }

        Ok(IdentifyResponse::Ata(id))
    }
}

lazy_static! {
    pub static ref BUSES: Mutex<Vec<Bus>> = Mutex::new(Vec::new());
    static ref ATA_PERF: AtaPerfCounters = AtaPerfCounters::new();
    static ref CHANNEL_QUEUES: [AtaChannelQueue; 2] = [
        AtaChannelQueue {
            inner: Mutex::new(AtaQueueState {
                active: false,
                pending: VecDeque::new(),
            }),
        },
        AtaChannelQueue {
            inner: Mutex::new(AtaQueueState {
                active: false,
                pending: VecDeque::new(),
            }),
        },
    ];
    static ref PERF_LOG_STATE: Mutex<PerfLogState> = Mutex::new(PerfLogState {
        last_ts: 0.0,
        last_read_bytes: 0,
        last_write_bytes: 0,
    });
}

fn maybe_log_perf() {
    #[cfg(debug_assertions)]
    {
        let now = crate::driver::timer::pit::uptime();
        let mut st = PERF_LOG_STATE.lock();
        if st.last_ts == 0.0 {
            st.last_ts = now;
            st.last_read_bytes = ATA_PERF.read_bytes.load(Ordering::Relaxed);
            st.last_write_bytes = ATA_PERF.write_bytes.load(Ordering::Relaxed);
            return;
        }
        let dt = now - st.last_ts;
        if dt < 2.0 {
            return;
        }

        let read_now = ATA_PERF.read_bytes.load(Ordering::Relaxed);
        let write_now = ATA_PERF.write_bytes.load(Ordering::Relaxed);
        let read_delta = read_now.saturating_sub(st.last_read_bytes);
        let write_delta = write_now.saturating_sub(st.last_write_bytes);
        let read_mibs = (read_delta as f64) / (1024.0 * 1024.0) / dt;
        let write_mibs = (write_delta as f64) / (1024.0 * 1024.0) / dt;
        let merged = ATA_PERF.queue_merged_groups.load(Ordering::Relaxed);
        let enq = ATA_PERF.queue_enqueued.load(Ordering::Relaxed);
        let retries = ATA_PERF.dma_cmd_retry.load(Ordering::Relaxed);
        let fallbacks = ATA_PERF.pio_fallback_count.load(Ordering::Relaxed);

        println!(
            "ATA-DMA perf r={:.2}MiB/s w={:.2}MiB/s merge={}/{} retry={} pio_fallback={}",
            read_mibs, write_mibs, merged, enq, retries, fallbacks
        );

        st.last_ts = now;
        st.last_read_bytes = read_now;
        st.last_write_bytes = write_now;
    }
}

fn mark_request(req_ptr: usize, ok: bool) {
    let req = unsafe { &*(req_ptr as *const AtaIoRequest) };
    req.status
        .store(if ok { REQ_DONE } else { REQ_FAILED }, Ordering::Release);
}

fn wait_for_request(req: &AtaIoRequest) -> Result<(), ()> {
    let mut spins = 0usize;
    loop {
        match req.status.load(Ordering::Acquire) {
            REQ_DONE => return Ok(()),
            REQ_FAILED => return Err(()),
            _ => {
                if spins < DMA_POLL_SPINS_BEFORE_HALT {
                    spin_loop();
                    spins += 1;
                } else {
                    crate::arch::x86_64::halt();
                }
            }
        }
    }
}

fn enqueue_request(bus: u8, req: &mut AtaIoRequest) -> Result<bool, ()> {
    let Some(queue) = CHANNEL_QUEUES.get(bus as usize) else {
        return Err(());
    };
    let mut q = queue.inner.lock();
    q.pending.push_back(req as *mut AtaIoRequest as usize);
    ATA_PERF.queue_enqueued.fetch_add(1, Ordering::Relaxed);
    if q.active {
        Ok(false)
    } else {
        q.active = true;
        Ok(true)
    }
}

fn dequeue_merged_group(bus: u8) -> Option<Vec<usize>> {
    let queue = CHANNEL_QUEUES.get(bus as usize)?;
    let mut q = queue.inner.lock();
    let first = match q.pending.pop_front() {
        Some(req) => req,
        None => {
            q.active = false;
            return None;
        }
    };

    let first_req = unsafe { &*(first as *const AtaIoRequest) };
    let op = first_req.op;
    let drive = first_req.drive;
    let mut next_lba = first_req.lba_start as u64 + first_req.sectors as u64;
    let mut total_bytes = first_req.byte_len;
    let mut group = Vec::with_capacity(4);
    group.push(first);

    while group.len() < MAX_MERGED_REQS {
        let Some(next_ptr) = q.pending.front().copied() else {
            break;
        };
        let next = unsafe { &*(next_ptr as *const AtaIoRequest) };
        if next.op != op || next.drive != drive || next.lba_start as u64 != next_lba {
            break;
        }
        if total_bytes + next.byte_len > MAX_MERGED_BYTES {
            break;
        }
        q.pending.pop_front();
        group.push(next_ptr);
        total_bytes += next.byte_len;
        next_lba += next.sectors as u64;
    }

    if group.len() > 1 {
        ATA_PERF.queue_merged_groups.fetch_add(1, Ordering::Relaxed);
        ATA_PERF
            .queue_merged_reqs
            .fetch_add((group.len() - 1) as u64, Ordering::Relaxed);
    }

    Some(group)
}

fn process_group(bus: u8, group: &[usize]) {
    if group.is_empty() {
        return;
    }

    let mut buses = BUSES.lock();
    let Some(dev) = buses.get_mut(bus as usize) else {
        for req in group {
            mark_request(*req, false);
        }
        return;
    };

    if dev.execute_merged_group(group).is_ok() {
        for req in group {
            mark_request(*req, true);
        }
        return;
    }

    // Failure containment: process each request independently.
    for req_ptr in group {
        let single = core::slice::from_ref(req_ptr);
        let ok = dev.execute_merged_group(single).is_ok();
        mark_request(*req_ptr, ok);
    }
}

fn drain_channel_queue(bus: u8) {
    while let Some(group) = dequeue_merged_group(bus) {
        process_group(bus, &group);
    }
}

struct IdeController {
    bmide_base: u16,
    max_udma_mode: u8,
}

fn max_udma_for_controller(vendor_id: u16, device_id: u16) -> u8 {
    match (vendor_id, device_id) {
        // PIIX3 / PIIX4.
        (0x8086, 0x7010) | (0x8086, 0x7111) => 2,
        _ => 6,
    }
}

fn find_bmide_controller() -> Option<IdeController> {
    let devs = crate::sys::pci::list();
    for d in devs.iter() {
        if d.class == 0x01 && d.subclass == 0x01 {
            // IDE controller, BAR4 is bus master IDE.
            let bar4 = d.base_addresses[4];
            if (bar4 & 1) == 0 {
                continue;
            }
            let base = (bar4 as u16) & 0xFFF0;
            if base != 0 {
                // Enable bus mastering (write to config space).
                let cfg = DeviceConfig::new(d.bus, d.device, d.function);
                cfg.enable_bus_mastering();
                return Some(IdeController {
                    bmide_base: base,
                    max_udma_mode: max_udma_for_controller(d.vendor_id, d.device_id),
                });
            }
        }
    }
    None
}

pub fn init() {
    let _ = crate::arch::x86_64::idt::register_irq_handler(14, on_irq_primary);
    let _ = crate::arch::x86_64::idt::register_irq_handler(15, on_irq_secondary);
    let Some(ctrl) = find_bmide_controller() else {
        crate::driver::disk::ata::init();
        return;
    };

    {
        let mut buses = BUSES.lock();
        // Primary channel bus master regs at BAR4 + 0, secondary at BAR4 + 8.
        buses.push(Bus::new(
            0,
            0x1F0,
            0x3F6,
            ctrl.bmide_base,
            ctrl.max_udma_mode,
        ));
        buses.push(Bus::new(
            1,
            0x170,
            0x376,
            ctrl.bmide_base + 8,
            ctrl.max_udma_mode,
        ));
    }

    let time = crate::driver::timer::pit::uptime();
    println!(
        "\x1b[93m[{:.6}]\x1b[0m ATA-DMA mode queue={} dma_buf={}KiB",
        time,
        if ENABLE_COOP_QUEUE { "on" } else { "off" },
        DMA_BUF_BYTES / 1024
    );
    let drives = list();
    for drive in drives {
        println!(
            "\x1b[93m[{:.6}]\x1b[0m ATA-DMA {}:{} {}",
            time, drive.bus, drive.dsk, drive
        );
        mount_ata_with_impl(drive.bus, drive.dsk, AtaImpl::Dma);
    }

    // If we couldn't enumerate any drives, fall back to PIO so boot can proceed.
    #[allow(static_mut_refs)]
    if unsafe { crate::driver::disk::BLOCK_DEVICE.is_none() } {
        crate::driver::disk::ata::init();
    }
}

#[derive(Clone, Debug)]
pub struct Drive {
    pub bus: u8,
    pub dsk: u8,
    model: String,
    serial: String,
    block_count: u32,
}

impl Drive {
    pub fn open(bus: u8, dsk: u8) -> Option<Self> {
        let mut buses = BUSES.lock();
        let res = buses[bus as usize].identify_drive(dsk);
        if let Ok(IdentifyResponse::Ata(res)) = res {
            let buf = res.map(u16::to_be_bytes).concat();
            let model: String = String::from_utf8_lossy(&buf[54..94]).trim().into();
            let serial: String = String::from_utf8_lossy(&buf[20..40]).trim().into();
            let block_count = u32::from_be_bytes(buf[120..124].try_into().unwrap()).rotate_left(16);
            Some(Self {
                bus,
                dsk,
                model,
                serial,
                block_count,
            })
        } else {
            None
        }
    }

    pub const fn block_size(&self) -> u32 {
        BLOCK_SIZE as u32
    }

    pub fn block_count(&self) -> u32 {
        self.block_count
    }

    fn humanized_size(&self) -> (usize, String) {
        let size = self.block_size() as usize;
        let count = self.block_count() as usize;
        let bytes = size * count;
        if bytes >> 20 < 1000 {
            (bytes >> 20, String::from("MB"))
        } else {
            (bytes >> 30, String::from("GB"))
        }
    }
}

impl fmt::Display for Drive {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let (size, unit) = self.humanized_size();
        write!(f, "{} {} ({} {})", self.model, self.serial, size, unit)
    }
}

pub fn list() -> Vec<Drive> {
    let mut res = Vec::new();
    for bus in 0..2 {
        for dsk in 0..2 {
            if let Some(drive) = Drive::open(bus, dsk) {
                res.push(drive)
            }
        }
    }
    res
}

pub fn read(bus: u8, drive: u8, block: u32, buf: &mut [u8]) -> Result<(), ()> {
    if buf.is_empty() || (buf.len() % BLOCK_SIZE) != 0 {
        return Err(());
    }
    if !ENABLE_COOP_QUEUE {
        let mut buses = BUSES.lock();
        let Some(dev) = buses.get_mut(bus as usize) else {
            return Err(());
        };
        return dev.read_dma_resilient(drive, block, buf);
    }

    let mut req = AtaIoRequest::new_read(drive, block, buf);
    let became_owner = enqueue_request(bus, &mut req)?;
    if became_owner {
        drain_channel_queue(bus);
    }
    wait_for_request(&req)
}

pub fn write(bus: u8, drive: u8, block: u32, buf: &[u8]) -> Result<(), ()> {
    if buf.is_empty() || (buf.len() % BLOCK_SIZE) != 0 {
        return Err(());
    }
    if !ENABLE_COOP_QUEUE {
        let mut buses = BUSES.lock();
        let Some(dev) = buses.get_mut(bus as usize) else {
            return Err(());
        };
        return dev.write_dma_resilient(drive, block, buf);
    }

    let mut req = AtaIoRequest::new_write(drive, block, buf);
    let became_owner = enqueue_request(bus, &mut req)?;
    if became_owner {
        drain_channel_queue(bus);
    }
    wait_for_request(&req)
}
