use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};
use crate::driver::disk::BlockDevice;

pub struct Ata {
    pub base_port: u16,
    pub sector_count: u64,
    pub sector_size: usize,
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

    pub fn detect(&mut self) -> bool {
        let mut port_cmd = Port::new(self.base_port + 7);
        let mut port_data = Port::new(self.base_port);

        // Select Master drive
        let mut port_drive_select = Port::<u8>::new(self.base_port + 6);
        unsafe { port_drive_select.write(0xA0) };

        // Send IDENTIFY command
        unsafe { port_cmd.write(0xEC) };

        // Check if the device exists
        if unsafe { port_cmd.read() } == 0u8 {
            return false; // No device
        }

        // Wait for the drive to be ready (BSY clear, DRQ set)
        while unsafe { port_cmd.read() } & 0x80 != 0u8 {} // Wait for BSY to clear
        if unsafe { port_cmd.read() } & 0x08 == 0u8 {
            return false; // DRQ not set, invalid device
        }

        // Read IDENTIFY data (256 words)
        let mut identify_data = [0u16; 256];
        for i in 0..256 {
            identify_data[i] = unsafe { port_data.read() };
        }

        // Standard sector size
        self.sector_size = 512;

        // Fetch sector count from words 60–61
        self.sector_count =
            ((identify_data[61] as u64) << 16) | (identify_data[60] as u64);

        true
    }

    pub fn get_model_name(&self) -> [u8; 40] {
        let mut model = [0u8; 40];

        let mut identify_data = [0u16; 256];
        for i in 0..256 {
            identify_data[i] = Self::inw(self.base_port);
        }

        for i in 0..20 {
            model[i * 2] = (identify_data[27 + i] >> 8) as u8;
            model[i * 2 + 1] = identify_data[27 + i] as u8;
        }

        model
    }
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
        Self::outb(self.base_port + 7, 0x20);

        for i in 0..self.sector_count as usize {
            buffer[i] = Self::inb(self.base_port);
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
        Self::outb(self.base_port + 7, 0x30);

        for &byte in buffer.iter() {
            Self::outb(self.base_port, byte);
        }

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