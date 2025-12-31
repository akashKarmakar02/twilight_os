use crate::driver::disk::mount_ata;
use crate::println;
use alloc::string::String;
use alloc::vec::Vec;
use bit_field::BitField;
use core::convert::TryInto;
use core::fmt;
use core::hint::spin_loop;
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};
// Information Technology
// AT Attachment with Packet Interface Extension (ATA/ATAPI-4)
// (1998)

pub const BLOCK_SIZE: usize = 512;

// Keep track of the last selected bus and drive pair to speed up operations
pub static LAST_SELECTED: Mutex<Option<(u8, u8)>> = Mutex::new(None);

#[repr(u16)]
#[derive(Debug, Clone, Copy)]
enum Command {
    Read = 0x20,
    Write = 0x30,
    Identify = 0xEC,
    SetFeatures = 0xEF,
}

enum IdentifyResponse {
    Ata([u16; 256]),
    Atapi,
    Sata,
    None,
}

#[allow(dead_code)]
#[repr(usize)]
#[derive(Debug, Clone, Copy)]
enum Status {
    ERR = 0,  // Error
    IDX = 1,  // (obsolete)
    CORR = 2, // (obsolete)
    DRQ = 3,  // Data Request
    DSC = 4,  // (command dependant)
    DF = 5,   // (command dependant)
    DRDY = 6, // Device Ready
    BSY = 7,  // Busy
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Bus {
    id: u8,
    irq: u8,

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
    drive_blockess_register: PortReadOnly<u8>,
}

impl Bus {
    pub fn new(id: u8, io_base: u16, ctrl_base: u16, irq: u8) -> Self {
        Self {
            id,
            irq,
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
            drive_blockess_register: PortReadOnly::new(ctrl_base + 1),
        }
    }

    fn check_floating_bus(&mut self) -> Result<(), ()> {
        match self.status() {
            0xFF | 0x7F => Err(()),
            _ => Ok(()),
        }
    }

    fn wait(&mut self, ns: u64) {
        crate::driver::timer::wait(ns);
    }

    fn clear_interrupt(&mut self) -> u8 {
        unsafe { self.status_register.read() }
    }

    fn status(&mut self) -> u8 {
        unsafe { self.alternate_status_register.read() }
    }

    fn lba1(&mut self) -> u8 {
        unsafe { self.lba1_register.read() }
    }

    fn lba2(&mut self) -> u8 {
        unsafe { self.lba2_register.read() }
    }

    fn read_data(&mut self) -> u16 {
        unsafe { self.data_register.read() }
    }

    fn write_data(&mut self, data: u16) {
        unsafe { self.data_register.write(data) }
    }

    fn is_error(&mut self) -> bool {
        self.status().get_bit(Status::ERR as usize)
    }

    fn poll(&mut self, bit: Status, val: bool) -> Result<(), ()> {
        let start = crate::driver::timer::pit::uptime();
        while self.status().get_bit(bit as usize) != val {
            if crate::driver::timer::pit::uptime() - start > 1.0 {
                println!("ATA hanged while polling {:?} bit in status register", bit);
                self.debug();
                return Err(());
            }
            spin_loop();
        }
        Ok(())
    }

    fn select_drive(&mut self, drive: u8) -> Result<(), ()> {
        self.poll(Status::BSY, false)?;
        self.poll(Status::DRQ, false)?;

        // Skip the rest if this drive was already selected
        if *LAST_SELECTED.lock() == Some((self.id, drive)) {
            return Ok(());
        } else {
            *LAST_SELECTED.lock() = Some((self.id, drive));
        }

        unsafe {
            // Bit 4 => DEV
            // Bit 5 => 1
            // Bit 7 => 1
            self.drive_register.write(0xA0 | (drive << 4))
        }
        crate::driver::timer::wait(400); // Wait at least 400 ns
        self.poll(Status::BSY, false)?;
        self.poll(Status::DRQ, false)?;
        Ok(())
    }

    fn write_command_params(&mut self, drive: u8, block: u32, sectors: u8) -> Result<(), ()> {
        let lba = true;
        let mut bytes = block.to_le_bytes();
        bytes[3].set_bit(4, drive > 0);
        bytes[3].set_bit(5, true);
        bytes[3].set_bit(6, lba);
        bytes[3].set_bit(7, true);
        unsafe {
            self.sector_count_register.write(sectors.max(1));
            self.lba0_register.write(bytes[0]);
            self.lba1_register.write(bytes[1]);
            self.lba2_register.write(bytes[2]);
            self.drive_register.write(bytes[3]);
        }
        Ok(())
    }

    fn write_command(&mut self, cmd: Command) -> Result<(), ()> {
        unsafe { self.command_register.write(cmd as u8) }
        self.wait(120); // Wait at least 400 ns
        self.status(); // Ignore results of first read
        self.clear_interrupt();
        if self.status() == 0 {
            // Drive does not exist
            return Err(());
        }
        if self.is_error() {
            //println!("ATA {:?} command errored", cmd);
            //self.debug();
            return Err(());
        }
        self.poll(Status::BSY, false)?;
        match cmd {
            Command::Read | Command::Write | Command::Identify => {
                self.poll(Status::DRQ, true)?;
            }
            Command::SetFeatures => {
                // Do nothing — no DRQ expected
            }
        }

        Ok(())
    }

    fn setup_pio(&mut self, drive: u8, block: u32, sectors: u8) -> Result<(), ()> {
        self.select_drive(drive)?;
        self.write_command_params(drive, block, sectors)?;
        Ok(())
    }

    fn set_pio_mode(&mut self, drive: u8, mode: u8) -> Result<(), ()> {
        // According to ATA/ATAPI-4 spec:
        // Set Features (0xEF) with subcommand 0x03 and transfer mode 0x08 + mode_num
        self.select_drive(drive)?;
        self.poll(Status::BSY, false)?;

        unsafe {
            self.features_register.write(0x03); // subcommand: Set Transfer Mode
            self.sector_count_register.write(0x08 | mode); // 0x08 + mode (for PIO)
            self.lba0_register.write(0);
            self.lba1_register.write(0);
            self.lba2_register.write(0);
        }

        self.write_command(Command::SetFeatures)?;
        self.poll(Status::BSY, false)?;
        Ok(())
    }

    fn read(&mut self, drive: u8, block: u32, buf: &mut [u8]) -> Result<(), ()> {
        if buf.is_empty() || buf.len() % BLOCK_SIZE != 0 {
            return Err(());
        }
        let mut remaining_sectors = buf.len() / BLOCK_SIZE;
        let mut current_block = block;
        let mut offset = 0usize;

        while remaining_sectors > 0 {
            let sectors = remaining_sectors.min(255);
            self.setup_pio(drive, current_block, sectors as u8)?;
            self.write_command(Command::Read)?;

            for sector_idx in 0..sectors {
                if sector_idx > 0 {
                    self.poll(Status::BSY, false)?;
                    self.poll(Status::DRQ, true)?;
                }
                for chunk in buf[offset..offset + BLOCK_SIZE].chunks_mut(2) {
                    let data = self.read_data().to_le_bytes();
                    chunk.clone_from_slice(&data);
                }
                offset += BLOCK_SIZE;
            }

            self.poll(Status::BSY, false)?;
            self.poll(Status::DRQ, false)?;
            current_block += sectors as u32;
            remaining_sectors -= sectors;
        }

        if self.is_error() {
            println!("ATA read: data error");
            self.debug();
            Err(())
        } else {
            Ok(())
        }
    }

    fn write(&mut self, drive: u8, block: u32, buf: &[u8]) -> Result<(), ()> {
        if buf.is_empty() || buf.len() % BLOCK_SIZE != 0 {
            return Err(());
        }
        let mut remaining_sectors = buf.len() / BLOCK_SIZE;
        let mut current_block = block;
        let mut offset = 0usize;

        while remaining_sectors > 0 {
            let sectors = remaining_sectors.min(255);
            self.setup_pio(drive, current_block, sectors as u8)?;
            self.write_command(Command::Write)?;

            for sector_idx in 0..sectors {
                if sector_idx > 0 {
                    self.poll(Status::BSY, false)?;
                    self.poll(Status::DRQ, true)?;
                }
                for chunk in buf[offset..offset + BLOCK_SIZE].chunks(2) {
                    let data = u16::from_le_bytes(chunk.try_into().unwrap());
                    self.write_data(data);
                }
                offset += BLOCK_SIZE;
            }

            self.poll(Status::BSY, false)?;
            self.poll(Status::DRQ, false)?;
            current_block += sectors as u32;
            remaining_sectors -= sectors;
        }

        if self.is_error() {
            println!("ATA write: data error");
            self.debug();
            Err(())
        } else {
            Ok(())
        }
    }

    fn identify_drive(&mut self, drive: u8) -> Result<IdentifyResponse, ()> {
        if self.check_floating_bus().is_err() {
            return Ok(IdentifyResponse::None);
        }
        self.select_drive(drive)?;
        self.write_command_params(drive, 0, 1)?;
        if self.write_command(Command::Identify).is_err() {
            if self.status() == 0 {
                return Ok(IdentifyResponse::None);
            } else {
                return Err(());
            }
        }
        match (self.lba1(), self.lba2()) {
            (0x00, 0x00) => Ok(IdentifyResponse::Ata([(); 256].map(|_| self.read_data()))),
            (0x14, 0xEB) => Ok(IdentifyResponse::Atapi),
            (0x3C, 0xC3) => Ok(IdentifyResponse::Sata),
            (_, _) => Err(()),
        }
    }

    #[allow(dead_code)]
    fn reset(&mut self) {
        unsafe {
            self.control_register.write(4); // Set SRST bit
            self.wait(5); // Wait at least 5 ns
            self.control_register.write(0); // Then clear it
            self.wait(200); // Wait at least 2 ms
        }
    }

    #[allow(dead_code)]
    fn debug(&mut self) {
        unsafe {
            println!(
                "ATA status register: 0b{:08b} <BSY|DRDY|#|#|DRQ|#|#|ERR>",
                self.alternate_status_register.read()
            );
            println!(
                "ATA error register:  0b{:08b} <#|#|#|#|#|ABRT|#|#>",
                self.error_register.read()
            );
        }
    }
}

lazy_static! {
    pub static ref BUSES: Mutex<Vec<Bus>> = Mutex::new(Vec::new());
}

pub fn init() {
    {
        let mut buses = BUSES.lock();
        buses.push(Bus::new(0, 0x1F0, 0x3F6, 14));
        buses.push(Bus::new(1, 0x170, 0x376, 15));
    }

    let time = crate::driver::timer::pit::uptime();
    let drives = list();

    for drive in drives {
        println!(
            "\x1b[93m[{:.6}]\x1b[0m ATA {}:{} {}",
            time, drive.bus, drive.dsk, drive
        );
        mount_ata(drive.bus, drive.dsk);
    }
}

#[derive(Clone, Debug)]
pub struct Drive {
    pub bus: u8,
    pub dsk: u8,
    model: String,
    serial: String,
    block_count: u32,
    block_index: u32,
}

impl Drive {
    pub fn size() -> usize {
        BLOCK_SIZE
    }

    pub fn open(bus: u8, dsk: u8) -> Option<Self> {
        let mut buses = BUSES.lock();
        let res = buses[bus as usize].identify_drive(dsk);

        if let Ok(IdentifyResponse::Ata(res)) = res {
            let buf = res.map(u16::to_be_bytes).concat();
            let model: String = String::from_utf8_lossy(&buf[54..94]).trim().into();
            let serial: String = String::from_utf8_lossy(&buf[20..40]).trim().into();
            let block_count = u32::from_be_bytes(buf[120..124].try_into().unwrap()).rotate_left(16);
            let block_index = 0;

            let _ = buses[bus as usize].set_pio_mode(dsk, 4);

            Some(Self {
                bus,
                dsk,
                model,
                serial,
                block_count,
                block_index,
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

pub trait FileIO {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()>;
    fn write(&mut self, buf: &[u8]) -> Result<usize, ()>;
    fn close(&mut self);
    fn poll(&mut self, event: IO) -> bool;
}

#[derive(Clone, Copy)]
pub enum IO {
    Read,
    Write,
}

impl FileIO for Drive {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        if self.block_index == self.block_count {
            return Ok(0);
        }

        let mut buses = BUSES.lock();
        let _ = buses[self.bus as usize].read(self.dsk, self.block_index, buf);
        let n = buf.len();
        self.block_index += 1;
        Ok(n)
    }

    fn write(&mut self, _buf: &[u8]) -> Result<usize, ()> {
        Err(())
    }

    fn close(&mut self) {}

    fn poll(&mut self, event: IO) -> bool {
        match event {
            IO::Read => true,
            IO::Write => false,
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
    buses[bus as usize].read(drive, block, buf)
}

pub fn write(bus: u8, drive: u8, block: u32, buf: &[u8]) -> Result<(), ()> {
    let mut buses = BUSES.lock();
    buses[bus as usize].write(drive, block, buf)
}
