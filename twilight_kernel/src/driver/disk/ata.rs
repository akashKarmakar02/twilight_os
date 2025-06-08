use crate::driver::disk::BlockDevice;
use alloc::string::{String, ToString};
use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};

pub struct Ata {
    pub base_port: u16,
    pub sector_count: u64,
    pub sector_size: usize,
    pub model: Option<String>,
}

enum Command {
    Read = 0x20,
    Write = 0x30,
    Identify = 0xEC,
}

#[allow(dead_code)]
impl Ata {
    fn inb(port: u16) -> u8 {
        let mut port = PortReadOnly::new(port);
        unsafe { port.read() }
    }

    fn outb(port: u16, data: u8) {
        let mut port = PortWriteOnly::new(port);
        unsafe { port.write(data) };
    }

    fn inw(port: u16) -> u16 {
        let mut port = PortReadOnly::new(port);
        unsafe { port.read() }
    }

    fn outw(port: u16, data: u16) {
        let mut port = PortWriteOnly::new(port);
        unsafe { port.write(data) };
    }

    pub fn detect(&mut self) -> bool {
        let mut port_data = Port::<u16>::new(self.base_port);
        let mut port_status = Port::<u8>::new(self.base_port + 7);
        let mut port_drive = Port::<u8>::new(self.base_port + 6);

        unsafe { port_drive.write(0xA0) }; // Master drive
        unsafe { port_status.write(Command::Identify as u8) };

        if unsafe { port_status.read() } == 0 {
            return false;
        }

        if self.wait_ready().is_err() {
            return false;
        }

        let mut identify_data = [0u16; 256];
        for word in identify_data.iter_mut() {
            *word = unsafe { port_data.read() };
        }

        self.sector_count = ((identify_data[61] as u64) << 16) | (identify_data[60] as u64);

        // Extract model name
        let mut model_bytes = [0u8; 40];
        for i in 0..20 {
            let word = identify_data[27 + i];
            model_bytes[i * 2] = (word >> 8) as u8;
            model_bytes[i * 2 + 1] = word as u8;
        }
        self.model = String::from_utf8(model_bytes.to_vec())
            .ok()
            .map(|s| s.trim().to_string());

        true
    }

    pub fn get_ata_model_name(&self) -> Result<String, &'static str> {
        match &self.model {
            Some(s) => Ok(s.clone()),
            None => Err("ATA model not detected yet"),
        }
    }

    fn wait_ready(&self) -> Result<(), &'static str> {
        for _ in 0..1000000 {
            let status = Self::inb(self.base_port + 7);
            let bsy = status & (1 << Status::BSY as u8);
            let drq = status & (1 << Status::DRQ as u8);
            if bsy == 0 && drq != 0 {
                return Ok(());
            }
        }
        Err("Device not ready (BSY or DRQ timeout)")
    }
}

#[allow(dead_code)]
enum IdentifyResponse {
    Ata([u16; 256]),
    Atapi,
    Sata,
    None
}

#[allow(dead_code)]
#[repr(usize)]
#[derive(Debug, Clone, Copy)]
enum Status {
    ERR  = 0, // Error
    IDX  = 1, // (obsolete)
    CORR = 2, // (obsolete)
    DRQ  = 3, // Data Request
    DSC  = 4, // (command dependant)
    DF   = 5, // (command dependant)
    DRDY = 6, // Device Ready
    BSY  = 7, // Busy
}

impl BlockDevice for Ata {
    fn read_block(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), &'static str> {
        if buffer.len() < self.sector_size {
            return Err("Buffer too small");
        }

        Self::outb(self.base_port + 6, 0xE0 | ((lba >> 24) & 0x0F) as u8);
        Self::outb(self.base_port + 2, 1);
        Self::outb(self.base_port + 3, lba as u8);
        Self::outb(self.base_port + 4, (lba >> 8) as u8);
        Self::outb(self.base_port + 5, (lba >> 16) as u8);
        Self::outb(self.base_port + 7, Command::Read as u8);

        self.wait_ready()?;

        for i in 0..(self.sector_size / 2) {
            let word = Self::inw(self.base_port);
            buffer[i * 2] = (word & 0xFF) as u8;
            buffer[i * 2 + 1] = (word >> 8) as u8;
        }

        Ok(())
    }

    fn write_block(&mut self, lba: u64, buffer: &[u8]) -> Result<(), &'static str> {
        if buffer.len() < self.sector_size {
            return Err("Buffer too small");
        }

        Self::outb(self.base_port + 6, 0xE0 | ((lba >> 24) & 0x0F) as u8);
        Self::outb(self.base_port + 2, 1);
        Self::outb(self.base_port + 3, lba as u8);
        Self::outb(self.base_port + 4, (lba >> 8) as u8);
        Self::outb(self.base_port + 5, (lba >> 16) as u8);
        Self::outb(self.base_port + 7, Command::Write as u8);

        self.wait_ready()?;

        for i in 0..(self.sector_size / 2) {
            let lo = buffer[i * 2] as u16;
            let hi = buffer[i * 2 + 1] as u16;
            Self::outw(self.base_port, (hi << 8) | lo);
        }

        // Flush
        Self::outb(self.base_port + 7, 0xE7);

        Ok(())
    }

    fn sector_count(&mut self) -> u64 {
        self.sector_count
    }

    fn sector_size(&mut self) -> u64 {
        self.sector_size as u64
    }

    fn send_command(&mut self, command: u32, _buffer: &mut [u8]) -> Result<(), &'static str> {
        Self::outb(self.base_port + 7, command as u8);
        Ok(())
    }
}