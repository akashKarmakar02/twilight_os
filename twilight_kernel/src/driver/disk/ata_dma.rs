use crate::driver::disk::AtaImpl;
use crate::driver::disk::mount_ata_with_impl;
use crate::println;
use crate::sys::memory::phys::PhysBuf;
use crate::sys::pci::DeviceConfig;
use alloc::string::String;
use alloc::vec::Vec;
use bit_field::BitField;
use core::convert::TryInto;
use core::fmt;
use core::hint::spin_loop;
use core::sync::atomic::{AtomicBool, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

// ATA DMA (Bus Master IDE) driver.
// Minimal, polling-based implementation to keep the existing PIO driver intact.

pub const BLOCK_SIZE: usize = 512;
// Bigger DMA buffer reduces per-command overhead significantly.
// With LBA48 DMA EXT we can exceed 256 sectors/command; LBA28 is still capped at 256.
const DMA_BUF_BYTES: usize = 512 * 512; // 256KiB
const PRDT_BYTES: usize = 4096;
const POLL_SPINS: usize = 5_000_000;

static IDE_IRQ_PRIMARY: AtomicBool = AtomicBool::new(false);
static IDE_IRQ_SECONDARY: AtomicBool = AtomicBool::new(false);

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
}

impl Bus {
    // PIIX3/PIIX4 legacy IDE typically tops out at UDMA2 (ATA/33).
    // Cap the negotiated mode for broad compatibility.
    const MAX_UDMA_MODE: u8 = 2;

    pub fn new(id: u8, io_base: u16, ctrl_base: u16, bm_base: u16) -> Self {
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

            prdt: PhysBuf::new(PRDT_BYTES),
            dma_buf: PhysBuf::new(DMA_BUF_BYTES),
            drive_lba48: [false; 2],
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

    fn best_xfer_mode_from_identify(id: &[u16; 256]) -> Option<u8> {
        // Word 88: Ultra DMA modes (bits 0..7 supported, 8..15 active)
        let udma = id[88];
        let supported_udma = (udma & 0x00FF) as u8;
        if supported_udma != 0 {
            for mode in (0..=6u8).rev() {
                if (supported_udma & (1u8 << mode)) != 0 {
                    let mode = core::cmp::min(mode, Self::MAX_UDMA_MODE);
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

    fn setup_dma_prdt(&mut self, len: usize) -> Result<(), ()> {
        if len == 0 || len > DMA_BUF_BYTES {
            return Err(());
        }

        // Build PRDT entries, splitting at 64KiB boundaries if needed.
        let base = self.dma_buf.addr();
        let mut remaining = len;
        let mut offset = 0usize;
        let mut entries: usize = 0;

        let prd_slice = unsafe {
            core::slice::from_raw_parts_mut(
                self.prdt.virt_addr().as_mut_ptr::<Prd>(),
                PRDT_BYTES / core::mem::size_of::<Prd>(),
            )
        };
        for prd in prd_slice.iter_mut() {
            *prd = Prd::default();
        }

        while remaining > 0 {
            if entries >= prd_slice.len() {
                return Err(());
            }

            let phys = base + offset as u64;
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
        prd_slice[entries - 1].flags = 1u16 << 15; // EOT

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

        for _ in 0..POLL_SPINS {
            let st = unsafe { self.bm_status.read() };
            if (st & 0x02) != 0 {
                // error
                return Err(());
            }
            // Completion: controller clears ACTIVE bit when the DMA engine is done.
            if (st & 0x01) == 0 {
                break;
            }

            // If we have an IRQ pending, re-check immediately.
            if irq_flag.swap(false, Ordering::AcqRel) {
                continue;
            }

            // Sleep until next interrupt to avoid busy-waiting.
            crate::arch::x86_64::halt();
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
        unsafe {
            let _ = self.status_register.read();
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
        let max_sectors = if use_lba48 {
            DMA_BUF_BYTES / BLOCK_SIZE
        } else {
            core::cmp::min(256, DMA_BUF_BYTES / BLOCK_SIZE)
        };

        while remaining_sectors > 0 {
            let sectors = remaining_sectors.min(max_sectors);
            let bytes = sectors * BLOCK_SIZE;

            self.select_drive(drive)?;
            if use_lba48 {
                self.write_command_params_lba48(drive, current_block as u64, sectors)?;
            } else {
                self.write_command_params(drive, current_block, sectors)?;
            }
            self.setup_dma_prdt(bytes)?;

            unsafe {
                // Set direction: 1 = read from disk to memory.
                let mut cmd = self.bm_cmd.read();
                cmd.set_bit(3, true);
                cmd.set_bit(0, false);
                self.bm_cmd.write(cmd);
            }

            self.write_command(if use_lba48 {
                Command::ReadDmaExt
            } else {
                Command::ReadDma
            })?;

            unsafe {
                // Start bus master.
                let mut cmd = self.bm_cmd.read();
                cmd.set_bit(0, true);
                self.bm_cmd.write(cmd);
            }

            self.dma_wait_done()?;

            buf[out_off..out_off + bytes].copy_from_slice(&self.dma_buf[..bytes]);
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
        let max_sectors = if use_lba48 {
            DMA_BUF_BYTES / BLOCK_SIZE
        } else {
            core::cmp::min(256, DMA_BUF_BYTES / BLOCK_SIZE)
        };

        while remaining_sectors > 0 {
            let sectors = remaining_sectors.min(max_sectors);
            let bytes = sectors * BLOCK_SIZE;

            self.dma_buf[..bytes].copy_from_slice(&buf[in_off..in_off + bytes]);

            self.select_drive(drive)?;
            if use_lba48 {
                self.write_command_params_lba48(drive, current_block as u64, sectors)?;
            } else {
                self.write_command_params(drive, current_block, sectors)?;
            }
            self.setup_dma_prdt(bytes)?;

            unsafe {
                // Set direction: 0 = write from memory to disk.
                let mut cmd = self.bm_cmd.read();
                cmd.set_bit(3, false);
                cmd.set_bit(0, false);
                self.bm_cmd.write(cmd);
            }

            self.write_command(if use_lba48 {
                Command::WriteDmaExt
            } else {
                Command::WriteDma
            })?;

            unsafe {
                let mut cmd = self.bm_cmd.read();
                cmd.set_bit(0, true);
                self.bm_cmd.write(cmd);
            }

            self.dma_wait_done()?;

            in_off += bytes;
            current_block += sectors as u32;
            remaining_sectors -= sectors;
        }
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
        if let Some(mode) = Self::best_xfer_mode_from_identify(&id) {
            if self.set_transfer_mode(drive, mode).is_ok() {}
        }

        Ok(IdentifyResponse::Ata(id))
    }
}

lazy_static! {
    pub static ref BUSES: Mutex<Vec<Bus>> = Mutex::new(Vec::new());
}

fn find_bmide_base() -> Option<u16> {
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
                let mut cfg = DeviceConfig::new(d.bus, d.device, d.function);
                cfg.enable_bus_mastering();
                return Some(base);
            }
        }
    }
    None
}

pub fn init() {
    let _ = crate::arch::x86_64::idt::register_irq_handler(14, on_irq_primary);
    let _ = crate::arch::x86_64::idt::register_irq_handler(15, on_irq_secondary);
    let Some(bmide) = find_bmide_base() else {
        crate::driver::disk::ata::init();
        return;
    };

    {
        let mut buses = BUSES.lock();
        // Primary channel bus master regs at bmide + 0, secondary at bmide + 8.
        buses.push(Bus::new(0, 0x1F0, 0x3F6, bmide));
        buses.push(Bus::new(1, 0x170, 0x376, bmide + 8));
    }

    let time = crate::driver::timer::pit::uptime();
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
    let mut buses = BUSES.lock();
    buses[bus as usize].read_dma(drive, block, buf)
}

pub fn write(bus: u8, drive: u8, block: u32, buf: &[u8]) -> Result<(), ()> {
    let mut buses = BUSES.lock();
    buses[bus as usize].write_dma(drive, block, buf)
}
