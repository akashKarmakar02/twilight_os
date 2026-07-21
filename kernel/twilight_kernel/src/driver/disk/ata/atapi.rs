use super::{BUSES, IdentifyResponse, Status, ensure_buses};
use crate::driver::disk::{BlockDeviceIO, OPTICAL_BLOCK_DEVICE};
use crate::{log, serial_println};
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use bit_field::BitField;
use core::cmp;

const ATAPI_BLOCK_SIZE: usize = 2048;
const ATA_CMD_PACKET: u8 = 0xA0;
const ATA_CMD_IDENTIFY_PACKET: u8 = 0xA1;
const SCSI_TEST_UNIT_READY: u8 = 0x00;
const SCSI_REQUEST_SENSE: u8 = 0x03;
const SCSI_READ_CAPACITY_10: u8 = 0x25;
const SCSI_READ_12: u8 = 0xA8;
const SCSI_SET_CD_SPEED: u8 = 0xBB;
const ATAPI_IREASON_COD: u8 = 1 << 0;
const ATAPI_IREASON_IO: u8 = 1 << 1;
// The byte-count register limits one PIO *phase*, not the whole SCSI command.
const MAX_PHASE_BYTES: usize = 0xFFFE;
// Matches the existing BMIDE bounce-buffer capacity and bounds lock hold time.
const MAX_BLOCKS_PER_COMMAND: usize = (2 * 1024 * 1024) / ATAPI_BLOCK_SIZE;
// Optical seeks and PACKET command setup are expensive. A compact sequential
// window turns common 2 KiB ISO9660 reads into one 128 KiB request.
const READ_AHEAD_BLOCKS: usize = 64;
const READY_RETRIES: usize = 8;

pub struct AtapiCdrom {
    bus: u8,
    drive: u8,
    block_count: usize,
    dma_mode: Option<u8>,
    read_ahead: Vec<u8>,
    read_ahead_lba: u32,
    read_ahead_blocks: usize,
}

impl AtapiCdrom {
    fn open(bus: u8, drive: u8) -> Result<Self, ()> {
        ensure_buses();
        let identify = {
            let mut buses = BUSES.lock();
            match buses
                .get_mut(bus as usize)
                .ok_or(())?
                .identify_drive(drive)?
            {
                IdentifyResponse::Atapi => {}
                _ => return Err(()),
            }
            identify_packet(&mut buses[bus as usize], drive)?
        };

        let dma_mode = crate::driver::disk::ata_dma::configure_atapi(bus, drive, &identify);

        let mut cdrom = Self {
            bus,
            drive,
            block_count: 0,
            dma_mode,
            read_ahead: vec![0; READ_AHEAD_BLOCKS * ATAPI_BLOCK_SIZE],
            read_ahead_lba: 0,
            read_ahead_blocks: 0,
        };
        cdrom.wait_until_ready()?;
        let (last_lba, block_size) = cdrom.read_capacity()?;
        if block_size as usize != ATAPI_BLOCK_SIZE {
            return Err(());
        }
        cdrom.block_count = last_lba as usize + 1;
        // MMC SET CD SPEED uses 0xffff to request the drive's maximum speed.
        // It is optional, so unsupported virtual/physical drives remain usable.
        let _ = cdrom.set_max_read_speed();
        Ok(cdrom)
    }

    fn wait_until_ready(&mut self) -> Result<(), ()> {
        for _ in 0..READY_RETRIES {
            if self
                .packet_in(
                    &[SCSI_TEST_UNIT_READY, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    &mut [],
                )
                .is_ok()
            {
                return Ok(());
            }
            let mut sense = [0u8; 18];
            let cdb = [
                SCSI_REQUEST_SENSE,
                0,
                0,
                0,
                sense.len() as u8,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
            ];
            let _ = self.packet_in(&cdb, &mut sense);
            crate::driver::timer::wait(10_000_000);
        }
        Err(())
    }

    fn read_capacity(&mut self) -> Result<(u32, u32), ()> {
        let mut data = [0u8; 8];
        let cdb = [SCSI_READ_CAPACITY_10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        self.packet_in(&cdb, &mut data)?;
        Ok(parse_capacity(data))
    }

    fn read_blocks_inner(&mut self, lba: u32, blocks: u32, out: &mut [u8]) -> Result<(), ()> {
        if blocks == 0 || out.len() != blocks as usize * ATAPI_BLOCK_SIZE {
            return Err(());
        }
        let cdb = read12_cdb(lba, blocks);
        if self.dma_mode.is_some() {
            if crate::driver::disk::ata_dma::read_atapi(self.bus, self.drive, &cdb, out).is_ok() {
                return Ok(());
            }
            // Match libata/FreeBSD's conservative error policy: stop using DMA
            // after the first failed command and keep the device online via PIO.
            self.dma_mode = None;
            serial_println!("ATAPI {}:{} DMA failed; falling back to PIO", self.bus, self.drive);
        }
        self.packet_in(&cdb, out)
    }

    fn set_max_read_speed(&mut self) -> Result<(), ()> {
        let cdb = [
            SCSI_SET_CD_SPEED,
            0,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        self.packet_in(&cdb, &mut [])
    }

    fn packet_in(&mut self, cdb: &[u8; 12], out: &mut [u8]) -> Result<(), ()> {
        let mut buses = BUSES.lock();
        let bus = buses.get_mut(self.bus as usize).ok_or(())?;

        bus.select_drive(self.drive)?;
        bus.poll(Status::BSY, false)?;
        bus.poll(Status::DRQ, false)?;

        let transfer_hint = cmp::min(out.len().max(ATAPI_BLOCK_SIZE), MAX_PHASE_BYTES) as u16;
        // SAFETY: `bus` exclusively owns the selected ATA channel registers
        // while BUSES is locked, and all values follow the PACKET task file.
        unsafe {
            bus.features_register.write(0);
            bus.sector_count_register.write(0);
            bus.lba0_register.write(0);
            bus.lba1_register.write(transfer_hint as u8);
            bus.lba2_register.write((transfer_hint >> 8) as u8);
            bus.command_register.write(ATA_CMD_PACKET);
        }
        bus.wait(400);
        bus.poll(Status::BSY, false)?;
        if bus.is_error() {
            return Err(());
        }
        bus.poll(Status::DRQ, true)?;

        let reason = bus.sector_count();
        if reason & (ATAPI_IREASON_COD | ATAPI_IREASON_IO) != ATAPI_IREASON_COD {
            return Err(());
        }

        for word in cdb.chunks_exact(2) {
            bus.write_data(u16::from_le_bytes([word[0], word[1]]));
        }

        let mut offset = 0usize;
        loop {
            bus.poll(Status::BSY, false)?;
            if bus.is_error() {
                return Err(());
            }
            if !bus.status().get_bit(Status::DRQ as usize) {
                break;
            }

            let reason = bus.sector_count();
            if reason & (ATAPI_IREASON_COD | ATAPI_IREASON_IO) != ATAPI_IREASON_IO {
                return Err(());
            }

            let phase_len = u16::from_le_bytes([bus.lba1(), bus.lba2()]) as usize;
            if phase_len == 0 {
                return Err(());
            }
            let copy_len = cmp::min(phase_len, out.len().saturating_sub(offset));
            let words = (phase_len + 1) / 2;
            if phase_len % 2 == 0 && copy_len == phase_len {
                bus.read_data_words(out[offset..].as_mut_ptr(), words);
            } else {
                for word_idx in 0..words {
                    let bytes = bus.read_data().to_le_bytes();
                    let byte_idx = word_idx * 2;
                    if byte_idx < copy_len {
                        out[offset + byte_idx] = bytes[0];
                    }
                    if byte_idx + 1 < copy_len {
                        out[offset + byte_idx + 1] = bytes[1];
                    }
                }
            }
            offset += copy_len;
        }

        if offset == out.len() { Ok(()) } else { Err(()) }
    }

    fn read_blocks_uncached(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        let total_blocks = buf.len() / ATAPI_BLOCK_SIZE;
        let mut done = 0usize;
        while done < total_blocks {
            let count = cmp::min(total_blocks - done, MAX_BLOCKS_PER_COMMAND);
            let start = done * ATAPI_BLOCK_SIZE;
            let end = start + count * ATAPI_BLOCK_SIZE;
            self.read_blocks_inner(start_addr + done as u32, count as u32, &mut buf[start..end])?;
            done += count;
        }
        Ok(())
    }

    fn cache_contains(&self, start_addr: u32, blocks: usize) -> bool {
        let start = start_addr as u64;
        let end = start + blocks as u64;
        let cache_start = self.read_ahead_lba as u64;
        let cache_end = cache_start + self.read_ahead_blocks as u64;
        self.read_ahead_blocks != 0 && start >= cache_start && end <= cache_end
    }

    fn copy_cached(&self, start_addr: u32, out: &mut [u8]) {
        let offset_blocks = start_addr as usize - self.read_ahead_lba as usize;
        let offset = offset_blocks * ATAPI_BLOCK_SIZE;
        out.copy_from_slice(&self.read_ahead[offset..offset + out.len()]);
    }

    fn read_blocks_cached(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        let blocks = buf.len() / ATAPI_BLOCK_SIZE;
        if self.cache_contains(start_addr, blocks) {
            self.copy_cached(start_addr, buf);
            return Ok(());
        }

        let available = self.block_count - start_addr as usize;
        let fill_blocks = cmp::min(READ_AHEAD_BLOCKS, available);
        let fill_bytes = fill_blocks * ATAPI_BLOCK_SIZE;
        let mut read_ahead = core::mem::take(&mut self.read_ahead);
        let result = self.read_blocks_uncached(start_addr, &mut read_ahead[..fill_bytes]);
        self.read_ahead = read_ahead;
        result?;
        self.read_ahead_lba = start_addr;
        self.read_ahead_blocks = fill_blocks;
        self.copy_cached(start_addr, buf);
        Ok(())
    }
}

fn parse_capacity(data: [u8; 8]) -> (u32, u32) {
    (
        u32::from_be_bytes([data[0], data[1], data[2], data[3]]),
        u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
    )
}

fn read12_cdb(lba: u32, blocks: u32) -> [u8; 12] {
    let lba = lba.to_be_bytes();
    let blocks = blocks.to_be_bytes();
    [
        SCSI_READ_12,
        0,
        lba[0],
        lba[1],
        lba[2],
        lba[3],
        blocks[0],
        blocks[1],
        blocks[2],
        blocks[3],
        0,
        0,
    ]
}

impl BlockDeviceIO for AtapiCdrom {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        self.read_blocks(addr, buf)
    }

    fn write(&mut self, _addr: u32, _buf: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn block_size(&self) -> usize {
        ATAPI_BLOCK_SIZE
    }

    fn block_count(&self) -> usize {
        self.block_count
    }

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        if buf.is_empty() || buf.len() % ATAPI_BLOCK_SIZE != 0 {
            return Err(());
        }
        let total_blocks = buf.len() / ATAPI_BLOCK_SIZE;
        let end = (start_addr as usize).checked_add(total_blocks).ok_or(())?;
        if end > self.block_count {
            return Err(());
        }

        if total_blocks <= READ_AHEAD_BLOCKS {
            self.read_blocks_cached(start_addr, buf)
        } else {
            self.read_blocks_uncached(start_addr, buf)
        }
    }
}

fn identify_packet(bus: &mut super::Bus, drive: u8) -> Result<[u16; 256], ()> {
    bus.select_drive(drive)?;
    // SAFETY: The selected ATA channel is idle and exclusively borrowed while
    // the IDENTIFY PACKET task file is issued.
    unsafe {
        bus.sector_count_register.write(0);
        bus.lba0_register.write(0);
        bus.lba1_register.write(0);
        bus.lba2_register.write(0);
        bus.command_register.write(ATA_CMD_IDENTIFY_PACKET);
    }
    bus.wait(400);
    bus.poll(Status::BSY, false)?;
    if bus.is_error() {
        return Err(());
    }
    bus.poll(Status::DRQ, true)?;
    let mut identify = [0u16; 256];
    bus.read_data_words(identify.as_mut_ptr().cast::<u8>(), identify.len());
    Ok(identify)
}

pub fn init() {
    #[allow(static_mut_refs)]
    if unsafe { OPTICAL_BLOCK_DEVICE.is_some() } {
        return;
    }

    ensure_buses();
    for bus in 0..2 {
        for drive in 0..2 {
            if let Ok(cdrom) = AtapiCdrom::open(bus, drive) {
                let blocks = cdrom.block_count();
                let dma_mode = cdrom.dma_mode;
                let dev = Box::leak(Box::new(cdrom));
                #[allow(static_mut_refs)]
                unsafe {
                    OPTICAL_BLOCK_DEVICE = Some(dev);
                }
                log!(
                    "ATAPI CD-ROM {}:{} ready ({} blocks, {} bytes/block, {}, readahead={} KiB)",
                    bus,
                    drive,
                    blocks,
                    ATAPI_BLOCK_SIZE,
                    if dma_mode.is_some() { "DMA" } else { "PIO" },
                    READ_AHEAD_BLOCKS * ATAPI_BLOCK_SIZE / 1024
                );
                return;
            }
        }
    }
    serial_println!("ATAPI: no CD-ROM found");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read12_cdb_is_big_endian() {
        let cdb = read12_cdb(0x1020_3040, 0x0000_0102);
        assert_eq!(&cdb[2..6], &[0x10, 0x20, 0x30, 0x40]);
        assert_eq!(&cdb[6..10], &[0, 0, 1, 2]);
    }

    #[test]
    fn capacity_is_big_endian() {
        assert_eq!(
            parse_capacity([0, 0, 0x86, 0x6a, 0, 0, 8, 0]),
            (34_410, 2048)
        );
    }

    #[test]
    fn command_batch_is_larger_than_one_phase() {
        assert!(MAX_BLOCKS_PER_COMMAND > MAX_PHASE_BYTES / ATAPI_BLOCK_SIZE);
        assert_eq!(MAX_BLOCKS_PER_COMMAND * ATAPI_BLOCK_SIZE, 2 * 1024 * 1024);
    }
}
