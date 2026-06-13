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
const DIR_INDEX_CACHE_CAPACITY: usize = 512;

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

    fn remove_prefix(&mut self, generation: usize, prefix: &str) {
        self.ensure_generation(generation);

        let is_match = |path: &str| {
            path == prefix
                || path
                    .strip_prefix(prefix)
                    .map(|suffix| suffix.starts_with('/'))
                    .unwrap_or(false)
        };

        self.map.retain(|path, _| !is_match(path.as_str()));
        self.order.retain(|path| !is_match(path.as_str()));
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

    fn invalidate_inode(&mut self, generation: usize, inode_no: u32) {
        self.ensure_generation(generation);
        if let Some(old) = self.map.remove(&inode_no) {
            self.total_bytes = self.total_bytes.saturating_sub(old.len());
        }
        self.order.retain(|ino| *ino != inode_no);
    }

    fn invalidate_all_entries(&mut self) {
        self.total_bytes = 0;
        self.map.clear();
        self.order.clear();
    }
}

#[derive(Clone, Default)]
struct DirIndexEntry {
    names: BTreeMap<String, u32>,
    next_free_slot_hint: usize,
}

#[derive(Default)]
struct DirIndexCache {
    generation: usize,
    map: BTreeMap<u32, DirIndexEntry>,
    order: VecDeque<u32>,
}

impl DirIndexCache {
    fn ensure_generation(&mut self, generation: usize) {
        if self.generation != generation {
            self.generation = generation;
            self.map.clear();
            self.order.clear();
        }
    }

    fn get(&mut self, generation: usize, parent_ino: u32) -> Option<DirIndexEntry> {
        self.ensure_generation(generation);
        self.map.get(&parent_ino).cloned()
    }

    fn lookup(&mut self, generation: usize, parent_ino: u32, name: &str) -> Option<u32> {
        self.ensure_generation(generation);
        self.map
            .get(&parent_ino)
            .and_then(|entry| entry.names.get(name).copied())
    }

    fn insert(&mut self, generation: usize, parent_ino: u32, entry: DirIndexEntry) {
        self.ensure_generation(generation);

        if !self.map.contains_key(&parent_ino) {
            self.order.push_back(parent_ino);
        }
        self.map.insert(parent_ino, entry);

        while self.order.len() > DIR_INDEX_CACHE_CAPACITY {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    fn update_after_insert(
        &mut self,
        generation: usize,
        parent_ino: u32,
        name: String,
        inode_no: u32,
        next_free_slot_hint: usize,
    ) {
        self.ensure_generation(generation);

        if let Some(entry) = self.map.get_mut(&parent_ino) {
            entry.names.insert(name, inode_no);
            entry.next_free_slot_hint = next_free_slot_hint;
        }
    }

    fn remove(&mut self, generation: usize, parent_ino: u32, name: &str) {
        self.ensure_generation(generation);

        if let Some(entry) = self.map.get_mut(&parent_ino) {
            entry.names.remove(name);
            entry.next_free_slot_hint = 0;
        }
    }

    fn rename(
        &mut self,
        generation: usize,
        old_parent_ino: u32,
        old_name: &str,
        new_parent_ino: u32,
        new_name: String,
        inode_no: u32,
    ) {
        self.remove(generation, old_parent_ino, old_name);
        self.update_after_insert(generation, new_parent_ino, new_name, inode_no, 0);
    }
}

pub(crate) struct TwilightFsShared {
    generation: AtomicUsize,
    lookup_cache: Mutex<PathLookupCache>,
    file_cache: Mutex<FileContentCache>,
    dir_index_cache: Mutex<DirIndexCache>,
}

impl TwilightFsShared {
    fn new() -> Self {
        Self {
            generation: AtomicUsize::new(0),
            lookup_cache: Mutex::new(PathLookupCache::default()),
            file_cache: Mutex::new(FileContentCache::default()),
            dir_index_cache: Mutex::new(DirIndexCache::default()),
        }
    }

    #[inline]
    pub(crate) fn generation(&self) -> usize {
        self.generation.load(Ordering::Acquire)
    }

    #[inline]
    pub(crate) fn invalidate_namespace(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.file_cache.lock().invalidate_all_entries();
    }

    #[inline]
    pub(crate) fn invalidate_file_inode(&self, inode_no: u32) {
        let generation = self.generation();
        self.file_cache
            .lock()
            .invalidate_inode(generation, inode_no);
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
    pub(crate) fn remove_lookup_prefix(&self, prefix: &str) {
        let generation = self.generation();
        self.lookup_cache.lock().remove_prefix(generation, prefix);
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

    #[inline]
    fn dir_index_get(&self, parent_ino: u32) -> Option<DirIndexEntry> {
        let generation = self.generation();
        self.dir_index_cache.lock().get(generation, parent_ino)
    }

    #[inline]
    fn dir_index_lookup(&self, parent_ino: u32, name: &str) -> Option<u32> {
        let generation = self.generation();
        self.dir_index_cache
            .lock()
            .lookup(generation, parent_ino, name)
    }

    #[inline]
    fn dir_index_set(&self, parent_ino: u32, entry: DirIndexEntry) {
        let generation = self.generation();
        self.dir_index_cache
            .lock()
            .insert(generation, parent_ino, entry);
    }

    #[inline]
    fn dir_index_update_after_insert(
        &self,
        parent_ino: u32,
        name: String,
        inode_no: u32,
        next_free_slot_hint: usize,
    ) {
        let generation = self.generation();
        self.dir_index_cache.lock().update_after_insert(
            generation,
            parent_ino,
            name,
            inode_no,
            next_free_slot_hint,
        );
    }

    #[inline]
    fn dir_index_remove(&self, parent_ino: u32, name: &str) {
        let generation = self.generation();
        self.dir_index_cache
            .lock()
            .remove(generation, parent_ino, name);
    }

    #[inline]
    fn dir_index_rename(
        &self,
        old_parent_ino: u32,
        old_name: &str,
        new_parent_ino: u32,
        new_name: String,
        inode_no: u32,
    ) {
        let generation = self.generation();
        self.dir_index_cache.lock().rename(
            generation,
            old_parent_ino,
            old_name,
            new_parent_ino,
            new_name,
            inode_no,
        );
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

        let mut current_inode = 1;

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
                entries[0] = PartitionEntry::new(0x00, TWILIGHT_PARTITION_TYPE, start_lba, sectors);
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
        self.shared.invalidate_namespace();
        Ok(())
    }

    pub fn allocate_zones(&mut self, count: usize) -> Result<Vec<u32>, TfsError> {
        let bits_per_block = self.superblock.block_size as usize * 8;
        let zmap_start = self.superblock.imap_blocks + 1;
        let max_data_zones = self
            .superblock
            .zones
            .saturating_sub(self.superblock.first_data_zone);

        let mut zones = Vec::with_capacity(count);
        let mut buf = [0u8; FS_BLOCK_SIZE];

        for i in 0..self.superblock.zmap_blocks {
            if zones.len() == count {
                break;
            }

            if read_tfs_block(self.device.lock().as_mut(), zmap_start + i, &mut buf).is_err() {
                return Err(TfsError::IoError);
            }

            let mut dirty = false;
            for byte_idx in 0..buf.len() {
                if zones.len() == count {
                    break;
                }
                if buf[byte_idx] != 0xFF {
                    for bit in 0..8 {
                        if zones.len() == count {
                            break;
                        }
                        if buf[byte_idx] & (1 << bit) == 0 {
                            let zone = i * bits_per_block as u32 + (byte_idx * 8 + bit) as u32;
                            if zone >= max_data_zones {
                                break;
                            }
                            buf[byte_idx] |= 1 << bit;
                            zones.push(zone + self.superblock.first_data_zone);
                            dirty = true;
                        }
                    }
                }
            }

            if dirty {
                if write_tfs_block(self.device.lock().as_mut(), zmap_start + i, &buf).is_err() {
                    return Err(TfsError::IoError);
                }
            }
        }

        if zones.len() < count {
            for zone in &zones {
                let _ = self.dealloc_zone(*zone);
            }
            return Err(TfsError::NoSpaceLeft);
        }

        Ok(zones)
    }

    pub fn allocate_zone(&mut self) -> Result<u32, TfsError> {
        let bits_per_block = self.superblock.block_size as usize * 8;
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

        let inode_index = inode as usize - 1;
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

        buffer[byte_index] &= !(1 << bit_in_byte);

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

    fn decode_dir_name(name: &[u8; 60]) -> Option<String> {
        let raw = core::str::from_utf8(name).ok()?.trim_end_matches('\0');
        if raw.is_empty() {
            None
        } else {
            Some(raw.to_string())
        }
    }

    fn build_dir_index_entry(&mut self, parent_inode: &Inode) -> Result<DirIndexEntry, FsError> {
        let dir_entry_size = size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;
        let total_slots = Inode::DIRECT_SLOT_COUNT * entries_per_block;

        let mut names = BTreeMap::new();
        let mut next_free_slot_hint = total_slots;
        let mut buf = [0u8; FS_BLOCK_SIZE];

        for slot in 0..total_slots {
            let block_idx = slot / entries_per_block;
            let entry_idx = slot % entries_per_block;
            let zone = parent_inode.direct_slot_get(block_idx);
            if zone == 0 {
                if next_free_slot_hint == total_slots {
                    next_free_slot_hint = slot;
                }
                continue;
            }

            if entry_idx == 0 {
                read_tfs_block(self.device.lock().as_mut(), zone, &mut buf)
                    .map_err(|_| InvalidInode)?;
            }

            let offset = entry_idx * dir_entry_size;
            let inode_no = u32::from_le_bytes([
                buf[offset],
                buf[offset + 1],
                buf[offset + 2],
                buf[offset + 3],
            ]);
            if inode_no == 0 {
                if next_free_slot_hint == total_slots {
                    next_free_slot_hint = slot;
                }
                continue;
            }

            let mut name = [0u8; 60];
            name.copy_from_slice(&buf[offset + 4..offset + 64]);
            if let Some(decoded) = Self::decode_dir_name(&name) {
                names.insert(decoded, inode_no);
            }
        }

        Ok(DirIndexEntry {
            names,
            next_free_slot_hint,
        })
    }

    fn ensure_dir_index_entry(
        &mut self,
        parent_inode_num: u32,
        parent_inode: &Inode,
    ) -> Result<DirIndexEntry, FsError> {
        if let Some(entry) = self.shared.dir_index_get(parent_inode_num) {
            return Ok(entry);
        }

        let entry = self.build_dir_index_entry(parent_inode)?;
        self.shared.dir_index_set(parent_inode_num, entry.clone());
        Ok(entry)
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
        let total_slots = Inode::DIRECT_SLOT_COUNT * entries_per_block;
        let start_hint = self
            .shared
            .dir_index_get(parent_inode_num)
            .map(|entry| core::cmp::min(entry.next_free_slot_hint, total_slots))
            .unwrap_or(0);
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
        let entry_bytes =
            unsafe { core::slice::from_raw_parts(&entry as *const _ as *const u8, dir_entry_size) };

        let mut try_insert = |fs: &mut TwilightFs,
                              start: usize,
                              end: usize|
         -> Result<Option<usize>, &'static str> {
            let mut buf = [0u8; FS_BLOCK_SIZE];
            let mut loaded_block_idx: Option<usize> = None;
            let mut loaded_block_zone = 0u32;

            for slot in start..end {
                let block_idx = slot / entries_per_block;
                let entry_idx = slot % entries_per_block;
                let mut block = parent_inode.direct_slot_get(block_idx);

                if block == 0 {
                    block = fs
                        .allocate_zone()
                        .map_err(|_| "Failed to allocate directory zone")?;
                    parent_inode.direct_slot_set(block_idx, block);

                    let zero = [0u8; FS_BLOCK_SIZE];
                    if write_tfs_block(fs.device.lock().as_mut(), block, &zero).is_err() {
                        return Err("Failed to initialize directory block");
                    }

                    fs.write_inode(parent_inode_num, &parent_inode)?;
                }

                if loaded_block_idx != Some(block_idx) || loaded_block_zone != block {
                    if read_tfs_block(fs.device.lock().as_mut(), block, &mut buf).is_err() {
                        return Err("Failed to read block");
                    }
                    loaded_block_idx = Some(block_idx);
                    loaded_block_zone = block;
                }

                if block_idx == 0 && parent_inode.size == 0 {
                    buf.fill(0);
                }

                let offset = entry_idx * dir_entry_size;
                let inode_field = u32::from_le_bytes([
                    buf[offset],
                    buf[offset + 1],
                    buf[offset + 2],
                    buf[offset + 3],
                ]);
                if inode_field == 0 {
                    buf[offset..offset + dir_entry_size].copy_from_slice(entry_bytes);
                    if write_tfs_block(fs.device.lock().as_mut(), block, &buf).is_err() {
                        return Err("Failed to write block");
                    }
                    parent_inode.size += dir_entry_size as u64;
                    fs.write_inode(parent_inode_num, &parent_inode)?;
                    return Ok(Some(slot));
                }
            }

            Ok(None)
        };

        let inserted_slot = if let Some(slot) = try_insert(self, start_hint, total_slots)? {
            Some(slot)
        } else if start_hint > 0 {
            try_insert(self, 0, start_hint)?
        } else {
            None
        };

        if let Some(slot) = inserted_slot {
            let next_hint = core::cmp::min(slot + 1, total_slots);
            self.shared.dir_index_update_after_insert(
                parent_inode_num,
                name.to_string(),
                child_inode_num,
                next_hint,
            );
            Ok(())
        } else {
            Err("Directory is full")
        }
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
        self.create_file_with_mode(parent_inode_num, name, 0o777)
    }

    fn create_file_with_mode(
        &mut self,
        parent_inode_num: u32,
        name: &str,
        mode: u16,
    ) -> Result<u32, FsError> {
        if name.len() > 60 {
            return Err(FileNotFound);
        }

        let parent_inode = self
            .read_inode(parent_inode_num)
            .map_err(|_| InvalidInode)?;
        let dir_index = self.ensure_dir_index_entry(parent_inode_num, &parent_inode)?;
        if dir_index.names.contains_key(name) {
            return Err(FileAlreadyExists);
        }

        let new_inode_num = self.allocate_inode().map_err(|_| InvalidInode)? + 1;
        let new_zone = self.allocate_zone().map_err(|_| InvalidInode)?;

        let time = CMOS::new().unix_time();

        let mut inode = Inode::new_file(time, mode);
        // Inherit encryption flag from parent directory
        if (parent_inode.flags & inode::IFLAG_ENCRYPTED) != 0 {
            inode.flags |= inode::IFLAG_ENCRYPTED;
        }
        inode.direct_slot_set(0, new_zone);

        self.write_inode(new_inode_num, &inode)
            .map_err(|_| InvalidInode)?;

        self.create_dir_entry(parent_inode_num, name, new_inode_num)
            .map_err(|_| InvalidInode)?;

        Ok(new_inode_num)
    }

    pub fn write_file(&mut self, inode_num: u32, data: &[u8]) -> Result<(), FsError> {
        if inode_num == 0 || inode_num as usize > self.superblock.ninodes as usize {
            return Err(InvalidInode);
        }

        let mut inode = self.read_inode(inode_num).map_err(|_| InvalidInode)?;
        let block_size = self.superblock.block_size as usize;
        let zone_entries = block_size / 4;
        let required_blocks = if data.is_empty() {
            0
        } else {
            (data.len() + block_size - 1) / block_size
        };
        let max_blocks =
            Inode::DIRECT_SLOT_COUNT + zone_entries + (zone_entries.saturating_mul(zone_entries));
        if required_blocks > max_blocks {
            return Err(FsError::FileSizeTooLarge);
        }

        let direct_capacity = Inode::DIRECT_SLOT_COUNT;
        let single_capacity = zone_entries;
        let existing_blocks = if inode.size == 0 {
            0
        } else {
            ((inode.size as usize) + block_size - 1) / block_size
        };
        let needs_indirect = required_blocks > direct_capacity;
        let is_tiny_direct_only = required_blocks > 0 && !needs_indirect;

        let missing_data_est = required_blocks.saturating_sub(existing_blocks);
        let mut metadata_est = 0usize;
        if needs_indirect && inode.single_indirect_get() == 0 {
            metadata_est += 1;
        }
        if required_blocks > direct_capacity + single_capacity {
            if inode.double_indirect_get() == 0 {
                metadata_est += 1;
            }
            let needed_double_data_blocks = required_blocks - direct_capacity - single_capacity;
            let existing_double_data_blocks =
                existing_blocks.saturating_sub(direct_capacity + single_capacity);
            let needed_l1_blocks = (needed_double_data_blocks + zone_entries - 1) / zone_entries;
            let existing_l1_blocks =
                (existing_double_data_blocks + zone_entries - 1) / zone_entries;
            metadata_est += needed_l1_blocks.saturating_sub(existing_l1_blocks);
        }

        let prealloc_count = if is_tiny_direct_only {
            0
        } else {
            missing_data_est + metadata_est
        };
        let mut preallocated = if prealloc_count > 0 {
            self.allocate_zones(prealloc_count).unwrap_or(Vec::new())
        } else {
            Vec::new()
        };
        // allocate_zones returns ascending zones. Reverse once so pop() preserves ascending order.
        preallocated.reverse();

        let mut get_new_zone = |fs: &mut TwilightFs| -> Result<u32, FsError> {
            if let Some(zone) = preallocated.pop() {
                Ok(zone)
            } else {
                fs.allocate_zone().map_err(|_| InvalidInode)
            }
        };

        struct WriteOp {
            zone: u32,
            data_offset: usize,
            len: usize,
        }

        let mut bytes_written = 0;
        let mut remaining = data.len();
        let mut write_ops: Vec<WriteOp> = Vec::with_capacity(required_blocks + 1);

        let mut direct_zones = [0u32; Inode::DIRECT_SLOT_COUNT];
        for (i, slot) in direct_zones.iter_mut().enumerate() {
            *slot = inode.direct_slot_get(i);
        }

        for i in 0..direct_zones.len() {
            if remaining == 0 {
                break;
            }

            if direct_zones[i] == 0 {
                let zone = get_new_zone(self)?;
                direct_zones[i] = zone;
            }

            let copy_size = core::cmp::min(block_size, remaining);
            write_ops.push(WriteOp {
                zone: direct_zones[i],
                data_offset: bytes_written,
                len: copy_size,
            });

            bytes_written += copy_size;
            remaining -= copy_size;
        }

        let mut single_indirect_block = [0u8; FS_BLOCK_SIZE];
        let mut single_indirect_loaded = false;
        let mut single_indirect_dirty = false;

        // if space in direct zones is filled, use indirect nodes
        if remaining > 0 {
            if inode.single_indirect_get() == 0 {
                let zone = get_new_zone(self)?;
                inode.single_indirect_set(zone);
                single_indirect_loaded = true;
                single_indirect_dirty = true;
            }

            if !single_indirect_loaded {
                read_tfs_block(
                    self.device.lock().as_mut(),
                    inode.single_indirect_get(),
                    &mut single_indirect_block,
                )?;
                single_indirect_loaded = true;
            }

            for i in 0..zone_entries {
                if remaining == 0 {
                    break;
                }

                let entry = u32::from_le_bytes([
                    single_indirect_block[i * 4],
                    single_indirect_block[i * 4 + 1],
                    single_indirect_block[i * 4 + 2],
                    single_indirect_block[i * 4 + 3],
                ]);

                let zone = if entry == 0 {
                    let new_zone = get_new_zone(self)?;
                    single_indirect_block[i * 4..i * 4 + 4]
                        .copy_from_slice(&new_zone.to_le_bytes());
                    single_indirect_dirty = true;
                    new_zone
                } else {
                    entry
                };

                let copy_size = core::cmp::min(block_size, remaining);
                write_ops.push(WriteOp {
                    zone,
                    data_offset: bytes_written,
                    len: copy_size,
                });

                bytes_written += copy_size;
                remaining -= copy_size;
            }
        }

        let mut double_indirect_block = [0u8; FS_BLOCK_SIZE];
        let mut double_indirect_loaded = false;
        let mut double_indirect_dirty = false;
        let mut dirty_l2_blocks: Vec<(u32, [u8; FS_BLOCK_SIZE])> = Vec::new();

        if remaining > 0 {
            if inode.double_indirect_get() == 0 {
                inode.double_indirect_set(get_new_zone(self)?);
                double_indirect_loaded = true;
                double_indirect_dirty = true;
            }

            if !double_indirect_loaded {
                read_tfs_block(
                    self.device.lock().as_mut(),
                    inode.double_indirect_get(),
                    &mut double_indirect_block,
                )?;
                double_indirect_loaded = true;
            }

            for i in 0..zone_entries {
                if remaining == 0 {
                    break;
                }

                let mut indirect_was_new = false;
                let indirect_zone = {
                    let entry = u32::from_le_bytes([
                        double_indirect_block[i * 4],
                        double_indirect_block[i * 4 + 1],
                        double_indirect_block[i * 4 + 2],
                        double_indirect_block[i * 4 + 3],
                    ]);
                    if entry == 0 {
                        let new_zone = get_new_zone(self)?;
                        double_indirect_block[i * 4..i * 4 + 4]
                            .copy_from_slice(&new_zone.to_le_bytes());
                        double_indirect_dirty = true;
                        indirect_was_new = true;
                        new_zone
                    } else {
                        entry
                    }
                };

                let mut indirect_block = [0u8; FS_BLOCK_SIZE];
                let mut indirect_dirty = false;
                if !indirect_was_new {
                    read_tfs_block(
                        self.device.lock().as_mut(),
                        indirect_zone,
                        &mut indirect_block,
                    )?;
                }

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
                            let new_zone = get_new_zone(self)?;
                            indirect_block[j * 4..j * 4 + 4]
                                .copy_from_slice(&new_zone.to_le_bytes());
                            indirect_dirty = true;
                            new_zone
                        } else {
                            entry
                        }
                    };

                    let copy_size = core::cmp::min(block_size, remaining);
                    write_ops.push(WriteOp {
                        zone,
                        data_offset: bytes_written,
                        len: copy_size,
                    });

                    bytes_written += copy_size;
                    remaining -= copy_size;
                }

                if indirect_dirty {
                    dirty_l2_blocks.push((indirect_zone, indirect_block));
                }
            }
        }

        if remaining > 0 {
            return Err(FsError::FileSizeTooLarge);
        }

        let mut i = 0usize;
        while i < write_ops.len() {
            let op = &write_ops[i];
            if op.len == block_size {
                let mut j = i + 1;
                // the next like is written badly thats why this comment (17 Feb, 2026)
                // If the next write_ops zmap zone is continous to this one then write them together. This will minimize I/O blocks and Up the filesystem speed.
                while j < write_ops.len()
                    && write_ops[j - 1].len == block_size
                    && write_ops[j].len == block_size
                    && write_ops[j].zone == write_ops[j - 1].zone + 1
                    && write_ops[j].data_offset == write_ops[j - 1].data_offset + block_size
                {
                    j += 1;
                }

                let data_start = write_ops[i].data_offset;
                let data_len = (j - i) * block_size;
                write_tfs_blocks(
                    self.device.lock().as_mut(),
                    write_ops[i].zone,
                    &data[data_start..data_start + data_len],
                )?;
                i = j;
            } else {
                let mut block = [0u8; FS_BLOCK_SIZE];
                block[..op.len].copy_from_slice(&data[op.data_offset..op.data_offset + op.len]);
                write_tfs_block(self.device.lock().as_mut(), op.zone, &block)?;
                i += 1;
            }
        }

        for (zone, block) in dirty_l2_blocks.iter() {
            write_tfs_block(self.device.lock().as_mut(), *zone, block)?;
        }
        if single_indirect_loaded && single_indirect_dirty {
            write_tfs_block(
                self.device.lock().as_mut(),
                inode.single_indirect_get(),
                &single_indirect_block,
            )?;
        }
        if double_indirect_loaded && double_indirect_dirty {
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
        self.write_inode(inode_num, &inode)
            .map_err(|_| InvalidInode)?;

        for zone in preallocated {
            let _ = self.dealloc_zone(zone);
        }

        self.shared.invalidate_file_inode(inode_num);
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
                                mode: inode.mode,
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
        self.create_dir_with_mode(parent_inode_num, name, 0o777)
    }

    fn create_dir_with_mode(
        &mut self,
        parent_inode_num: u32,
        name: &str,
        mode: u16,
    ) -> Result<u32, FsError> {
        if name.len() > 60 {
            return Err(FileNameTooLong);
        }

        let parent_inode = self
            .read_inode(parent_inode_num)
            .map_err(|_| InvalidInode)?;
        let dir_index = self.ensure_dir_index_entry(parent_inode_num, &parent_inode)?;
        if dir_index.names.contains_key(name) {
            return Err(FileAlreadyExists);
        }

        let new_inode_num = self.allocate_inode().map_err(|_| InvalidInode)? + 1;
        let new_zone = self.allocate_zone().map_err(|_| InvalidInode)?;

        let time = CMOS::new().unix_time();

        let mut inode = Inode::new_dir(time, mode);
        if (parent_inode.flags & inode::IFLAG_ENCRYPTED) != 0 {
            inode.flags |= inode::IFLAG_ENCRYPTED;
        }
        inode.direct_slot_set(0, new_zone);
        self.write_inode(new_inode_num, &inode)
            .map_err(|_| InvalidInode)?;

        self.create_dir_entry(parent_inode_num, name, new_inode_num)
            .map_err(|_| InvalidInode)?;

        self.create_dir_entry(new_inode_num, ".", new_inode_num)
            .map_err(|_| InvalidInode)?;
        self.create_dir_entry(new_inode_num, "..", parent_inode_num)
            .map_err(|_| InvalidInode)?;

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

        let _ = self
            .ensure_dir_index_entry(parent_inode_num, &parent_inode)
            .map_err(|_| "Failed to build directory index")?;
        Ok(self.shared.dir_index_lookup(parent_inode_num, name))
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

    fn join_parent_and_name(parent: &str, name: &str) -> String {
        if parent == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent, name)
        }
    }

    fn find_dir_entry_slot(
        &mut self,
        parent_inode_num: u32,
        name: &str,
    ) -> Result<Option<(u32, usize, u32)>, FsError> {
        let parent_inode = self
            .read_inode(parent_inode_num)
            .map_err(|_| InvalidInode)?;
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
        let mut parent_inode = self
            .read_inode(parent_inode_num)
            .map_err(|_| InvalidInode)?;
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

        let old_parent_inode = self
            .read_inode(old_parent_inode_num)
            .map_err(|_| InvalidInode)?;
        if !old_parent_inode.is_dir() {
            return Err(FsError::InvalidPath);
        }
        let new_parent_inode = self
            .read_inode(new_parent_inode_num)
            .map_err(|_| InvalidInode)?;
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
                let existing_inode = self
                    .read_inode(existing_inode_num)
                    .map_err(|_| InvalidInode)?;
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

        let old_full_path = Self::join_parent_and_name(old_parent_path.as_str(), old_name.as_str());
        let new_full_path = Self::join_parent_and_name(new_parent_path.as_str(), new_name.as_str());

        if old_parent_inode_num == new_parent_inode_num {
            self.shared.dir_index_rename(
                old_parent_inode_num,
                old_name.as_str(),
                new_parent_inode_num,
                new_name,
                old_inode_num,
            );
        } else {
            self.shared
                .dir_index_remove(old_parent_inode_num, old_name.as_str());
        }
        self.shared.remove_lookup_prefix(old_full_path.as_str());
        self.shared.remove_lookup_prefix(new_full_path.as_str());
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
            1
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

                    for di in 0..Inode::DIRECT_SLOT_COUNT {
                        let z = inode.direct_slot_get(di);
                        if z != 0 {
                            self.free_zone(z).unwrap();
                        }
                    }

                    self.dealloc_inode(inode_num).unwrap();

                    buf[offset..offset + dir_entry_size].fill(0);
                    write_tfs_block(self.device.lock().as_mut(), zone, &buf)?;

                    if parent_inode.size >= dir_entry_size as u64 {
                        parent_inode.size -= dir_entry_size as u64;
                    }
                    self.write_inode(parent_inode_num, &parent_inode).unwrap();

                    let target_path = Self::join_parent_and_name(parent_path.as_str(), target_name);
                    self.shared.dir_index_remove(parent_inode_num, target_name);
                    self.shared.remove_lookup_prefix(target_path.as_str());
                    self.shared.invalidate_file_inode(inode_num);
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

    fn alloc_zones(&mut self, count: usize) -> Result<Vec<u32>, TfsError> {
        self.allocate_zones(count)
    }

    fn write_blocks(&mut self, start_lba: u32, buf: &[u8]) -> Result<(), ()> {
        if buf.len() % self.block_size() != 0 {
            return Err(());
        }
        write_tfs_blocks(self.device.lock().as_mut(), start_lba, buf).map_err(|_| ())
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
                    mode: inode.mode,
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

    fn mkdir(&mut self, parent_dir: &str, path: &str, mode: u16) -> Result<(), ()> {
        if let Ok(inode_num) = self.resolve_path(parent_dir) {
            let inode = self.read_inode(inode_num).unwrap();
            if inode.is_dir() {
                if let Err(_) = self.create_dir_with_mode(inode_num, path, mode) {
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

    fn touch(&mut self, parent_path: &str, filename: &str, mode: u16) -> Result<(), ()> {
        if let Ok(inode_num) = self.resolve_path(parent_path) {
            let inode = self.read_inode(inode_num).unwrap();
            if inode.is_dir() {
                self.create_file_with_mode(inode_num, filename, mode)
                    .map(|_| ())
                    .map_err(|_| ())
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
                    mode: inode.mode,
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
                    mode: inode.mode,
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

    fn chmod(&mut self, path: &str, mode: u16) -> Result<(), crate::sys::fs::vfs::VfsError> {
        const CHMOD_MASK: u16 = 0o7777;
        use crate::sys::fs::vfs::VfsError;

        let inode_num = if path == "/" {
            1
        } else {
            self.resolve_path(path).map_err(|_| VfsError::NotFound)?
        };
        let mut inode = self.read_inode(inode_num).map_err(|_| VfsError::Io)?;
        inode.mode = (inode.mode & inode::MODE_TYPE_MASK) | (mode & CHMOD_MASK);
        inode.change_time = CMOS::new().unix_time();
        self.write_inode(inode_num, &inode)
            .map_err(|_| VfsError::Io)
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

    fn fs_type_name(&self) -> &'static str {
        "twilightfs"
    }

    fn source_name(&self) -> &'static str {
        "/dev/disk0"
    }

    fn stats(&mut self) -> Result<crate::sys::fs::vfs::FsStats, ()> {
        let block_size = self.superblock.block_size as usize;
        let bits_per_block = block_size * 8;
        let data_zones = self
            .superblock
            .zones
            .saturating_sub(self.superblock.first_data_zone) as usize;
        let total_inodes = self.superblock.ninodes as usize;
        let mut free_zones = 0u64;
        let mut free_inodes = 0u64;
        let mut buf = [0u8; FS_BLOCK_SIZE];

        let zmap_start = self.superblock.imap_blocks + 1;
        for block_idx in 0..self.superblock.zmap_blocks {
            read_tfs_block(
                self.device.lock().as_mut(),
                zmap_start + block_idx,
                &mut buf,
            )
            .map_err(|_| ())?;
            let first_bit = block_idx as usize * bits_per_block;
            let valid_bits = data_zones.saturating_sub(first_bit).min(bits_per_block);
            for bit in 0..valid_bits {
                if buf[bit / 8] & (1 << (bit % 8)) == 0 {
                    free_zones += 1;
                }
            }
        }

        for block_idx in 0..self.superblock.imap_blocks {
            read_tfs_block(self.device.lock().as_mut(), 1 + block_idx, &mut buf).map_err(|_| ())?;
            let first_bit = block_idx as usize * bits_per_block;
            let valid_bits = total_inodes.saturating_sub(first_bit).min(bits_per_block);
            for bit in 0..valid_bits {
                if buf[bit / 8] & (1 << (bit % 8)) == 0 {
                    free_inodes += 1;
                }
            }
        }

        Ok(crate::sys::fs::vfs::FsStats {
            fs_type: u32::from_le_bytes(*b"TFS0") as u64,
            block_size: block_size as u64,
            blocks: self.superblock.zones as u64,
            blocks_free: free_zones,
            blocks_available: free_zones,
            files: self.superblock.ninodes as u64,
            files_free: free_inodes,
            name_length: 255,
            fragment_size: block_size as u64,
            flags: 0,
        })
    }
}

pub struct TfsProxy;

impl FileSystem for TfsProxy {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.open(path)
    }

    fn mkdir(&mut self, parent_dir: &str, path: &str, mode: u16) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.mkdir(parent_dir, path, mode)
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

    fn touch(&mut self, parent_path: &str, filename: &str, mode: u16) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.touch(parent_path, filename, mode)
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.metadata(path)
    }

    fn chmod(&mut self, path: &str, mode: u16) -> Result<(), crate::sys::fs::vfs::VfsError> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.chmod(path, mode)
    }

    fn set_attr(&mut self, path: &str, attr: u32, value: u32) -> Result<(), ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.set_attr(path, attr, value)
    }

    fn get_attr(&mut self, path: &str, attr: u32) -> Result<u32, ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.get_attr(path, attr)
    }

    fn fs_type_name(&self) -> &'static str {
        "twilightfs"
    }

    fn source_name(&self) -> &'static str {
        "/dev/disk0"
    }

    fn stats(&mut self) -> Result<crate::sys::fs::vfs::FsStats, ()> {
        #[allow(static_mut_refs)]
        unsafe { MFS.get_unchecked().lock() }.stats()
    }
}
