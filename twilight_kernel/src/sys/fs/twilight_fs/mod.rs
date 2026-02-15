pub mod blockgroup;
pub mod dir_entry;
pub mod inode;
mod journal;
pub mod metadata;
pub mod superblock;

use crate::driver;
use crate::driver::disk::virtioblkdev::VirtioBlkHandle;
use crate::driver::disk::{BLOCK_DEVICE, BlockDeviceIO, UsbBlkHandle};
use crate::driver::timer::cmos::CMOS;
use crate::sys::fs::MFS;
use crate::sys::fs::partition::{self, PartitionEntry, TWILIGHT_PARTITION_TYPE};
use crate::sys::fs::twilight_fs::FsError::{
    FileAlreadyExists, FileNameTooLong, FileNotFound, InvalidInode,
};
use crate::sys::fs::twilight_fs::inode::{Inode, TFSVfsNode};
use crate::sys::fs::twilight_fs::superblock::Superblock;
use crate::sys::fs::vfs::{BlockDev, FileSystem, FileType, FsCtx, Metadata, VfsNode};
use crate::sys::syscall::fs_attr::IFLAG_ENCRYPTED;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::size_of;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use spin::rwlock::RwLock;

pub const FS_BLOCK_SIZE: usize = 2048;
static FS_BLOCK_OFFSET: AtomicUsize = AtomicUsize::new(0);

const PATH_LOOKUP_CACHE_CAPACITY: usize = 1024;
const FILE_CACHE_MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const FILE_CACHE_MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

#[inline]
fn to_u32_saturating(value: u64) -> u32 {
    if value > u32::MAX as u64 {
        u32::MAX
    } else {
        value as u32
    }
}

#[inline]
pub fn fs_block_offset_bytes() -> usize {
    FS_BLOCK_OFFSET.load(Ordering::Relaxed)
}

#[inline]
pub fn set_fs_block_offset_bytes(offset: usize) {
    FS_BLOCK_OFFSET.store(offset, Ordering::Relaxed);
}

#[inline]
pub fn set_fs_block_offset_lba(start_lba: u32) {
    set_fs_block_offset_bytes((start_lba as usize) * partition::SECTOR_SIZE as usize);
}

#[inline]
fn fs_block_offset_sectors() -> usize {
    fs_block_offset_bytes() / partition::SECTOR_SIZE as usize
}

pub fn read_tfs_block(
    device: &mut dyn BlockDeviceIO,
    block_no: u32,
    buf: &mut [u8; 2048],
) -> Result<(), FsError> {
    read_tfs_blocks(device, block_no, buf)
}

pub fn read_tfs_blocks(
    device: &mut dyn BlockDeviceIO,
    start_block_no: u32,
    buf: &mut [u8],
) -> Result<(), FsError> {
    let start_block_no = start_block_no as usize;
    let start_device_block = (start_block_no * 4) + fs_block_offset_sectors();
    device
        .read_blocks(start_device_block as u32, buf)
        .map_err(|_| InvalidInode)
}

pub fn write_tfs_block(
    device: &mut dyn BlockDeviceIO,
    block_no: u32,
    buf: &[u8; 2048],
) -> Result<(), FsError> {
    write_tfs_blocks(device, block_no, buf)
}

pub fn write_tfs_blocks(
    device: &mut dyn BlockDeviceIO,
    start_block_no: u32,
    buf: &[u8],
) -> Result<(), FsError> {
    let start_block_no = start_block_no as usize;
    let start_device_block = (start_block_no * 4) + fs_block_offset_sectors();
    device
        .write_blocks(start_device_block as u32, buf)
        .map_err(|_| InvalidInode)
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub inode: u32,
    pub name: [u8; 60], // MINIX v2 uses fixed 60-byte names
}

#[derive(Debug)]
pub enum FsError {
    NotSupported,
    FileAlreadyExists,
    FileNotFound,
    InvalidPath,
    InvalidInode,
    FileNameTooLong,
    FileSizeTooLarge,
}

fn detect_twilight_partition(bus: u8, dsk: u8) -> Option<PartitionEntry> {
    let mut sector = [0u8; 512];
    if driver::disk::ata::read(bus, dsk, 0, &mut sector).is_err() {
        return None;
    }

    if !partition::has_signature(&sector) {
        return None;
    }

    let entries = partition::decode_entries(&sector);
    partition::find_entry(&entries, TWILIGHT_PARTITION_TYPE)
}

fn detect_twilight_partition_blk_dev() -> Option<PartitionEntry> {
    let mut sector = [0u8; 512];
    #[allow(static_mut_refs)]
    let dev = unsafe { BLOCK_DEVICE.as_mut().unwrap() };
    if dev.read(0, &mut sector).is_err() {
        return None;
    }

    if !partition::has_signature(&sector) {
        return None;
    }

    let entries = partition::decode_entries(&sector);
    partition::find_entry(&entries, TWILIGHT_PARTITION_TYPE)
}

fn detect_twilight_partition_on_device(device: &mut dyn BlockDeviceIO) -> Option<PartitionEntry> {
    if device.block_size() != 512 {
        return None;
    }

    let mut sector = [0u8; 512];
    if device.read(0, &mut sector).is_err() {
        return None;
    }

    if !partition::has_signature(&sector) {
        return None;
    }

    let entries = partition::decode_entries(&sector);
    partition::find_entry(&entries, TWILIGHT_PARTITION_TYPE)
}

pub fn format_superblock(
    block_device: &'static mut dyn BlockDeviceIO,
    partition_start_lba: u32,
    partition_sector_count: u32,
) -> Result<TwilightFs, &'static str> {
    set_fs_block_offset_lba(partition_start_lba);
    let sb = Superblock::write(block_device, partition_sector_count)?;
    let device_box: Box<dyn BlockDeviceIO + Send + 'static> =
        unsafe { Box::from_raw(block_device as *mut _) };
    let device_arc = Arc::new(Mutex::new(device_box));
    Ok(TwilightFs {
        superblock: sb,
        device: device_arc,
        shared: Arc::new(TwilightFsShared::new()),
    })
}

pub fn read_superblock(device: &mut dyn BlockDeviceIO) -> Result<Superblock, &'static str> {
    let mut buf = [0u8; FS_BLOCK_SIZE];
    if read_tfs_block(device, 0, &mut buf).is_err() {
        return Err("Failed to read TwilightFS superblock");
    }
    let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };

    if !sb.is_valid() {
        return Err("Invalid TwiligtFS magic");
    }
    Ok(sb)
}

#[derive(Debug)]
pub enum TfsError {
    InvalidPath,
    FileNotFound,
    FileAlreadyExists,
    FileNameTooLong,
    NoSpaceLeft,
    IoError,
    InvalidInode,
    InvalidZone,
}

#[derive(Default)]
struct PathLookupCache {
    generation: usize,
    map: BTreeMap<String, u32>,
    order: VecDeque<String>,
}

impl PathLookupCache {
    fn ensure_generation(&mut self, generation: usize) {
        if self.generation != generation {
            self.generation = generation;
            self.map.clear();
            self.order.clear();
        }
    }

    fn get(&mut self, generation: usize, path: &str) -> Option<u32> {
        self.ensure_generation(generation);
        self.map.get(path).copied()
    }

    fn insert(&mut self, generation: usize, path: String, ino: u32) {
        self.ensure_generation(generation);

        if self.map.contains_key(path.as_str()) {
            return;
        }

        self.map.insert(path.clone(), ino);
        self.order.push_back(path);

        while self.order.len() > PATH_LOOKUP_CACHE_CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(old.as_str());
            }
        }
    }
}

#[derive(Default)]
struct FileContentCache {
    generation: usize,
    total_bytes: usize,
    map: BTreeMap<u32, Vec<u8>>,
    order: VecDeque<u32>,
}

impl FileContentCache {
    fn ensure_generation(&mut self, generation: usize) {
        if self.generation != generation {
            self.generation = generation;
            self.total_bytes = 0;
            self.map.clear();
            self.order.clear();
        }
    }

    fn get_slice(
        &mut self,
        generation: usize,
        inode_no: u32,
        offset: usize,
        buf: &mut [u8],
    ) -> Option<usize> {
        self.ensure_generation(generation);
        let data = self.map.get(&inode_no)?;
        if offset >= data.len() {
            return Some(0);
        }
        let n = core::cmp::min(buf.len(), data.len() - offset);
        buf[..n].copy_from_slice(&data[offset..offset + n]);
        Some(n)
    }

    fn insert(&mut self, generation: usize, inode_no: u32, data: Vec<u8>) {
        self.ensure_generation(generation);

        let size = data.len();
        if size == 0 || size > FILE_CACHE_MAX_FILE_BYTES || size > FILE_CACHE_MAX_TOTAL_BYTES {
            return;
        }

        if self.map.contains_key(&inode_no) {
            return;
        }

        while self.total_bytes + size > FILE_CACHE_MAX_TOTAL_BYTES {
            let Some(evict) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.map.remove(&evict) {
                self.total_bytes = self.total_bytes.saturating_sub(old.len());
            }
        }

        self.total_bytes += size;
        self.map.insert(inode_no, data);
        self.order.push_back(inode_no);
    }
}

pub(crate) struct TwilightFsShared {
    generation: AtomicUsize,
    lookup_cache: Mutex<PathLookupCache>,
    file_cache: Mutex<FileContentCache>,
}

impl TwilightFsShared {
    fn new() -> Self {
        Self {
            generation: AtomicUsize::new(0),
            lookup_cache: Mutex::new(PathLookupCache::default()),
            file_cache: Mutex::new(FileContentCache::default()),
        }
    }

    #[inline]
    pub(crate) fn generation(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn invalidate_all(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    #[inline]
    pub(crate) fn lookup_cached(&self, path: &str) -> Option<u32> {
        let generation = self.generation();
        self.lookup_cache.lock().get(generation, path)
    }

    #[inline]
    pub(crate) fn insert_lookup(&self, path: String, ino: u32) {
        let generation = self.generation();
        self.lookup_cache.lock().insert(generation, path, ino);
    }

    #[inline]
    pub(crate) fn read_cached_file_slice(
        &self,
        inode_no: u32,
        offset: usize,
        buf: &mut [u8],
    ) -> Option<usize> {
        let generation = self.generation();
        self.file_cache
            .lock()
            .get_slice(generation, inode_no, offset, buf)
    }

    #[inline]
    pub(crate) fn insert_file_cache(&self, inode_no: u32, data: Vec<u8>) {
        let generation = self.generation();
        self.file_cache.lock().insert(generation, inode_no, data);
    }
}

#[derive(Clone)]
pub struct TwilightFs {
    pub superblock: Superblock,
    pub device: BlockDev,
    pub(crate) shared: Arc<TwilightFsShared>,
}

impl TwilightFs {
    pub fn resolve_path(&mut self, path: &str) -> Result<u32, FsError> {
        if path.is_empty() {
            return Err(FsError::InvalidPath);
        }

        if path == "/" {
            return Ok(1);
        }

        let mut canonical = String::new();
        if !path.starts_with('/') {
            canonical.push('/');
        }
        canonical.push_str(path);

        if let Some(ino) = self.shared.lookup_cached(canonical.as_str()) {
            return Ok(ino);
        }

        // Start from root inode (assumed to be inode number 1)
        let mut current_inode = 1;

        // Skip empty and root path
        let path_parts = canonical.split('/').filter(|s| !s.is_empty());
        let mut prefix = String::from("/");

        for part in path_parts {
            let next = self
                .find_dir_entry(current_inode, part)
                .map_err(|_| FsError::InvalidInode)?;

            let Some(inode) = next else {
                return Err(FileNotFound);
            };

            if prefix.len() > 1 {
                prefix.push('/');
            }
            prefix.push_str(part);
            self.shared.insert_lookup(prefix.clone(), inode);
            current_inode = inode;
        }

        self.shared.insert_lookup(canonical, current_inode);
        Ok(current_inode)
    }

    pub fn check_ata(bus: u8, dsk: u8) -> Result<Self, &'static str> {
        if let Some(entry) = detect_twilight_partition(bus, dsk) {
            set_fs_block_offset_lba(entry.lba_start);
        } else {
            set_fs_block_offset_bytes(0);
        }

        let mut device =
            driver::disk::AtaBlockDevice::new(bus, dsk).ok_or("Failed to open ATA device")?;

        let mut buf = [0u8; FS_BLOCK_SIZE];
        if read_tfs_block(&mut device, 0, &mut buf).is_err() {
            return Err("Failed to read Twilight FS superblock");
        }

        let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };
        if !sb.is_valid() {
            return Err("Invalid Twilight FS superblock magic");
        }

        let device_box: Box<dyn BlockDeviceIO + Send + 'static> = Box::new(device);
        let device_arc = Arc::new(Mutex::new(device_box));

        Ok(TwilightFs {
            superblock: sb,
            device: device_arc,
            shared: Arc::new(TwilightFsShared::new()),
        })
    }
    pub fn check_virtio_blk() -> Result<Self, &'static str> {
        if let Some(entry) = detect_twilight_partition_blk_dev() {
            set_fs_block_offset_lba(entry.lba_start);
        } else {
            set_fs_block_offset_bytes(0);
        }

        let mut device = VirtioBlkHandle;
        let mut buf = [0u8; FS_BLOCK_SIZE];
        if read_tfs_block(&mut device, 0, &mut buf).is_err() {
            return Err("Failed to read Twilight FS superblock");
        }

        let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };
        if !sb.is_valid() {
            return Err("Invalid Twilight FS superblock magic");
        }

        let device_box: Box<dyn BlockDeviceIO + Send + 'static> = Box::new(VirtioBlkHandle);
        let device_arc = Arc::new(Mutex::new(device_box));

        Ok(TwilightFs {
            superblock: sb,
            device: device_arc,
            shared: Arc::new(TwilightFsShared::new()),
        })
    }

    pub fn check_usb_blk() -> Result<Self, &'static str> {
        let mut device = UsbBlkHandle;
        if device.block_size() == 0 || device.block_count() == 0 {
            return Err("USB block device not available");
        }

        if let Some(entry) = detect_twilight_partition_on_device(&mut device) {
            set_fs_block_offset_lba(entry.lba_start);
        } else {
            set_fs_block_offset_bytes(0);
        }

        let mut buf = [0u8; FS_BLOCK_SIZE];
        if read_tfs_block(&mut device, 0, &mut buf).is_err() {
            return Err("Failed to read USB Twilight FS superblock");
        }

        let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };
        if !sb.is_valid() {
            return Err("Invalid Twilight FS superblock magic on USB");
        }

        let device_box: Box<dyn BlockDeviceIO + Send + 'static> = Box::new(UsbBlkHandle);
        let device_arc = Arc::new(Mutex::new(device_box));

        let mut fs = TwilightFs {
            superblock: sb,
            device: device_arc,
            shared: Arc::new(TwilightFsShared::new()),
        };
        fs.validate_root_inode()?;
        Ok(fs)
    }

    pub fn format_usb_blk() -> Result<Self, &'static str> {
        let mut device = UsbBlkHandle;
        let block_size = device.block_size();
        let block_count = device.block_count();
        if block_size == 0 || block_count == 0 {
            return Err("USB block device not available");
        }

        let total_sectors = block_count.min(u32::MAX as usize) as u32;
        if total_sectors <= 4096 {
            return Err("USB device too small");
        }

        let (partition_start_lba, partition_sectors) = if block_size == 512 {
            if let Some(entry) = detect_twilight_partition_on_device(&mut device) {
                (entry.lba_start, entry.sectors)
            } else {
                let mut mbr = [0u8; 512];
                let start_lba = 2048u32;
                if total_sectors <= start_lba + 2048 {
                    return Err("USB device too small for partitioned TwilightFS");
                }
                let sectors = total_sectors - start_lba;
                let mut entries = [PartitionEntry::empty(); 4];
                entries[0] =
                    PartitionEntry::new(0x00, TWILIGHT_PARTITION_TYPE, start_lba, sectors);
                partition::encode_entries(&mut mbr, &entries);
                partition::write_signature(&mut mbr);
                device
                    .write(0, &mbr)
                    .map_err(|_| "Failed to write USB partition table")?;
                (start_lba, sectors)
            }
        } else {
            (0, total_sectors)
        };

        set_fs_block_offset_lba(partition_start_lba);
        let sb = Superblock::write(&mut device, partition_sectors)?;
        let zero = [0u8; FS_BLOCK_SIZE];
        for block in 1..sb.first_data_zone {
            write_tfs_block(&mut device, block, &zero)
                .map_err(|_| "Failed to clear USB TwilightFS metadata")?;
        }

        let device_box: Box<dyn BlockDeviceIO + Send + 'static> = Box::new(UsbBlkHandle);
        let device_arc = Arc::new(Mutex::new(device_box));

        let mut fs = TwilightFs {
            superblock: sb,
            device: device_arc,
            shared: Arc::new(TwilightFsShared::new()),
        };
        fs.initialize_root_inode()?;
        Ok(fs)
    }

    fn validate_root_inode(&mut self) -> Result<(), &'static str> {
        let root_inode = self
            .read_inode(1)
            .map_err(|_| "Failed to read USB TwilightFS root inode")?;
        if !root_inode.is_dir() {
            return Err("USB TwilightFS root inode is invalid");
        }
        Ok(())
    }

    fn initialize_root_inode(&mut self) -> Result<(), &'static str> {
        let root_inode_num = self
            .allocate_inode()
            .map_err(|_| "Failed to allocate USB TwilightFS root inode")?
            + 1;
        if root_inode_num != 1 {
            return Err("Unexpected USB TwilightFS root inode number");
        }

        let root_zone = self
            .allocate_zone()
            .map_err(|_| "Failed to allocate USB TwilightFS root directory zone")?;
        let mut root_inode = Inode::new_dir(CMOS::new().unix_time(), 0o755);
        root_inode.direct_slot_set(0, root_zone);

        self.write_inode(root_inode_num, &root_inode)?;
        self.create_dir_entry(root_inode_num, ".", root_inode_num)?;
        self.create_dir_entry(root_inode_num, "..", root_inode_num)?;
        self.shared.invalidate_all();
        Ok(())
    }

    pub fn allocate_zone(&mut self) -> Result<u32, TfsError> {
        let bits_per_block = self.superblock.block_size as usize * 8;
        // Layout: block 0 superblock, then imap, then zmap, then inode table.
        // So zmap starts right after the imap.
        let zmap_start = self.superblock.imap_blocks + 1;
        let max_data_zones = self
            .superblock
            .zones
            .saturating_sub(self.superblock.first_data_zone);

        let mut buf = [0u8; FS_BLOCK_SIZE];
        for i in 0..self.superblock.zmap_blocks {
            if read_tfs_block(self.device.lock().as_mut(), zmap_start + i, &mut buf).is_err() {
                return Err(TfsError::IoError);
            }

            for byte_idx in 0..buf.len() {
                if buf[byte_idx] != 0xFF {
                    for bit in 0..8 {
                        if buf[byte_idx] & (1 << bit) == 0 {
                            let zone = i * bits_per_block as u32 + (byte_idx * 8 + bit) as u32;
                            if zone >= max_data_zones {
                                return Err(TfsError::NoSpaceLeft);
                            }
                            buf[byte_idx] |= 1 << bit;
                            if write_tfs_block(self.device.lock().as_mut(), zmap_start + i, &buf)
                                .is_err()
                            {
                                return Err(TfsError::IoError);
                            }

                            return Ok(zone + self.superblock.first_data_zone);
                        }
                    }
                }
            }
        }

        Err(TfsError::NoSpaceLeft)
    }

    pub fn allocate_inode(&mut self) -> Result<u32, TfsError> {
        let bits_per_block = self.superblock.block_size as usize * 8;
        let total_inodes = self.superblock.ninodes as usize;

        for block_idx in 0..self.superblock.imap_blocks {
            let imap_block_lba = 1 + block_idx;
            let mut buf = [0u8; FS_BLOCK_SIZE];
            if read_tfs_block(self.device.lock().as_mut(), imap_block_lba, &mut buf).is_err() {
                return Err(TfsError::IoError);
            }

            for byte_idx in 0..self.superblock.block_size as usize {
                let byte = buf[byte_idx];

                if byte != 0xFF {
                    for bit in 0..8 {
                        if byte & (1 << bit) == 0 {
                            let inode_idx =
                                (block_idx as usize * bits_per_block) + (byte_idx * 8) + bit;
                            if inode_idx >= total_inodes {
                                break;
                            }

                            buf[byte_idx] |= 1 << bit;
                            if write_tfs_block(self.device.lock().as_mut(), imap_block_lba, &buf)
                                .is_err()
                            {
                                return Err(TfsError::IoError);
                            }
                            return Ok(inode_idx as u32);
                        }
                    }
                }
            }
        }

        Err(TfsError::NoSpaceLeft)
    }

    pub fn dealloc_zone(&mut self, zone: u32) -> Result<(), TfsError> {
        let first_zone = self.superblock.first_data_zone;

        if zone < first_zone {
            return Err(TfsError::InvalidZone);
        }

        let relative_zone = zone - first_zone;
        let bits_per_block = self.superblock.block_size as usize * 8;
        let block_index = (relative_zone as usize) / bits_per_block;
        let bit_index = (relative_zone as usize) % bits_per_block;
        let byte_index = bit_index / 8;
        let bit = bit_index % 8;

        if block_index >= self.superblock.zmap_blocks as usize {
            return Err(TfsError::InvalidZone);
        }

        // Layout: block 0 superblock, then imap, then zmap.
        let zmap_start = 1 + self.superblock.imap_blocks;
        let zmap_block = zmap_start + block_index as u32;

        let mut buf = [0u8; FS_BLOCK_SIZE];
        if read_tfs_block(self.device.lock().as_mut(), zmap_block, &mut buf).is_err() {
            return Err(TfsError::IoError);
        }

        buf[byte_index] &= !(1 << bit);

        if write_tfs_block(self.device.lock().as_mut(), zmap_block, &buf).is_err() {
            return Err(TfsError::IoError);
        }

        Ok(())
    }

    pub fn dealloc_inode(&mut self, inode: u32) -> Result<(), TfsError> {
        if inode == 0 || inode as usize > self.superblock.ninodes as usize {
            return Err(TfsError::InvalidInode);
        }

        let inode_index = inode as usize - 1; // MINIX inodes are 1-based
        let bits_per_block = self.superblock.block_size as usize * 8;

        let block_index = inode_index / bits_per_block;
        let bit_index = inode_index % bits_per_block;
        let byte_index = bit_index / 8;
        let bit_in_byte = bit_index % 8;

        let imap_block_lba = 1 + block_index as u32;
        let mut buffer = [0u8; FS_BLOCK_SIZE];
        if read_tfs_block(self.device.lock().as_mut(), imap_block_lba, &mut buffer).is_err() {
            return Err(TfsError::IoError);
        }

        buffer[byte_index] &= !(1 << bit_in_byte); // clear the bit

        if write_tfs_block(self.device.lock().as_mut(), imap_block_lba, &buffer).is_err() {
            return Err(TfsError::IoError);
        }

        Ok(())
    }

    // TODO: move this to inode impl
    pub fn write_inode(&mut self, inode_num: u32, inode: &Inode) -> Result<(), &'static str> {
        if inode_num == 0 || inode_num as usize > self.superblock.ninodes as usize {
            return Err("Invalid inode number");
        }

        let inode_index = (inode_num - 1) as usize;
        let inode_size = size_of::<Inode>();
        let block_size = self.superblock.block_size as usize;
        let inodes_per_block = block_size / inode_size;

        // Layout: block 0 superblock, then imap, then zmap, then inode table.
        let inode_table_start = self.superblock.imap_blocks + self.superblock.zmap_blocks + 1;
        let block_offset = inode_index / inodes_per_block;
        let byte_offset = (inode_index % inodes_per_block) * inode_size;
        let block_num = inode_table_start + block_offset as u32;

        let mut buffer = [0u8; FS_BLOCK_SIZE];
        if read_tfs_block(self.device.lock().as_mut(), block_num, &mut buffer).is_err() {
            return Err("Failed to read inode block");
        }

        let inode_bytes = unsafe {
            core::slice::from_raw_parts(inode as *const _ as *const u8, size_of::<Inode>())
        };
        buffer[byte_offset..byte_offset + inode_size].copy_from_slice(inode_bytes);

        if write_tfs_block(self.device.lock().as_mut(), block_num, &buffer).is_err() {
            return Err("Failed to write inode block");
        }

        Ok(())
    }

    // TODO: move this to inode impl
    pub fn read_inode(&mut self, inode_num: u32) -> Result<Inode, &'static str> {
        if inode_num == 0 || inode_num as usize > self.superblock.ninodes as usize {
            return Err("Invalid inode number");
        }

        let inode_index = (inode_num - 1) as usize;
        let inode_size = size_of::<Inode>();
        let block_size = self.superblock.block_size as usize;
        let inodes_per_block = block_size / inode_size;

        // Layout: block 0 superblock, then imap, then zmap, then inode table.
        let inode_table_start = self.superblock.imap_blocks + self.superblock.zmap_blocks + 1;
        let block_offset = inode_index / inodes_per_block;
        let byte_offset = (inode_index % inodes_per_block) * inode_size;
        let block_num = inode_table_start + block_offset as u32;

        let mut buffer = [0u8; FS_BLOCK_SIZE];
        if read_tfs_block(self.device.lock().as_mut(), block_num, &mut buffer).is_err() {
            return Err("Failed to read inode block");
        }

        let inode_bytes = unsafe {
            core::slice::from_raw_parts(
                buffer[byte_offset..byte_offset + inode_size].as_ptr() as *const _,
                size_of::<Inode>(),
            )
        };
        let inode: Inode = unsafe { core::ptr::read(inode_bytes.as_ptr() as *const _) };

        Ok(inode)
    }

    // TODO: move this to DirEntry impl
    pub fn create_dir_entry(
        &mut self,
        parent_inode_num: u32,
        name: &str,
        child_inode_num: u32,
    ) -> Result<(), &'static str> {
        let mut parent_inode = self.read_inode(parent_inode_num)?;

        let dir_entry_size = size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;

        let mut entry_added = false;
        let name_bytes = {
            let mut name_buf = [0u8; 60];
            let name_bytes = name.as_bytes();
            let len = name_bytes.len().min(60);
            name_buf[..len].copy_from_slice(&name_bytes[..len]);
            name_buf
        };

        let entry = DirEntry {
            inode: child_inode_num,
            name: name_bytes,
        };

        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let mut block = parent_inode.direct_slot_get(i);
            if block == 0 {
                block = self
                    .allocate_zone()
                    .map_err(|_| "Failed to allocate directory zone")?;
                parent_inode.direct_slot_set(i, block);

                // Directory scans rely on empty slots being zeroed.
                let zero = [0u8; FS_BLOCK_SIZE];
                if write_tfs_block(self.device.lock().as_mut(), block, &zero).is_err() {
                    return Err("Failed to initialize directory block");
                }

                self.write_inode(parent_inode_num, &parent_inode)?;
            }

            let mut buf = [0u8; FS_BLOCK_SIZE];
            if read_tfs_block(self.device.lock().as_mut(), block, &mut buf).is_err() {
                return Err("Failed to read block");
            }
            if i == 0 && parent_inode.size == 0 {
                buf.fill(0);
            }

            for j in 0..entries_per_block {
                let offset = j * dir_entry_size;
                let inode_field = u32::from_le_bytes([
                    buf[offset],
                    buf[offset + 1],
                    buf[offset + 2],
                    buf[offset + 3],
                ]);
                if inode_field == 0 {
                    // Found empty slot
                    let entry_bytes = unsafe {
                        core::slice::from_raw_parts(&entry as *const _ as *const u8, dir_entry_size)
                    };
                    buf[offset..offset + dir_entry_size].copy_from_slice(entry_bytes);
                    if write_tfs_block(self.device.lock().as_mut(), block, &buf).is_err() {
                        return Err("Failed to write block");
                    }
                    parent_inode.size += dir_entry_size as u64;
                    self.write_inode(parent_inode_num, &parent_inode)?;

                    entry_added = true;
                    break;
                }
            }

            if entry_added {
                return Ok(());
            }
        }

        Err("Directory is full")
    }

    // TODO: move this to DirEntry impl
    pub fn read_dir_entries(&mut self, inode: &Inode) -> Result<Vec<DirEntry>, &'static str> {
        let dir_entry_size = size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;
        let mut entries = Vec::new();

        let mut buf = [0u8; FS_BLOCK_SIZE];

        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let zone = inode.direct_slot_get(i);
            if zone == 0 {
                continue;
            }

            if read_tfs_block(self.device.lock().as_mut(), zone, &mut buf).is_err() {
                return Err("Failed to read block");
            }
            for i in 0..entries_per_block {
                let offset = i * dir_entry_size;
                let raw = &buf[offset..offset + dir_entry_size];
                let inode = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                if inode == 0 {
                    continue;
                }

                let mut name = [0u8; 60];
                name.copy_from_slice(&raw[4..64]);

                entries.push(DirEntry { inode, name });
            }
        }

        Ok(entries)
    }

    // TODO: move this to DirEntry impl
    pub fn create_file(&mut self, parent_inode_num: u32, name: &str) -> Result<u32, FsError> {
        if name.len() > 60 {
            return Err(FileNotFound);
        }

        // --- Check if file already exists ---
        let parent_inode = self.read_inode(parent_inode_num).unwrap();
        let entries = self.read_dir_entries(&parent_inode).unwrap();

        for entry in &entries {
            let existing_name = core::str::from_utf8(&entry.name)
                .unwrap_or("")
                .trim_end_matches('\0');

            if existing_name == name {
                return Err(FileAlreadyExists);
            }
        }

        // Allocate inode and zone
        let new_inode_num = self.allocate_inode().unwrap() + 1;
        let new_zone = self.allocate_zone().unwrap();

        let time = CMOS::new().unix_time();

        // Initialize inode
        let mut inode = Inode::new_file(time, 0o777);
        // Inherit encryption flag from parent directory
        if (parent_inode.flags & inode::IFLAG_ENCRYPTED) != 0 {
            inode.flags |= inode::IFLAG_ENCRYPTED;
        }
        inode.direct_slot_set(0, new_zone);

        self.write_inode(new_inode_num, &inode).unwrap();

        self.create_dir_entry(parent_inode_num, name, new_inode_num)
            .unwrap();

        self.shared.invalidate_all();
        Ok(new_inode_num)
    }

    pub fn write_file(&mut self, inode_num: u32, data: &[u8]) -> Result<(), FsError> {
        if inode_num == 0 || inode_num as usize > self.superblock.ninodes as usize {
            return Err(InvalidInode);
        }

        let mut inode = self.read_inode(inode_num).unwrap();
        let block_size = self.superblock.block_size as usize;

        let mut bytes_written = 0;
        let mut remaining = data.len();
        let mut direct_zones = [0u32; Inode::DIRECT_SLOT_COUNT];
        for (i, slot) in direct_zones.iter_mut().enumerate() {
            *slot = inode.direct_slot_get(i);
        }

        for i in 0..direct_zones.len() {
            if remaining == 0 {
                break;
            }

            if direct_zones[i] == 0 {
                let zone = self.allocate_zone().unwrap();
                direct_zones[i] = zone;
            }

            let block = direct_zones[i];
            let mut buffer = [0u8; FS_BLOCK_SIZE];

            let copy_size = core::cmp::min(block_size, remaining);
            buffer[..copy_size].copy_from_slice(&data[bytes_written..bytes_written + copy_size]);

            write_tfs_block(self.device.lock().as_mut(), block, &buffer)?;

            bytes_written += copy_size;
            remaining -= copy_size;
        }

        // if space in direct zones is filled, use indirect nodes
        if remaining > 0 {
            if inode.single_indirect_get() == 0 {
                let zone = self.allocate_zone().unwrap();
                inode.single_indirect_set(zone);
                let zero_block = [0u8; FS_BLOCK_SIZE];
                write_tfs_block(self.device.lock().as_mut(), zone, &zero_block)?;
            }

            let mut indirect_block = [0u8; FS_BLOCK_SIZE];
            read_tfs_block(
                self.device.lock().as_mut(),
                inode.single_indirect_get(),
                &mut indirect_block,
            )?;

            let zone_entries = FS_BLOCK_SIZE / 4;
            for i in 0..zone_entries {
                if remaining == 0 {
                    break;
                }

                let entry = u32::from_le_bytes([
                    indirect_block[i * 4],
                    indirect_block[i * 4 + 1],
                    indirect_block[i * 4 + 2],
                    indirect_block[i * 4 + 3],
                ]);

                let zone = if entry == 0 {
                    let new_zone = self.allocate_zone().unwrap();
                    indirect_block[i * 4..i * 4 + 4].copy_from_slice(&new_zone.to_le_bytes());
                    new_zone
                } else {
                    entry
                };

                let mut buffer = [0u8; FS_BLOCK_SIZE];
                let copy_size = core::cmp::min(block_size, remaining);

                buffer[..copy_size]
                    .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);
                write_tfs_block(self.device.lock().as_mut(), zone, &buffer)?;

                bytes_written += copy_size;
                remaining -= copy_size;
            }

            // store updated indirect block
            write_tfs_block(
                self.device.lock().as_mut(),
                inode.single_indirect_get(),
                &indirect_block,
            )?;
        }

        if remaining > 0 {
            if inode.double_indirect_get() == 0 {
                inode.double_indirect_set(self.allocate_zone().unwrap());
                let zero_block = [0u8; FS_BLOCK_SIZE];
                write_tfs_block(
                    self.device.lock().as_mut(),
                    inode.double_indirect_get(),
                    &zero_block,
                )?;
            }

            let mut double_indirect_block = [0u8; FS_BLOCK_SIZE];
            read_tfs_block(
                self.device.lock().as_mut(),
                inode.double_indirect_get(),
                &mut double_indirect_block,
            )?;

            let zone_entries = FS_BLOCK_SIZE / 4;
            for i in 0..zone_entries {
                if remaining == 0 {
                    break;
                }

                let indirect_zone = {
                    let entry = u32::from_le_bytes([
                        double_indirect_block[i * 4],
                        double_indirect_block[i * 4 + 1],
                        double_indirect_block[i * 4 + 2],
                        double_indirect_block[i * 4 + 3],
                    ]);
                    if entry == 0 {
                        let new_zone = self.allocate_zone().unwrap();
                        double_indirect_block[i * 4..i * 4 + 4]
                            .copy_from_slice(&new_zone.to_le_bytes());
                        let zero_block = [0u8; FS_BLOCK_SIZE];
                        write_tfs_block(self.device.lock().as_mut(), new_zone, &zero_block)?;
                        new_zone
                    } else {
                        entry
                    }
                };

                let mut indirect_block = [0u8; FS_BLOCK_SIZE];
                read_tfs_block(
                    self.device.lock().as_mut(),
                    indirect_zone,
                    &mut indirect_block,
                )?;

                let zone_entries = FS_BLOCK_SIZE / 4;
                for j in 0..zone_entries {
                    if remaining == 0 {
                        break;
                    }

                    let zone = {
                        let entry = u32::from_le_bytes([
                            indirect_block[j * 4],
                            indirect_block[j * 4 + 1],
                            indirect_block[j * 4 + 2],
                            indirect_block[j * 4 + 3],
                        ]);
                        if entry == 0 {
                            let new_zone = self.allocate_zone().unwrap();
                            indirect_block[j * 4..j * 4 + 4]
                                .copy_from_slice(&new_zone.to_le_bytes());
                            let zero_block = [0u8; FS_BLOCK_SIZE];
                            write_tfs_block(self.device.lock().as_mut(), new_zone, &zero_block)?;
                            new_zone
                        } else {
                            entry
                        }
                    };

                    let mut buffer = [0u8; FS_BLOCK_SIZE];
                    let copy_size = core::cmp::min(block_size, remaining);

                    buffer[..copy_size]
                        .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);
                    write_tfs_block(self.device.lock().as_mut(), zone, &buffer)?;

                    bytes_written += copy_size;
                    remaining -= copy_size;
                }

                // store updated indirect block
                write_tfs_block(self.device.lock().as_mut(), indirect_zone, &indirect_block)?;
            }

            // store updated double indirect block
            write_tfs_block(
                self.device.lock().as_mut(),
                inode.double_indirect_get(),
                &double_indirect_block,
            )?;
        }

        for (i, zone) in direct_zones.iter().copied().enumerate() {
            inode.direct_slot_set(i, zone);
        }
        inode.size = bytes_written as u64;
        self.write_inode(inode_num, &inode).unwrap();

        self.shared.invalidate_all();
        Ok(())
    }

    pub fn list_dir(&mut self, dir_inode_num: u32) -> Result<Vec<Metadata>, &'static str> {
        let dir_inode = self.read_inode(dir_inode_num)?;
        let mut entries = Vec::new();

        const DIR_ENTRY_SIZE: usize = size_of::<DirEntry>();

        let mut buffer = [0u8; FS_BLOCK_SIZE];

        let mut bytes_processed = 0;
        let total_size = dir_inode.size as usize;
        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let zone_num = dir_inode.direct_slot_get(i);
            if zone_num == 0 {
                break;
            }

            if read_tfs_block(self.device.lock().as_mut(), zone_num, &mut buffer).is_err() {
                return Err("Failed to read directory data block");
            }

            for chunk in buffer.chunks_exact(DIR_ENTRY_SIZE) {
                if bytes_processed >= total_size {
                    break;
                }

                let entry = unsafe { &*(chunk.as_ptr() as *const DirEntry) };

                if entry.inode == 0 {
                    continue;
                } else {
                    bytes_processed += DIR_ENTRY_SIZE;
                }

                let inode = self.read_inode(entry.inode)?;

                let file_type = if inode.is_dir() {
                    FileType::Dir
                } else {
                    FileType::File
                };

                let name_end = entry
                    .name
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(entry.name.len());
                let name_bytes = &entry.name[..name_end];

                match core::str::from_utf8(name_bytes) {
                    Ok(name) => {
                        if !name.is_empty() {
                            entries.push(Metadata {
                                name: String::from(name),
                                ino: entry.inode,
                                gid: inode.gid,
                                uid: inode.uid,
                                size: inode.size as usize,
                                file_type,
                                access_time: to_u32_saturating(inode.access_time),
                                created_time: to_u32_saturating(inode.created_time),
                                modified_time: to_u32_saturating(inode.modified_time),
                            });
                        }
                    }
                    Err(_) => {
                        continue;
                    }
                }
            }
        }

        Ok(entries)
    }

    // TODO: move this to DirEntry impl
    pub fn create_dir(&mut self, parent_inode_num: u32, name: &str) -> Result<u32, FsError> {
        if name.len() > 60 {
            return Err(FileNameTooLong);
        }

        // Check if directory with same name already exists
        let parent_inode = self.read_inode(parent_inode_num).unwrap();
        let entries = self.read_dir_entries(&parent_inode).unwrap();

        for entry in &entries {
            let existing_name = core::str::from_utf8(&entry.name)
                .unwrap_or("")
                .trim_end_matches('\0');

            if existing_name == name {
                return Err(FileAlreadyExists);
            }
        }

        // Allocate inode and zone for the new directory
        let new_inode_num = self.allocate_inode().unwrap() + 1;
        let new_zone = self.allocate_zone().unwrap();

        let time = CMOS::new().unix_time();

        // Create the new directory inode
        let mut inode = Inode::new_dir(time, 0o777);
        if (parent_inode.flags & inode::IFLAG_ENCRYPTED) != 0 {
            inode.flags |= inode::IFLAG_ENCRYPTED;
        }
        inode.direct_slot_set(0, new_zone);
        self.write_inode(new_inode_num, &inode).unwrap();

        self.create_dir_entry(parent_inode_num, name, new_inode_num)
            .unwrap();

        self.create_dir_entry(new_inode_num, ".", new_inode_num)
            .unwrap();
        self.create_dir_entry(new_inode_num, "..", parent_inode_num)
            .unwrap();

        self.shared.invalidate_all();
        Ok(new_inode_num)
    }

    pub fn find_dir_entry(
        &mut self,
        parent_inode_num: u32,
        name: &str,
    ) -> Result<Option<u32>, &'static str> {
        let parent_inode = self.read_inode(parent_inode_num)?;

        if parent_inode.direct_slot_get(0) == 0 {
            return Ok(None);
        }

        let dir_entry_size = size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;
        let mut buffer = [0u8; FS_BLOCK_SIZE];

        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let zone = parent_inode.direct_slot_get(i);
            if zone == 0 {
                continue;
            }

            read_tfs_block(self.device.lock().as_mut(), zone, &mut buffer).unwrap();

            for i in 0..entries_per_block {
                let offset = i * dir_entry_size;
                let entry =
                    unsafe { core::ptr::read(buffer[offset..].as_ptr() as *const DirEntry) };

                if entry.inode != 0 {
                    let entry_name = core::str::from_utf8(&entry.name)
                        .unwrap_or("")
                        .trim_end_matches('\0');

                    if entry_name == name {
                        return Ok(Some(entry.inode));
                    }
                }
            }
        }

        Ok(None)
    }

    fn split_parent_and_name(path: &str) -> Result<(String, String), FsError> {
        let mut components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        if components.is_empty() {
            return Err(FsError::InvalidPath);
        }

        let name = components.pop().unwrap();
        if name.is_empty() || name == "." || name == ".." {
            return Err(FsError::InvalidPath);
        }

        let parent = if components.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", components.join("/"))
        };

        Ok((parent, name.to_string()))
    }

    fn find_dir_entry_slot(
        &mut self,
        parent_inode_num: u32,
        name: &str,
    ) -> Result<Option<(u32, usize, u32)>, FsError> {
        let parent_inode = self.read_inode(parent_inode_num).map_err(|_| InvalidInode)?;
        let dir_entry_size = size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;
        let mut buffer = [0u8; FS_BLOCK_SIZE];

        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let zone = parent_inode.direct_slot_get(i);
            if zone == 0 {
                continue;
            }

            read_tfs_block(self.device.lock().as_mut(), zone, &mut buffer)?;

            for i in 0..entries_per_block {
                let offset = i * dir_entry_size;
                let entry =
                    unsafe { core::ptr::read(buffer[offset..].as_ptr() as *const DirEntry) };

                if entry.inode == 0 {
                    continue;
                }

                let entry_name = core::str::from_utf8(&entry.name)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                if entry_name == name {
                    return Ok(Some((zone, offset, entry.inode)));
                }
            }
        }

        Ok(None)
    }

    fn clear_dir_entry_slot(
        &mut self,
        parent_inode_num: u32,
        zone: u32,
        offset: usize,
    ) -> Result<(), FsError> {
        let mut parent_inode = self.read_inode(parent_inode_num).map_err(|_| InvalidInode)?;
        let dir_entry_size = size_of::<DirEntry>();
        let mut buf = [0u8; FS_BLOCK_SIZE];
        read_tfs_block(self.device.lock().as_mut(), zone, &mut buf)?;
        if offset + dir_entry_size > buf.len() {
            return Err(FsError::InvalidPath);
        }
        buf[offset..offset + dir_entry_size].fill(0);
        write_tfs_block(self.device.lock().as_mut(), zone, &buf)?;
        if parent_inode.size >= dir_entry_size as u64 {
            parent_inode.size -= dir_entry_size as u64;
            self.write_inode(parent_inode_num, &parent_inode)
                .map_err(|_| InvalidInode)?;
        }
        Ok(())
    }

    pub fn rename_entry(&mut self, old_path: &str, new_path: &str) -> Result<(), FsError> {
        if old_path == new_path {
            return Ok(());
        }

        let (old_parent_path, old_name) = Self::split_parent_and_name(old_path)?;
        let (new_parent_path, new_name) = Self::split_parent_and_name(new_path)?;

        if old_name.len() > 60 || new_name.len() > 60 {
            return Err(FileNameTooLong);
        }

        let old_parent_inode_num = self.resolve_path(old_parent_path.as_str())?;
        let new_parent_inode_num = self.resolve_path(new_parent_path.as_str())?;

        let old_parent_inode = self.read_inode(old_parent_inode_num).map_err(|_| InvalidInode)?;
        if !old_parent_inode.is_dir() {
            return Err(FsError::InvalidPath);
        }
        let new_parent_inode = self.read_inode(new_parent_inode_num).map_err(|_| InvalidInode)?;
        if !new_parent_inode.is_dir() {
            return Err(FsError::InvalidPath);
        }

        let Some((old_zone, old_offset, old_inode_num)) =
            self.find_dir_entry_slot(old_parent_inode_num, old_name.as_str())?
        else {
            return Err(FileNotFound);
        };

        let old_inode = self.read_inode(old_inode_num).map_err(|_| InvalidInode)?;
        if old_inode.is_dir() && old_parent_inode_num != new_parent_inode_num {
            return Err(FsError::NotSupported);
        }

        if let Some((_zone, _offset, existing_inode_num)) =
            self.find_dir_entry_slot(new_parent_inode_num, new_name.as_str())?
        {
            if existing_inode_num != old_inode_num {
                let existing_inode = self.read_inode(existing_inode_num).map_err(|_| InvalidInode)?;
                if existing_inode.is_dir() {
                    return Err(FileAlreadyExists);
                }
                self.remove_entry(new_path)?;
            } else if old_parent_inode_num == new_parent_inode_num {
                return Ok(());
            } else {
                return Err(FileAlreadyExists);
            }
        }

        let mut name_bytes = [0u8; 60];
        name_bytes[..new_name.len()].copy_from_slice(new_name.as_bytes());

        if old_parent_inode_num == new_parent_inode_num {
            let mut buf = [0u8; FS_BLOCK_SIZE];
            read_tfs_block(self.device.lock().as_mut(), old_zone, &mut buf)?;
            buf[old_offset + size_of::<u32>()..old_offset + size_of::<DirEntry>()]
                .copy_from_slice(&name_bytes);
            write_tfs_block(self.device.lock().as_mut(), old_zone, &buf)?;
        } else {
            self.create_dir_entry(new_parent_inode_num, new_name.as_str(), old_inode_num)
                .map_err(|_| InvalidInode)?;
            self.clear_dir_entry_slot(old_parent_inode_num, old_zone, old_offset)?;
        }

        self.shared.invalidate_all();
        Ok(())
    }

    pub fn read_file(&mut self, inode_num: u32) -> Result<Vec<u8>, &'static str> {
        let inode = self.read_inode(inode_num)?;

        let mut content = Vec::new();
        let mut remaining = inode.size as usize;
        let block_size = self.superblock.block_size as usize;
        let mut buffer = [0u8; FS_BLOCK_SIZE];

        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let zone = inode.direct_slot_get(i);
            if zone == 0 {
                break;
            }

            let to_read = core::cmp::min(remaining, block_size);

            read_tfs_block(self.device.lock().as_mut(), zone, &mut buffer).unwrap();
            content.extend_from_slice(&buffer[..to_read]);

            remaining -= to_read;
            if remaining == 0 {
                break;
            }
        }

        if inode.single_indirect_get() != 0 {
            read_tfs_block(
                self.device.lock().as_mut(),
                inode.single_indirect_get(),
                &mut buffer,
            )
            .unwrap();
            let zone_size = FS_BLOCK_SIZE / 4;
            for i in 0..(zone_size - 1) {
                let zone_id_buf: [u8; 4] = buffer[i * 4..(i + 1) * 4]
                    .try_into()
                    .expect("invalid zone id size");
                let zone_id = u32::from_le_bytes(zone_id_buf);
                if zone_id == 0 {
                    break;
                }

                let to_read = core::cmp::min(remaining, block_size);

                let mut indirect_content_buf = [0u8; FS_BLOCK_SIZE];

                read_tfs_block(
                    self.device.lock().as_mut(),
                    zone_id,
                    &mut indirect_content_buf,
                )
                .unwrap();

                content.extend_from_slice(&indirect_content_buf[..to_read]);

                remaining -= to_read;
                if remaining == 0 {
                    break;
                }
            }
        }

        if inode.double_indirect_get() != 0 {
            read_tfs_block(
                self.device.lock().as_mut(),
                inode.double_indirect_get(),
                &mut buffer,
            )
            .unwrap();
            let zone_size = FS_BLOCK_SIZE / 4;
            for i in 0..(zone_size - 1) {
                if remaining == 0 {
                    break;
                }
                let zone_id_buf: [u8; 4] = buffer[i * 4..(i + 1) * 4]
                    .try_into()
                    .expect("invalid zone id size");
                let zone_id = u32::from_le_bytes(zone_id_buf);
                if zone_id == 0 {
                    break;
                }

                let mut indirect_zones_buf = [0u8; FS_BLOCK_SIZE];
                read_tfs_block(
                    self.device.lock().as_mut(),
                    zone_id,
                    &mut indirect_zones_buf,
                )
                .unwrap();

                let zone_entries = FS_BLOCK_SIZE / 4;
                for j in 0..(zone_entries - 1) {
                    let zone_id_buf: [u8; 4] = indirect_zones_buf[j * 4..(j + 1) * 4]
                        .try_into()
                        .expect("invalid zone id size");
                    let zone_id = u32::from_le_bytes(zone_id_buf);
                    if zone_id == 0 {
                        break;
                    }

                    let to_read = core::cmp::min(remaining, block_size);

                    let mut zone_buf = [0u8; FS_BLOCK_SIZE];
                    read_tfs_block(self.device.lock().as_mut(), zone_id, &mut zone_buf).unwrap();

                    content.extend_from_slice(&zone_buf[..to_read]);

                    remaining -= to_read;
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }

        Ok(content)
    }

    pub fn remove_entry(&mut self, path: &str) -> Result<(), FsError> {
        let mut components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
        if components.is_empty() {
            return Err(FsError::InvalidPath);
        }

        let target_name = components.pop().unwrap();
        let parent_path = format!("/{}", components.join("/"));
        let parent_inode_num = if components.is_empty() {
            1 // root
        } else {
            self.resolve_path(&parent_path)?
        };

        let mut parent_inode = self.read_inode(parent_inode_num).unwrap();
        let dir_entry_size = size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;

        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let zone = parent_inode.direct_slot_get(i);
            if zone == 0 {
                continue;
            }

            let mut buf = [0u8; FS_BLOCK_SIZE];
            if read_tfs_block(self.device.lock().as_mut(), zone, &mut buf).is_err() {
                return Err(InvalidInode);
            }

            for i in 0..entries_per_block {
                let offset = i * dir_entry_size;
                let entry = unsafe { core::ptr::read(buf[offset..].as_ptr() as *const DirEntry) };

                let entry_name = core::str::from_utf8(&entry.name)
                    .unwrap_or("")
                    .trim_end_matches('\0');

                if entry.inode != 0 && entry_name == target_name {
                    let inode_num = entry.inode;
                    let inode = self.read_inode(inode_num).unwrap();

                    // Free all zones
                    for di in 0..Inode::DIRECT_SLOT_COUNT {
                        let z = inode.direct_slot_get(di);
                        if z != 0 {
                            self.free_zone(z).unwrap();
                        }
                    }

                    // Free inode
                    self.dealloc_inode(inode_num).unwrap();

                    buf[offset..offset + dir_entry_size].fill(0);
                    write_tfs_block(self.device.lock().as_mut(), zone, &buf)?;

                    // Update parent inode size if large enough
                    if parent_inode.size >= dir_entry_size as u64 {
                        parent_inode.size -= dir_entry_size as u64;
                    }
                    self.write_inode(parent_inode_num, &parent_inode).unwrap();

                    self.shared.invalidate_all();
                    return Ok(());
                }
            }
        }

        Err(FileNotFound)
    }
}

impl FsCtx for TwilightFs {
    fn block_size(&self) -> usize {
        self.superblock.block_size as usize
    }

    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        if buf.len() != self.block_size() {
            return Err(());
        }

        if let Err(_) = read_tfs_block(
            self.device.lock().as_mut(),
            lba,
            <&mut [u8; 2048]>::try_from(buf).unwrap(),
        ) {
            return Err(());
        }

        Ok(())
    }

    fn write_block(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()> {
        if buf.len() != self.block_size() {
            return Err(());
        }

        if let Err(_) = write_tfs_block(
            self.device.lock().as_mut(),
            lba,
            <&[u8; 2048]>::try_from(buf).unwrap(),
        ) {
            return Err(());
        }

        Ok(())
    }

    fn alloc_zone(&mut self) -> Result<u32, TfsError> {
        self.allocate_zone()
    }

    fn free_zone(&mut self, zone: u32) -> Result<(), TfsError> {
        self.dealloc_zone(zone)
    }

    fn write_inode_twilight(&mut self, ino: u32, inode: Inode) -> Result<(), &'static str> {
        self.write_inode(ino, &inode)
    }

    fn remove_file(&mut self, path: &str) -> Result<(), ()> {
        if self.remove_entry(path).is_err() {
            Err(())
        } else {
            Ok(())
        }
    }
}

impl FileSystem for TwilightFs {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()> {
        let inode_no = if path == "/" {
            1
        } else {
            self.resolve_path(path).or_else(|_| Err(()))?
        };

        if let Ok(inode) = self.read_inode(inode_no) {
            let file_type = if inode.is_dir() {
                FileType::Dir
            } else {
                FileType::File
            };
            let node = VfsNode::new(
                self.device.clone(),
                Metadata {
                    file_type,
                    size: inode.size as usize,
                    gid: inode.gid,
                    uid: inode.uid,
                    name: path.split("/").last().unwrap().to_string(),
                    ino: inode_no,
                    access_time: to_u32_saturating(inode.access_time),
                    created_time: to_u32_saturating(inode.created_time),
                    modified_time: to_u32_saturating(inode.modified_time),
                },
                Arc::new(RwLock::new(TFSVfsNode {
                    inode,
                    ctx: Arc::new(Mutex::new(self.clone())),
                    full_path: path.to_string(),
                    inode_no,
                    shared: self.shared.clone(),
                })),
            );
            Ok(node)
        } else {
            Err(())
        }
    }

    fn mkdir(&mut self, parent_dir: &str, path: &str) -> Result<(), ()> {
        if let Ok(inode_num) = self.resolve_path(parent_dir) {
            if let Ok(_) = self.resolve_path(format!("{}/{}", parent_dir, path).as_str()) {
                return Err(());
            }
            let inode = self.read_inode(inode_num).unwrap();
            if inode.is_dir() {
                if let Err(_) = self.create_dir(inode_num, path) {
                    Err(())
                } else {
                    Ok(())
                }
            } else {
                Err(())
            }
        } else {
            Ok(())
        }
    }

    fn rmdir(&mut self, path: &str) -> Result<(), ()> {
        if let Err(_) = self.remove_entry(path) {
            Err(())
        } else {
            Ok(())
        }
    }

    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<(), ()> {
        self.rename_entry(old_path, new_path).map_err(|_| ())
    }

    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()> {
        if let Ok(inode) = self.resolve_path(path) {
            match self.list_dir(inode) {
                Ok(entries) => Ok(entries),
                Err(_) => Err(()),
            }
        } else {
            Err(())
        }
    }

    fn rm(&mut self, path: &str) -> Result<(), ()> {
        if let Err(_) = self.remove_entry(path) {
            Err(())
        } else {
            Ok(())
        }
    }

    fn touch(&mut self, parent_path: &str, filename: &str) -> Result<(), ()> {
        if let Ok(inode_num) = self.resolve_path(parent_path) {
            if let Ok(_) = self.resolve_path(format!("{}/{}", parent_path, filename).as_str()) {
                return Err(());
            }
            let inode = self.read_inode(inode_num).unwrap();
            if inode.is_dir() {
                self.create_file(inode_num, filename).unwrap();
                Ok(())
            } else {
                Err(())
            }
        } else {
            Err(())
        }
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        if let Ok(inode_num) = self.resolve_path(path) {
            let inode = self.read_inode(inode_num).unwrap();
            let name = path.split('/').last().unwrap();

            if inode.is_dir() {
                Ok(Metadata {
                    file_type: FileType::Dir,
                    size: inode.size as usize,
                    name: name.to_string(),
                    gid: inode.gid,
                    uid: inode.uid,
                    ino: inode_num,
                    access_time: to_u32_saturating(inode.access_time),
                    created_time: to_u32_saturating(inode.created_time),
                    modified_time: to_u32_saturating(inode.modified_time),
                })
            } else {
                Ok(Metadata {
                    file_type: FileType::File,
                    size: inode.size as usize,
                    name: name.to_string(),
                    ino: inode_num,
                    gid: inode.gid,
                    uid: inode.uid,
                    access_time: to_u32_saturating(inode.access_time),
                    created_time: to_u32_saturating(inode.created_time),
                    modified_time: to_u32_saturating(inode.modified_time),
                })
            }
        } else {
            Err(())
        }
    }

    fn set_attr(&mut self, path: &str, attr: u32, value: u32) -> Result<(), ()> {
        if attr != IFLAG_ENCRYPTED {
            return Err(());
        }

        let inode_num = if path == "/" {
            1
        } else {
            self.resolve_path(path).or_else(|_| Err(()))?
        };

        let mut inode = self.read_inode(inode_num).map_err(|_| ())?;
        if value != 0 {
            inode.flags |= inode::IFLAG_ENCRYPTED;
        } else {
            inode.flags &= !inode::IFLAG_ENCRYPTED;
        }
        self.write_inode(inode_num, &inode).map_err(|_| ())
    }

    fn get_attr(&mut self, path: &str, attr: u32) -> Result<u32, ()> {
        if attr != IFLAG_ENCRYPTED {
            return Err(());
        }

        let inode_num = if path == "/" {
            1
        } else {
            self.resolve_path(path).or_else(|_| Err(()))?
        };

        let inode = self.read_inode(inode_num).map_err(|_| ())?;
        Ok(if (inode.flags & inode::IFLAG_ENCRYPTED) != 0 {
            IFLAG_ENCRYPTED
        } else {
            0
        })
    }
}

pub struct TfsProxy;

impl FileSystem for TfsProxy {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.open(path)
    }

    fn mkdir(&mut self, parent_dir: &str, path: &str) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.mkdir(parent_dir, path)
    }

    fn rmdir(&mut self, path: &str) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.rmdir(path)
    }

    fn rename(&mut self, old_path: &str, new_path: &str) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.rename(old_path, new_path)
    }

    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.ls(path)
    }

    fn rm(&mut self, path: &str) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.rm(path)
    }

    fn touch(&mut self, parent_path: &str, filename: &str) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.touch(parent_path, filename)
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.metadata(path)
    }

    fn set_attr(&mut self, path: &str, attr: u32, value: u32) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.set_attr(path, attr, value)
    }

    fn get_attr(&mut self, path: &str, attr: u32) -> Result<u32, ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.get_attr(path, attr)
    }
}
