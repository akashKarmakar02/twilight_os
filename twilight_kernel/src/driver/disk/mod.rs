use crate::sys::fs::vfs::BlockDev;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::mutex::Mutex;

pub mod ata;

pub const BLOCK_SIZE: usize = 2048;

pub static mut BLOCK_DEVICE: Option<&'static mut dyn BlockDeviceIO> = None;

struct DummyBlockDev;

impl BlockDeviceIO for DummyBlockDev {
    fn read(&mut self, _addr: u32, _buf: &mut [u8]) -> Result<(), ()> {
        Err(())
    }
    fn write(&mut self, _addr: u32, _buf: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn block_size(&self) -> usize {
        0
    }

    fn block_count(&self) -> usize {
        0
    }
}
pub trait BlockDeviceIO: Send + Sync + 'static {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ()>;
    fn write(&mut self, addr: u32, buf: &[u8]) -> Result<(), ()>;
    fn block_size(&self) -> usize;
    fn block_count(&self) -> usize;

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        let block_size = self.block_size();
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(());
        }
        for (idx, chunk) in buf.chunks_mut(block_size).enumerate() {
            self.read(start_addr + idx as u32, chunk)?;
        }
        Ok(())
    }

    fn write_blocks(&mut self, start_addr: u32, buf: &[u8]) -> Result<(), ()> {
        let block_size = self.block_size();
        if block_size == 0 || buf.is_empty() || buf.len() % block_size != 0 {
            return Err(());
        }
        for (idx, chunk) in buf.chunks(block_size).enumerate() {
            self.write(start_addr + idx as u32, chunk)?;
        }
        Ok(())
    }
}

const ATA_CACHE_SIZE: usize = 512;

#[derive(Clone, Debug)]
pub struct AtaBlockDevice {
    cache: [Option<(u32, Vec<u8>)>; ATA_CACHE_SIZE],
    dev: ata::Drive,
}

impl AtaBlockDevice {
    pub fn new(bus: u8, dsk: u8) -> Option<Self> {
        ata::Drive::open(bus, dsk).map(|dev| {
            let cache: [Option<(u32, Vec<u8>)>; 512] = [(); ATA_CACHE_SIZE].map(|_| None);
            Self { dev, cache }
        })
    }

    /*
    pub fn len(&self) -> usize {
        self.block_size() * self.block_count()
    }
    */

    fn hash(&self, block_addr: u32) -> usize {
        (block_addr as usize) % self.cache.len()
    }

    fn cached_block(&self, block_addr: u32) -> Option<&[u8]> {
        let h = self.hash(block_addr);
        if let Some((cached_addr, cached_buf)) = &self.cache[h] {
            if block_addr == *cached_addr {
                return Some(cached_buf);
            }
        }
        None
    }

    fn set_cached_block(&mut self, block_addr: u32, buf: &[u8]) {
        let h = self.hash(block_addr);
        self.cache[h] = Some((block_addr, buf.to_vec()));
    }

    fn unset_cached_block(&mut self, block_addr: u32) {
        let h = self.hash(block_addr);
        self.cache[h] = None;
    }
}

impl BlockDeviceIO for AtaBlockDevice {
    fn read(&mut self, block_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        if let Some(cached) = self.cached_block(block_addr) {
            buf.copy_from_slice(cached);
            return Ok(());
        }

        ata::read(self.dev.bus, self.dev.dsk, block_addr, buf)?;
        self.set_cached_block(block_addr, buf);
        Ok(())
    }

    fn write(&mut self, block_addr: u32, buf: &[u8]) -> Result<(), ()> {
        ata::write(self.dev.bus, self.dev.dsk, block_addr, buf)?;
        self.unset_cached_block(block_addr);
        Ok(())
    }

    fn block_size(&self) -> usize {
        self.dev.block_size() as usize
    }

    fn block_count(&self) -> usize {
        self.dev.block_count() as usize
    }

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        let block_size = self.block_size();
        if buf.len() == block_size {
            return self.read(start_addr, buf);
        }
        if buf.len() % block_size != 0 {
            return Err(());
        }
        ata::read(self.dev.bus, self.dev.dsk, start_addr, buf)?;
        for (idx, chunk) in buf.chunks(block_size).enumerate() {
            self.set_cached_block(start_addr + idx as u32, chunk);
        }
        Ok(())
    }

    fn write_blocks(&mut self, start_addr: u32, buf: &[u8]) -> Result<(), ()> {
        let block_size = self.block_size();
        if buf.len() == block_size {
            return self.write(start_addr, buf);
        }
        if buf.len() % block_size != 0 {
            return Err(());
        }
        ata::write(self.dev.bus, self.dev.dsk, start_addr, buf)?;
        for idx in 0..(buf.len() / block_size) {
            self.unset_cached_block(start_addr + idx as u32);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct MemBlockDevice {
    dev: Vec<[u8; BLOCK_SIZE]>,
}

impl MemBlockDevice {
    pub fn new(len: usize) -> Self {
        let dev = vec![[0; BLOCK_SIZE]; len];
        Self { dev }
    }
}

impl BlockDeviceIO for MemBlockDevice {
    fn read(&mut self, block_index: u32, buf: &mut [u8]) -> Result<(), ()> {
        // TODO: check for overflow
        buf[..].clone_from_slice(&self.dev[block_index as usize][..]);
        Ok(())
    }

    fn write(&mut self, block_index: u32, buf: &[u8]) -> Result<(), ()> {
        // TODO: check for overflow
        self.dev[block_index as usize][..].clone_from_slice(buf);
        Ok(())
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn block_count(&self) -> usize {
        self.dev.len()
    }
}

pub fn mount_ata(bus: u8, dsk: u8) {
    let block_dev = Box::leak(Box::new(AtaBlockDevice::new(bus, dsk).unwrap()));

    #[allow(static_mut_refs)]
    unsafe {
        BLOCK_DEVICE = Some(block_dev)
    };
}

pub fn dummy_blockdev() -> BlockDev {
    Arc::new(Mutex::new(Box::new(DummyBlockDev)))
}
