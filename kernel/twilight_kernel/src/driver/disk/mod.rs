use crate::sys::fs::vfs::BlockDev;
use crate::utils::sync::Mutex;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

pub mod ata;
pub mod ata_dma;
pub mod virtioblkdev;

pub const BLOCK_SIZE: usize = 2048;

pub static mut BLOCK_DEVICE: Option<&'static mut dyn BlockDeviceIO> = None;
pub static mut USB_BLOCK_DEVICE: Option<&'static mut dyn BlockDeviceIO> = None;
pub static mut OPTICAL_BLOCK_DEVICE: Option<&'static mut dyn BlockDeviceIO> = None;

pub struct OpticalBlkHandle;

impl OpticalBlkHandle {
    fn with_dev<R>(f: impl FnOnce(&mut dyn BlockDeviceIO) -> Result<R, ()>) -> Result<R, ()> {
        #[allow(static_mut_refs)]
        unsafe {
            match OPTICAL_BLOCK_DEVICE.as_mut() {
                Some(dev) => f(*dev),
                None => Err(()),
            }
        }
    }
}

impl BlockDeviceIO for OpticalBlkHandle {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.read(addr, buf))
    }

    fn write(&mut self, _addr: u32, _buf: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn block_size(&self) -> usize {
        #[allow(static_mut_refs)]
        unsafe {
            OPTICAL_BLOCK_DEVICE
                .as_ref()
                .map(|dev| dev.block_size())
                .unwrap_or(0)
        }
    }

    fn block_count(&self) -> usize {
        #[allow(static_mut_refs)]
        unsafe {
            OPTICAL_BLOCK_DEVICE
                .as_ref()
                .map(|dev| dev.block_count())
                .unwrap_or(0)
        }
    }

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.read_blocks(start_addr, buf))
    }
}

pub struct UsbBlkHandle;

impl UsbBlkHandle {
    fn with_dev<R>(f: impl FnOnce(&mut dyn BlockDeviceIO) -> Result<R, ()>) -> Result<R, ()> {
        #[allow(static_mut_refs)]
        unsafe {
            match USB_BLOCK_DEVICE.as_mut() {
                Some(dev) => f(*dev),
                None => Err(()),
            }
        }
    }
}

pub struct GlobalBlkHandle;

impl GlobalBlkHandle {
    fn with_dev<R>(f: impl FnOnce(&mut dyn BlockDeviceIO) -> Result<R, ()>) -> Result<R, ()> {
        #[allow(static_mut_refs)]
        unsafe {
            match BLOCK_DEVICE.as_mut() {
                Some(dev) => f(*dev),
                None => Err(()),
            }
        }
    }
}

impl BlockDeviceIO for GlobalBlkHandle {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.read(addr, buf))
    }

    fn write(&mut self, addr: u32, buf: &[u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.write(addr, buf))
    }

    fn block_size(&self) -> usize {
        #[allow(static_mut_refs)]
        unsafe {
            BLOCK_DEVICE
                .as_ref()
                .map(|dev| dev.block_size())
                .unwrap_or(0)
        }
    }

    fn block_count(&self) -> usize {
        #[allow(static_mut_refs)]
        unsafe {
            BLOCK_DEVICE
                .as_ref()
                .map(|dev| dev.block_count())
                .unwrap_or(0)
        }
    }

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.read_blocks(start_addr, buf))
    }

    fn write_blocks(&mut self, start_addr: u32, buf: &[u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.write_blocks(start_addr, buf))
    }
}

impl BlockDeviceIO for UsbBlkHandle {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.read(addr, buf))
    }

    fn write(&mut self, addr: u32, buf: &[u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.write(addr, buf))
    }

    fn block_size(&self) -> usize {
        #[allow(static_mut_refs)]
        unsafe {
            USB_BLOCK_DEVICE
                .as_ref()
                .map(|dev| dev.block_size())
                .unwrap_or(0)
        }
    }

    fn block_count(&self) -> usize {
        #[allow(static_mut_refs)]
        unsafe {
            USB_BLOCK_DEVICE
                .as_ref()
                .map(|dev| dev.block_count())
                .unwrap_or(0)
        }
    }

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.read_blocks(start_addr, buf))
    }

    fn write_blocks(&mut self, start_addr: u32, buf: &[u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.write_blocks(start_addr, buf))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtaImpl {
    Pio,
    Dma,
}

pub const ATA_DEFAULT_IMPL: AtaImpl = AtaImpl::Dma;

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

pub struct PartitionBlockDevice {
    inner: Box<dyn BlockDeviceIO + Send>,
    start_block: u32,
    block_count: usize,
    read_only: bool,
}

impl PartitionBlockDevice {
    pub fn new(
        inner: Box<dyn BlockDeviceIO + Send>,
        start_block: u32,
        block_count: usize,
        read_only: bool,
    ) -> Result<Self, ()> {
        let end = (start_block as usize).checked_add(block_count).ok_or(())?;
        if block_count == 0 || end > inner.block_count() {
            return Err(());
        }
        Ok(Self {
            inner,
            start_block,
            block_count,
            read_only,
        })
    }
}

impl BlockDeviceIO for PartitionBlockDevice {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        let block_size = self.block_size();
        if block_size == 0 || buf.len() % block_size != 0 {
            return Err(());
        }
        let blocks = buf.len() / block_size;
        if addr as usize + blocks > self.block_count {
            return Err(());
        }
        self.inner.read_blocks(self.start_block + addr, buf)
    }

    fn write(&mut self, addr: u32, buf: &[u8]) -> Result<(), ()> {
        if self.read_only {
            return Err(());
        }
        let block_size = self.block_size();
        if block_size == 0 || buf.len() % block_size != 0 {
            return Err(());
        }
        let blocks = buf.len() / block_size;
        if addr as usize + blocks > self.block_count {
            return Err(());
        }
        self.inner.write_blocks(self.start_block + addr, buf)
    }

    fn block_size(&self) -> usize {
        self.inner.block_size()
    }

    fn block_count(&self) -> usize {
        self.block_count
    }

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        self.read(start_addr, buf)
    }

    fn write_blocks(&mut self, start_addr: u32, buf: &[u8]) -> Result<(), ()> {
        self.write(start_addr, buf)
    }
}

pub struct FileBlockDevice {
    node: crate::sys::fs::vfs::VfsNode,
    block_size: usize,
    block_count: usize,
    cache_start: usize,
    cache: Vec<u8>,
}

const FILE_BLOCK_READ_AHEAD_BYTES: usize = 512 * 1024;

impl FileBlockDevice {
    pub fn new(node: crate::sys::fs::vfs::VfsNode, block_size: usize) -> Result<Self, ()> {
        if block_size == 0 || node.metadata.size == 0 || node.metadata.size % block_size != 0 {
            return Err(());
        }
        let block_count = node.metadata.size / block_size;
        Ok(Self {
            node,
            block_size,
            block_count,
            cache_start: 0,
            cache: Vec::new(),
        })
    }

    fn cached_read(&mut self, offset: usize, out: &mut [u8]) -> Result<(), ()> {
        let end = offset.checked_add(out.len()).ok_or(())?;
        let cache_end = self.cache_start.saturating_add(self.cache.len());
        if !self.cache.is_empty() && offset >= self.cache_start && end <= cache_end {
            let start = offset - self.cache_start;
            out.copy_from_slice(&self.cache[start..start + out.len()]);
            return Ok(());
        }

        if out.len() >= FILE_BLOCK_READ_AHEAD_BYTES {
            return self.node.read_exact(offset, out);
        }

        let file_size = self.node.metadata.size;
        let window_start = (offset / self.block_size) * self.block_size;
        let window_len = core::cmp::min(FILE_BLOCK_READ_AHEAD_BYTES, file_size - window_start);
        self.cache.clear();
        self.cache_start = window_start;
        self.cache.resize(window_len, 0);
        if self.node.read_exact(window_start, &mut self.cache).is_err() {
            self.cache.clear();
            return Err(());
        }

        let start = offset - window_start;
        out.copy_from_slice(&self.cache[start..start + out.len()]);
        Ok(())
    }
}

impl BlockDeviceIO for FileBlockDevice {
    fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        if buf.is_empty() || buf.len() % self.block_size != 0 {
            return Err(());
        }
        let offset = (addr as usize).checked_mul(self.block_size).ok_or(())?;
        let end = offset.checked_add(buf.len()).ok_or(())?;
        if end > self.node.metadata.size {
            return Err(());
        }
        self.cached_read(offset, buf)
    }

    fn write(&mut self, _addr: u32, _buf: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn block_size(&self) -> usize {
        self.block_size
    }

    fn block_count(&self) -> usize {
        self.block_count
    }
}

const ATA_CACHE_SIZE: usize = 512;
const ATA_CACHE_SMALL_IO_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug)]
pub struct AtaBlockDevice {
    cache: [Option<(u32, Vec<u8>)>; ATA_CACHE_SIZE],
    dev: AtaDevice,
}

#[derive(Clone, Debug)]
enum AtaDevice {
    Pio(ata::Drive),
    Dma(ata_dma::Drive),
}

impl AtaDevice {
    fn open(bus: u8, dsk: u8, imp: AtaImpl) -> Option<Self> {
        match imp {
            AtaImpl::Pio => ata::Drive::open(bus, dsk).map(AtaDevice::Pio),
            AtaImpl::Dma => ata_dma::Drive::open(bus, dsk).map(AtaDevice::Dma),
        }
    }

    fn bus(&self) -> u8 {
        match self {
            AtaDevice::Pio(d) => d.bus,
            AtaDevice::Dma(d) => d.bus,
        }
    }

    fn dsk(&self) -> u8 {
        match self {
            AtaDevice::Pio(d) => d.dsk,
            AtaDevice::Dma(d) => d.dsk,
        }
    }

    fn block_size(&self) -> u32 {
        match self {
            AtaDevice::Pio(d) => d.block_size(),
            AtaDevice::Dma(d) => d.block_size(),
        }
    }

    fn block_count(&self) -> u32 {
        match self {
            AtaDevice::Pio(d) => d.block_count(),
            AtaDevice::Dma(d) => d.block_count(),
        }
    }

    fn read(&self, block_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        match self {
            AtaDevice::Pio(_) => ata::read(self.bus(), self.dsk(), block_addr, buf),
            AtaDevice::Dma(_) => ata_dma::read(self.bus(), self.dsk(), block_addr, buf)
                .or_else(|_| ata::read(self.bus(), self.dsk(), block_addr, buf)),
        }
    }

    fn write(&self, block_addr: u32, buf: &[u8]) -> Result<(), ()> {
        match self {
            AtaDevice::Pio(_) => ata::write(self.bus(), self.dsk(), block_addr, buf),
            AtaDevice::Dma(_) => ata_dma::write(self.bus(), self.dsk(), block_addr, buf)
                .or_else(|_| ata::write(self.bus(), self.dsk(), block_addr, buf)),
        }
    }
}

impl AtaBlockDevice {
    pub fn new(bus: u8, dsk: u8) -> Option<Self> {
        AtaDevice::open(bus, dsk, ATA_DEFAULT_IMPL).map(|dev| {
            let cache: [Option<(u32, Vec<u8>)>; 512] = [(); ATA_CACHE_SIZE].map(|_| None);
            Self { dev, cache }
        })
    }

    pub fn new_with_impl(bus: u8, dsk: u8, imp: AtaImpl) -> Option<Self> {
        AtaDevice::open(bus, dsk, imp).map(|dev| {
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

    fn clear_cache(&mut self) {
        for entry in self.cache.iter_mut() {
            *entry = None;
        }
    }
}

impl BlockDeviceIO for AtaBlockDevice {
    fn read(&mut self, block_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        if buf.len() == self.block_size() {
            if let Some(cached) = self.cached_block(block_addr) {
                buf.copy_from_slice(cached);
                return Ok(());
            }
        }

        self.dev.read(block_addr, buf)?;
        if buf.len() == self.block_size() {
            self.set_cached_block(block_addr, buf);
        }
        Ok(())
    }

    fn write(&mut self, block_addr: u32, buf: &[u8]) -> Result<(), ()> {
        self.dev.write(block_addr, buf)?;
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
        self.dev.read(start_addr, buf)?;
        // Avoid per-sector Vec allocations on large streaming reads.
        if buf.len() <= ATA_CACHE_SMALL_IO_BYTES {
            for (idx, chunk) in buf.chunks(block_size).enumerate() {
                self.set_cached_block(start_addr + idx as u32, chunk);
            }
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
        self.dev.write(start_addr, buf)?;
        // Large writes: invalidate whole cache once (faster, safe).
        if buf.len() > ATA_CACHE_SMALL_IO_BYTES {
            self.clear_cache();
        } else {
            for idx in 0..(buf.len() / block_size) {
                self.unset_cached_block(start_addr + idx as u32);
            }
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

pub fn mount_mem() {
    let block_dev = Box::leak(Box::new(MemBlockDevice::new(30)));

    #[allow(static_mut_refs)]
    unsafe {
        BLOCK_DEVICE = Some(block_dev)
    };
}

pub fn mount_ata(bus: u8, dsk: u8) {
    let block_dev = Box::leak(Box::new(AtaBlockDevice::new(bus, dsk).unwrap()));

    #[allow(static_mut_refs)]
    unsafe {
        BLOCK_DEVICE = Some(block_dev)
    };
}

pub fn mount_ata_with_impl(bus: u8, dsk: u8, imp: AtaImpl) {
    let block_dev = Box::leak(Box::new(
        AtaBlockDevice::new_with_impl(bus, dsk, imp).unwrap(),
    ));

    #[allow(static_mut_refs)]
    unsafe {
        BLOCK_DEVICE = Some(block_dev)
    };
}

pub fn dummy_blockdev() -> BlockDev {
    Arc::new(Mutex::new(Box::new(DummyBlockDev)))
}

pub fn init() {
    match ATA_DEFAULT_IMPL {
        AtaImpl::Pio => ata::init(),
        AtaImpl::Dma => ata_dma::init(),
    }
    ata::atapi::init();
    virtioblkdev::init();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NumberedBlocks;

    impl BlockDeviceIO for NumberedBlocks {
        fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), ()> {
            buf.fill(addr as u8);
            Ok(())
        }

        fn write(&mut self, _addr: u32, _buf: &[u8]) -> Result<(), ()> {
            Ok(())
        }

        fn block_size(&self) -> usize {
            512
        }

        fn block_count(&self) -> usize {
            16
        }
    }

    #[test]
    fn partition_translates_and_bounds_blocks() {
        let mut partition =
            PartitionBlockDevice::new(Box::new(NumberedBlocks), 2, 4, true).unwrap();
        let mut bytes = [0u8; 1024];
        partition.read_blocks(1, &mut bytes).unwrap();
        assert!(bytes[..512].iter().all(|byte| *byte == 3));
        assert!(bytes[512..].iter().all(|byte| *byte == 4));
        assert!(partition.read_blocks(3, &mut bytes).is_err());
        assert!(partition.write_blocks(0, &[0u8; 512]).is_err());
    }
}
