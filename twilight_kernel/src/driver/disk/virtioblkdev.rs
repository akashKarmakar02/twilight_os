#![allow(dead_code)]

use crate::driver::disk::{ATA_CACHE_SIZE, BLOCK_DEVICE, BlockDeviceIO};
use crate::println;
use crate::sys::memory::phys::PhysBuf;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use conquer_once::spin::OnceCell;
use core::mem::size_of;
use core::ptr::{read_volatile, write_bytes, write_volatile};
use core::sync::atomic::{Ordering, compiler_fence};
use spin::Mutex;
use x86_64::align_up;
use x86_64::instructions::port::Port;

const VIRTIO_PCI_DEVICE_FEATURES: u16 = 0x00;
const VIRTIO_PCI_GUEST_FEATURES: u16 = 0x04;
const VIRTIO_PCI_QUEUE_ADDR: u16 = 0x08;
const VIRTIO_PCI_QUEUE_SIZE: u16 = 0x0C;
const VIRTIO_PCI_QUEUE_SELECT: u16 = 0x0E;
const VIRTIO_PCI_QUEUE_NOTIFY: u16 = 0x10;
const VIRTIO_PCI_STATUS: u16 = 0x12;
const VIRTIO_PCI_ISR: u16 = 0x13;

// Status bits
const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
const VIRTIO_STATUS_DRIVER: u8 = 2;
const VIRTIO_STATUS_DRIVER_OK: u8 = 4;
const VIRTIO_STATUS_FAILED: u8 = 128;

pub const VIRTQ_DESC_F_NEXT: u16 = 1;
pub const VIRTQ_DESC_F_WRITE: u16 = 2;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;
const VIRTIO_PCI_CONFIG_OFF: u16 = 0x14; // legacy: device-specific config base

const MAX_RETRIES: usize = 3;

// Virtio legacy 32-bit feature bits (subset).
const VIRTIO_F_RING_INDIRECT_DESC: u32 = 1 << 28;
const VIRTIO_F_RING_EVENT_IDX: u32 = 1 << 29;

const VIRTIO_BLK_F_BLK_SIZE: u32 = 1 << 6;
const VIRTIO_BLK_F_FLUSH: u32 = 1 << 9;
const VIRTIO_BLK_F_CONFIG_WCE: u32 = 1 << 11;

#[repr(C)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

#[repr(C)]
pub struct VirtqAvail {
    pub flags: u16,
    pub idx: u16,
    pub ring: [u16; 256],
}

#[repr(C)]
pub struct VirtqUsedElem {
    pub id: u32,
    pub len: u32,
}

#[repr(C)]
struct VirtioBlkReq {
    type_: u32,
    reserved: u32,
    sector: u64, // LBA
}

#[repr(C)]
pub struct VirtqUsed {
    pub flags: u16,
    pub idx: u16,
    pub ring: [VirtqUsedElem; 256],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct DmaBuf {
    virt: *mut u8,
    phys: u64,
    len: usize,
}

pub static VIRTIO_DEV: OnceCell<Mutex<VirtioBlkDev>> = OnceCell::uninit();

/// A small handle that forwards to the global Virtio block device (behind a mutex).
/// Multiple handles can safely exist without duplicating virtqueue state.
pub struct VirtioBlkHandle;

impl VirtioBlkHandle {
    fn with_dev<R>(f: impl FnOnce(&mut VirtioBlkDev) -> R) -> Result<R, ()> {
        let dev_lock = VIRTIO_DEV.get().ok_or(())?;
        let mut dev = dev_lock.lock();
        Ok(f(&mut dev))
    }
}

impl BlockDeviceIO for VirtioBlkHandle {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.read(lba, buf))?
    }

    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.write(lba, buf))?
    }

    fn block_size(&self) -> usize {
        let Ok(sz) = Self::with_dev(|dev| dev.block_size()) else {
            return 0;
        };
        sz
    }

    fn block_count(&self) -> usize {
        let Ok(cnt) = Self::with_dev(|dev| dev.block_count()) else {
            return 0;
        };
        cnt
    }

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.read_blocks(start_addr, buf))?
    }

    fn write_blocks(&mut self, start_addr: u32, buf: &[u8]) -> Result<(), ()> {
        Self::with_dev(|dev| dev.write_blocks(start_addr, buf))?
    }
}

pub fn init() {
    if let Some(dev) = VirtioBlkDev::probe_and_init() {
        let _ = VIRTIO_DEV.try_init_once(|| Mutex::new(dev));
        let handle = Box::leak(Box::new(VirtioBlkHandle));
        #[allow(static_mut_refs)]
        unsafe {
            BLOCK_DEVICE = Some(handle);
        }
    }
}

const QSZ: usize = 256;
const DMA_DATA_BYTES: usize = 512 * 1024;
const CACHE_LINES: usize = 1024; // 1024 * 512B = 512KiB

#[derive(Clone, Copy)]
struct CacheLine {
    valid: bool,
    dirty: bool,
    tag: u32, // sector LBA
    data: [u8; 512],
}

impl Default for CacheLine {
    fn default() -> Self {
        Self {
            valid: false,
            dirty: false,
            tag: 0,
            data: [0u8; 512],
        }
    }
}

pub struct VirtQueue {
    // queue backing pages (must stay alive forever)
    _qmem: PhysBuf,
    qvirt: u64,
    qphys: u64,
    qsz: u16,

    desc: *mut VirtqDesc,
    avail: *mut u8,
    used: *mut u8,

    // driver cursors
    last_used: u16,

    // simple free-list
    free: [u16; QSZ],
    free_len: usize,
}

impl VirtQueue {
    pub fn new(qmem: PhysBuf, qsz: u16) -> Self {
        let qsz_usize = qsz as usize;
        let qvirt = qmem.virt_addr().as_u64();
        let qphys = qmem.addr();

        let desc_off = 0usize;
        let avail_off = desc_off + size_of::<VirtqDesc>() * qsz_usize;
        // avail: flags(u16)+idx(u16)+ring[u16;qsz]+used_event(u16)
        let avail_size = 4 + (2 * qsz_usize) + 2;
        let used_off = align_up((avail_off + avail_size) as u64, 4096) as usize;

        let desc = (qvirt as usize + desc_off) as *mut VirtqDesc;
        let avail = (qvirt as usize + avail_off) as *mut u8;
        let used = (qvirt as usize + used_off) as *mut u8;

        // init free list with all descriptor ids [0..QSZ)
        let mut free = [0u16; QSZ];
        let mut i = 0;
        while i < qsz_usize {
            free[i] = i as u16;
            i += 1;
        }

        Self {
            _qmem: qmem,
            qvirt,
            qphys,
            desc,
            avail,
            used,
            qsz,
            last_used: 0,
            free,
            free_len: qsz_usize,
        }
    }

    fn alloc_desc(&mut self) -> u16 {
        if self.free_len == 0 {
            panic!("virtq: out of descriptors");
        }
        self.free_len -= 1;
        self.free[self.free_len]
    }

    fn free_desc(&mut self, id: u16) {
        if self.free_len >= QSZ {
            panic!("virtq: double free");
        }
        self.free[self.free_len] = id;
        self.free_len += 1;
    }

    /// Submit a head descriptor index into avail ring (queue 0) and return completed used_id.
    fn submit_and_wait(&mut self, io_base: u16, head: u16) -> u16 {
        let qsz = self.qsz as usize;

        // publish to avail
        let avail_u16 = self.avail as *mut u16;
        let a_idx = unsafe { read_volatile(avail_u16.add(1)) };
        let ring = unsafe { avail_u16.add(2) };
        unsafe { write_volatile(ring.add((a_idx as usize) % qsz), head) };
        // Ensure descriptor/avail writes are not reordered by the compiler.
        // On x86 (and with coherent DMA memory), a compiler fence is sufficient here.
        compiler_fence(Ordering::Release);
        unsafe {
            write_volatile(avail_u16.add(1), a_idx.wrapping_add(1));
        }
        compiler_fence(Ordering::Release);

        // notify queue 0
        let mut notify = Port::<u16>::new(io_base + VIRTIO_PCI_QUEUE_NOTIFY);
        unsafe {
            notify.write(0);
        }

        // wait for used.idx advance
        loop {
            // serial_println!("{:#X}", self.used as *mut u8 as u64);
            let used_u16 = self.used as *mut u16;
            let u_idx = unsafe { read_volatile(used_u16.add(1)) };
            // serial_println!("{u_idx}");
            if u_idx != self.last_used {
                break;
            }
            core::hint::spin_loop();
        }

        compiler_fence(Ordering::Acquire);
        let used_slot = (self.last_used as usize) % qsz;
        let used_ring = unsafe { self.used.add(4) as *mut VirtqUsedElem };
        let used_id = unsafe { read_volatile(&(*used_ring.add(used_slot)).id) as u16 };
        self.last_used = self.last_used.wrapping_add(1);
        used_id
    }
}

fn read_u64_port(io: u16) -> u64 {
    // no Port<u64> exists; read as two u32
    unsafe {
        let mut lo = Port::<u32>::new(io);
        let mut hi = Port::<u32>::new(io + 4);
        (lo.read() as u64) | ((hi.read() as u64) << 32)
    }
}

pub fn virtio_blk_capacity_sectors(io_base: u16) -> u64 {
    read_u64_port(io_base + VIRTIO_PCI_CONFIG_OFF)
}

pub struct VirtioBlkDev {
    pub(crate) io_base: u16,
    pub(crate) vq: VirtQueue,
    req_dma: PhysBuf,
    st_dma: PhysBuf,
    data_dma: PhysBuf,
    cache: Box<[CacheLine]>,
    cache_data: [Option<(u32, Vec<u8>)>; ATA_CACHE_SIZE],
}

impl VirtioBlkDev {
    const MAX_BLOCK_CACHE_BYTES: usize = 4096;
    const VIRTIO_BLK_CFG_WRITEBACK_OFF: u16 = 0x20; // struct virtio_blk_config::writeback

    fn probe_and_init() -> Option<Self> {
        let mut dev = crate::sys::pci::find_device(0x1AF4, 0x1001)?;
        dev.enable_bus_mastering();
        let io = dev.io_base();

        // reset
        unsafe {
            Port::<u8>::new(io + VIRTIO_PCI_STATUS).write(0);
        }

        // ack + driver
        unsafe {
            let mut st = Port::<u8>::new(io + VIRTIO_PCI_STATUS);
            st.write(VIRTIO_STATUS_ACKNOWLEDGE);
            st.write(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);
        }

        // legacy feature negotiation (we don't require any features)
        unsafe {
            let dev_features = Port::<u32>::new(io + VIRTIO_PCI_DEVICE_FEATURES).read();
            // Ask for useful bits when available. CONFIG_WCE can materially affect write speed
            // because it enables the device's write cache behavior.
            let wanted = VIRTIO_F_RING_INDIRECT_DESC
                | VIRTIO_F_RING_EVENT_IDX
                | VIRTIO_BLK_F_BLK_SIZE
                | VIRTIO_BLK_F_FLUSH
                | VIRTIO_BLK_F_CONFIG_WCE;
            let guest = dev_features & wanted;
            Port::<u32>::new(io + VIRTIO_PCI_GUEST_FEATURES).write(guest);

            if (guest & VIRTIO_BLK_F_CONFIG_WCE) != 0 {
                let mut wb = Port::<u8>::new(
                    io + VIRTIO_PCI_CONFIG_OFF + Self::VIRTIO_BLK_CFG_WRITEBACK_OFF,
                );
                // 1 = writeback enabled (when supported by device).
                wb.write(1);
            }
        }

        // select queue 0, read size
        unsafe {
            Port::<u16>::new(io + VIRTIO_PCI_QUEUE_SELECT).write(0);
        }
        let max_qsz = unsafe { Port::<u16>::new(io + VIRTIO_PCI_QUEUE_SIZE).read() } as usize;
        let qsz = core::cmp::min(max_qsz, QSZ);
        if qsz < 8 {
            println!("virtio-blk: queue too small: {}", max_qsz);
            return None;
        }
        // program chosen queue size
        unsafe {
            Port::<u16>::new(io + VIRTIO_PCI_QUEUE_SIZE).write(qsz as u16);
        }

        // Allocate virtqueue backing memory (contiguous pages).
        // Layout depends on the negotiated queue size.
        let desc_sz = size_of::<VirtqDesc>() * qsz;
        let avail_sz = 4 + (2 * qsz) + 2; // flags+idx+ring+used_event
        let used_off = align_up((desc_sz + avail_sz) as u64, 4096) as usize;
        let used_sz = 4 + (size_of::<VirtqUsedElem>() * qsz) + 2; // flags+idx+ring+avail_event
        let total = used_off + used_sz;

        let qmem = PhysBuf::new(total);
        unsafe {
            write_bytes(qmem.virt_addr().as_mut_ptr::<u8>(), 0, total);
        }

        // register PFN
        unsafe {
            Port::<u16>::new(io + VIRTIO_PCI_QUEUE_SELECT).write(0);
        }
        unsafe {
            Port::<u32>::new(io + VIRTIO_PCI_QUEUE_ADDR).write((qmem.addr() >> 12) as u32);
        }

        // driver ok
        unsafe {
            Port::<u8>::new(io + VIRTIO_PCI_STATUS)
                .write(VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK);
        }

        let vq = VirtQueue::new(qmem, qsz as u16);
        let req_dma = PhysBuf::new(size_of::<VirtioBlkReq>());
        let st_dma = PhysBuf::new(1);
        let data_dma = PhysBuf::new(DMA_DATA_BYTES);
        let cache = vec![CacheLine::default(); CACHE_LINES].into_boxed_slice();
        let cache_data: [Option<(u32, Vec<u8>)>; 512] = [(); ATA_CACHE_SIZE].map(|_| None);

        Some(Self {
            io_base: io,
            vq,
            req_dma,
            st_dma,
            data_dma,
            cache,
            cache_data,
        })
    }

    fn cache_idx(lba: u32) -> usize {
        (lba as usize) % CACHE_LINES
    }

    fn read_sector_raw(&mut self, lba: u32, out: &mut [u8; 512]) -> Result<(), ()> {
        self.rw_bytes(lba as u64, Some(out.as_mut_slice()), None)
    }

    fn write_sector_raw(&mut self, lba: u32, data: &[u8; 512]) -> Result<(), ()> {
        self.rw_bytes(lba as u64, None, Some(data.as_slice()))
    }

    fn cache_flush_all(&mut self) -> Result<(), ()> {
        for idx in 0..self.cache.len() {
            let line = self.cache[idx];
            if line.valid && line.dirty {
                self.write_sector_raw(line.tag, &line.data)?;
                self.cache[idx].dirty = false;
            }
        }
        Ok(())
    }

    fn cache_read_sector(&mut self, lba: u32, out: &mut [u8; 512]) -> Result<(), ()> {
        let idx = Self::cache_idx(lba);
        let cur = self.cache[idx];

        if cur.valid && cur.tag == lba {
            *out = cur.data;
            return Ok(());
        }

        if cur.valid && cur.dirty {
            self.write_sector_raw(cur.tag, &cur.data)?;
        }

        self.read_sector_raw(lba, out)?;
        self.cache[idx] = CacheLine {
            valid: true,
            dirty: false,
            tag: lba,
            data: *out,
        };
        Ok(())
    }

    fn cache_write_sector(&mut self, lba: u32, data: &[u8; 512]) -> Result<(), ()> {
        let idx = Self::cache_idx(lba);
        let cur = self.cache[idx];

        if cur.valid && cur.tag == lba {
            self.cache[idx] = CacheLine {
                valid: true,
                dirty: true,
                tag: lba,
                data: *data,
            };
            return Ok(());
        }

        if cur.valid && cur.dirty {
            self.write_sector_raw(cur.tag, &cur.data)?;
        }

        self.cache[idx] = CacheLine {
            valid: true,
            dirty: true,
            tag: lba,
            data: *data,
        };
        Ok(())
    }

    #[inline]
    fn cache_invalidate_range(&mut self, start_lba: u32, byte_len: usize) {
        if byte_len == 0 {
            return;
        }
        let sectors = (byte_len + 511) / 512;
        for i in 0..sectors {
            let lba = start_lba.wrapping_add(i as u32);
            let idx = Self::cache_idx(lba);
            if self.cache[idx].valid && self.cache[idx].tag == lba {
                self.cache[idx].valid = false;
                self.cache[idx].dirty = false;
            }
        }
    }

    pub fn flush(&mut self) -> Result<(), ()> {
        self.cache_flush_all()?;

        // two descriptors: req + status
        unsafe {
            *self.req_dma.virt_addr().as_mut_ptr::<VirtioBlkReq>() = VirtioBlkReq {
                type_: VIRTIO_BLK_T_FLUSH,
                reserved: 0,
                sector: 0,
            };
            *self.st_dma.virt_addr().as_mut_ptr::<u8>() = 0xFF;

            let d0 = self.vq.alloc_desc();
            let d1 = self.vq.alloc_desc();

            write_volatile(
                self.vq.desc.add(d0 as usize),
                VirtqDesc {
                    addr: self.req_dma.addr(),
                    len: size_of::<VirtioBlkReq>() as u32,
                    flags: VIRTQ_DESC_F_NEXT,
                    next: d1,
                },
            );
            write_volatile(
                self.vq.desc.add(d1 as usize),
                VirtqDesc {
                    addr: self.st_dma.addr(),
                    len: 1,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                },
            );

            let used_id = self.vq.submit_and_wait(self.io_base, d0);
            let st = read_volatile(self.st_dma.virt_addr().as_mut_ptr::<u8>());

            self.vq.free_desc(d1);
            self.vq.free_desc(d0);

            if st != 0 {
                return Err(());
            }

            if used_id != d0 {
                println!(
                    "virtio-blk: flush completed id {} (expected {})",
                    used_id, d0
                );
            }

            Ok(())
        }
    }

    fn rw_chunk(
        &mut self,
        lba: u64,
        len: usize,
        read_out: Option<&mut [u8]>,
        write_in: Option<&[u8]>,
    ) -> Result<(), ()> {
        if len == 0 || (len % 512) != 0 || len > DMA_DATA_BYTES {
            return Err(());
        }

        let is_read = read_out.is_some();
        let (req_type, data_write_flag) = if is_read {
            (VIRTIO_BLK_T_IN, VIRTQ_DESC_F_WRITE) // device writes data
        } else {
            (VIRTIO_BLK_T_OUT, 0) // device reads data
        };

        unsafe {
            *self.req_dma.virt_addr().as_mut_ptr::<VirtioBlkReq>() = VirtioBlkReq {
                type_: req_type,
                reserved: 0,
                sector: lba,
            };
            *self.st_dma.virt_addr().as_mut_ptr::<u8>() = 0xFF;

            if let Some(src) = write_in {
                core::ptr::copy_nonoverlapping(
                    src.as_ptr(),
                    self.data_dma.virt_addr().as_mut_ptr(),
                    len,
                );
            }

            let d0 = self.vq.alloc_desc();
            let d1 = self.vq.alloc_desc();
            let d2 = self.vq.alloc_desc();

            write_volatile(
                self.vq.desc.add(d0 as usize),
                VirtqDesc {
                    addr: self.req_dma.addr(),
                    len: size_of::<VirtioBlkReq>() as u32,
                    flags: VIRTQ_DESC_F_NEXT,
                    next: d1,
                },
            );

            write_volatile(
                self.vq.desc.add(d1 as usize),
                VirtqDesc {
                    addr: self.data_dma.addr(),
                    len: len as u32,
                    flags: VIRTQ_DESC_F_NEXT | data_write_flag,
                    next: d2,
                },
            );

            write_volatile(
                self.vq.desc.add(d2 as usize),
                VirtqDesc {
                    addr: self.st_dma.addr(),
                    len: 1,
                    flags: VIRTQ_DESC_F_WRITE,
                    next: 0,
                },
            );

            let used_id = self.vq.submit_and_wait(self.io_base, d0);

            let st = read_volatile(self.st_dma.virt_addr().as_mut_ptr::<u8>());

            self.vq.free_desc(d2);
            self.vq.free_desc(d1);
            self.vq.free_desc(d0);

            if st != 0 {
                return Err(());
            }

            if let Some(dst) = read_out {
                core::ptr::copy_nonoverlapping(
                    self.data_dma.virt_addr().as_ptr(),
                    dst.as_mut_ptr(),
                    len,
                );
            }

            if used_id != d0 {
                // should not happen with single outstanding, but good log
                println!("virtio-blk: completed id {} (expected {})", used_id, d0);
            }
        }
        Ok(())
    }

    fn rw_bytes(
        &mut self,
        lba: u64,
        mut read_out: Option<&mut [u8]>,
        write_in: Option<&[u8]>,
    ) -> Result<(), ()> {
        let total_len = read_out
            .as_ref()
            .map(|b| b.len())
            .unwrap_or_else(|| write_in.unwrap().len());
        if total_len == 0 || (total_len % 512) != 0 {
            return Err(());
        }

        let mut offset = 0usize;
        while offset < total_len {
            let remaining = total_len - offset;
            let mut chunk_len = core::cmp::min(remaining, DMA_DATA_BYTES);
            chunk_len -= chunk_len % 512;
            if chunk_len == 0 {
                return Err(());
            }

            let chunk_lba = lba + (offset / 512) as u64;
            let mut ok = false;
            for _ in 0..MAX_RETRIES {
                let read_chunk = read_out
                    .as_mut()
                    .map(|b| &mut b[offset..offset + chunk_len]);
                let write_chunk = write_in.map(|b| &b[offset..offset + chunk_len]);
                if self
                    .rw_chunk(chunk_lba, chunk_len, read_chunk, write_chunk)
                    .is_ok()
                {
                    ok = true;
                    break;
                }
            }
            if !ok {
                return Err(());
            }

            offset += chunk_len;
        }

        Ok(())
    }

    fn hash(&self, block_addr: u32) -> usize {
        (block_addr as usize) % self.cache_data.len()
    }

    fn cached_block(&self, block_addr: u32) -> Option<&[u8]> {
        let h = self.hash(block_addr);
        if let Some((cached_addr, cached_buf)) = &self.cache_data[h] {
            if block_addr == *cached_addr {
                return Some(cached_buf);
            }
        }
        None
    }

    fn set_cached_block(&mut self, block_addr: u32, buf: &[u8]) {
        if buf.len() > Self::MAX_BLOCK_CACHE_BYTES {
            return;
        }
        let h = self.hash(block_addr);
        self.cache_data[h] = Some((block_addr, buf.to_vec()));
    }

    fn unset_cached_block(&mut self, block_addr: u32) {
        let h = self.hash(block_addr);
        self.cache_data[h] = None;
    }
}

unsafe impl Send for VirtioBlkDev {}
unsafe impl Sync for VirtioBlkDev {}

impl BlockDeviceIO for VirtioBlkDev {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        if buf.is_empty() || (buf.len() % 512) != 0 {
            return Err(());
        }

        // Fast path: do a single multi-sector virtio request (chunked internally).
        // The previous implementation routed all reads through a 512B cache and ended up
        // submitting one virtqueue request per sector, which is extremely slow.
        if buf.len() > 512 {
            self.rw_bytes(lba as u64, Some(buf), None)?;
            self.set_cached_block(lba, buf);
            return Ok(());
        }

        // Small reads: allow sector cache hits.
        if let Some(cached) = self.cached_block(lba) {
            if cached.len() == buf.len() {
                buf.copy_from_slice(cached);
                return Ok(());
            }
        }

        let sector: &mut [u8; 512] = buf.try_into().map_err(|_| ())?;
        self.cache_read_sector(lba, sector)?;
        self.set_cached_block(lba, buf);
        Ok(())
    }

    fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()> {
        if buf.is_empty() || (buf.len() % 512) != 0 {
            return Err(());
        }

        // Write-through: issue the actual virtio write in large chunks.
        // The prior write-back cache never hit the device unless evicted/flush() ran.
        self.rw_bytes(lba as u64, None, Some(buf))?;

        // Keep cache coherent: invalidate any cached sectors we overwrote.
        self.cache_invalidate_range(lba, buf.len());
        self.unset_cached_block(lba);
        Ok(())
    }

    fn block_size(&self) -> usize {
        512
    }

    fn block_count(&self) -> usize {
        read_u64_port(self.io_base + VIRTIO_PCI_CONFIG_OFF) as usize
    }

    fn read_blocks(&mut self, start_addr: u32, buf: &mut [u8]) -> Result<(), ()> {
        self.read(start_addr, buf)
    }

    fn write_blocks(&mut self, start_addr: u32, buf: &[u8]) -> Result<(), ()> {
        self.write(start_addr, buf)
    }
}

fn mount_virtio_blk(blk_dev: VirtioBlkDev) {
    let virtio_blk_box = Box::leak(Box::new(blk_dev));

    #[allow(static_mut_refs)]
    unsafe {
        BLOCK_DEVICE = Some(virtio_blk_box);
    }
}
