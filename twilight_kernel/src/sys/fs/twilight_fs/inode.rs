use crate::sys;
use crate::sys::fs::twilight_fs::{
    TwilightFsShared, read_tfs_block, read_tfs_blocks, write_tfs_block,
};
use crate::sys::fs::vfs::{BlockDev, FsCtx, VfsNodeOps};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use twilight_common::syscall::types::{EIO, EISDIR};

pub const MODE_TYPE_MASK: u16 = 0xF000;
pub const MODE_PERM_MASK: u16 = 0x01FF; // rwxrwxrwx
pub const MODE_DIR: u16 = 0o040000;
pub const MODE_FILE: u16 = 0o100000;

pub type InodeFlags = u32;
pub const IFLAG_IMMUTABLE: InodeFlags = 1 << 0;
pub const IFLAG_APPEND: InodeFlags = 1 << 1;
pub const IFLAG_ENCRYPTED: InodeFlags = 1 << 2;
pub const IFLAG_INLINE_DATA: InodeFlags = 1 << 3;
pub const IFLAG_DIR_INDEXED: InodeFlags = 1 << 4;
pub const IFLAG_HAS_XATTR: InodeFlags = 1 << 5;

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File = 1,
    Directory = 2,
    Symlink = 3,
    BlockDevice = 4,
    CharacterDevice = 5,
    Socket = 6,
    Pipe = 7,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Extent32 {
    pub start_block: u32, // physical start block
    pub block_len: u32,   // length in blocks
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct XattrBlockHeader {
    pub magic: u32,
    pub used_bytes: u16,
    pub count: u16,
    pub checksum: u32,
}

pub const INODE_INLINE_BYTES: usize = 64;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Inode {
    pub mode: u16,
    pub nlinks: u16,
    pub uid: u32,
    pub gid: u32,

    pub size: u64,
    pub access_time: u64,
    pub modified_time: u64,
    pub change_time: u64,
    pub created_time: u64,

    pub flags: InodeFlags,
    pub generation: u64,

    pub xattr_block: u32,
    pub _pad0: u32,

    pub direct: [Extent32; 6],
    pub indirect: u32,
    pub double_indirect: u32,
    pub triple_indirect: u32,

    pub inline_data: [u8; INODE_INLINE_BYTES],

    pub inode_checksum: u32,
    pub _pad1: u32,
}

impl Inode {
    pub const DIRECT_SLOT_COUNT: usize = 6;

    fn base(mode: u16, now: u64) -> Self {
        let user_id = sys::proc::user::get_uid();
        let g_id = sys::proc::user::get_gid();
        Self {
            mode,
            nlinks: 1,
            uid: user_id as u32,
            gid: g_id as u32,
            size: 0,
            access_time: now,
            modified_time: now,
            change_time: now,
            created_time: now,
            flags: 0,
            generation: 0,
            xattr_block: 0,
            _pad0: 0,
            direct: [Extent32 {
                start_block: 0,
                block_len: 0,
            }; Self::DIRECT_SLOT_COUNT],
            indirect: 0,
            double_indirect: 0,
            triple_indirect: 0,
            inline_data: [0; INODE_INLINE_BYTES],
            inode_checksum: 0,
            _pad1: 0,
        }
    }

    pub fn new_file(now: u64, perms: u16) -> Self {
        Self::base(MODE_FILE | (perms & MODE_PERM_MASK), now)
    }

    pub fn new_dir(now: u64, perms: u16) -> Self {
        let mut inode = Self::base(MODE_DIR | (perms & MODE_PERM_MASK), now);
        inode.nlinks = 2;
        inode
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        (self.mode & MODE_TYPE_MASK) == MODE_DIR
    }

    #[inline]
    pub fn is_file(&self) -> bool {
        (self.mode & MODE_TYPE_MASK) == MODE_FILE
    }

    #[inline]
    pub fn direct_slot_get(&self, index: usize) -> u32 {
        if index >= Self::DIRECT_SLOT_COUNT {
            return 0;
        }

        let extent = self.direct[index];
        if extent.block_len == 0 {
            0
        } else {
            extent.start_block
        }
    }

    #[inline]
    pub fn direct_slot_set(&mut self, index: usize, zone: u32) {
        if index >= Self::DIRECT_SLOT_COUNT {
            return;
        }

        self.direct[index] = if zone == 0 {
            Extent32 {
                start_block: 0,
                block_len: 0,
            }
        } else {
            Extent32 {
                start_block: zone,
                block_len: 1,
            }
        };
    }

    #[inline]
    pub fn single_indirect_get(&self) -> u32 {
        self.indirect
    }

    #[inline]
    pub fn single_indirect_set(&mut self, zone: u32) {
        self.indirect = zone;
    }

    #[inline]
    pub fn double_indirect_get(&self) -> u32 {
        self.double_indirect
    }

    #[inline]
    pub fn double_indirect_set(&mut self, zone: u32) {
        self.double_indirect = zone;
    }
}

pub(crate) struct TFSVfsNode {
    pub inode_no: u32,
    pub inode: Inode,
    pub full_path: String,
    pub ctx: Arc<Mutex<dyn FsCtx>>,
    pub shared: Arc<TwilightFsShared>,
}

unsafe impl Send for TFSVfsNode {}
unsafe impl Sync for TFSVfsNode {}

#[allow(dead_code)]
impl VfsNodeOps for TFSVfsNode {
    fn read(&self, device: &mut BlockDev, lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let block_size = 2048;
        let file_size = self.inode.size as usize;

        if lba >= file_size {
            return Ok(0);
        }

        let max_to_read = core::cmp::min(file_size - lba, buf.len());

        let should_cache = file_size > 0 && file_size <= super::FILE_CACHE_MAX_FILE_BYTES;

        if should_cache {
            if let Some(n) = self.shared.read_cached_file_slice(self.inode_no, lba, buf) {
                return Ok(core::cmp::min(n, max_to_read));
            }

            if let Ok(data) = self.read_all_file(device) {
                let n = core::cmp::min(max_to_read, data.len().saturating_sub(lba));
                if n > 0 {
                    buf[..n].copy_from_slice(&data[lba..lba + n]);
                }
                self.shared.insert_file_cache(self.inode_no, data);
                return Ok(n);
            }
        }

        let mut remaining = max_to_read;
        let mut written = 0;
        let mut buffer = [0u8; 2048];

        // Cache indirect metadata blocks during this read to avoid re-reading pointers.
        let mut single_indirect_cache = [0u8; 2048];
        let mut single_indirect_loaded = false;
        let mut double_root_cache = [0u8; 2048];
        let mut double_root_loaded = false;
        let mut double_l1_cache = [0u8; 2048];
        let mut double_l1_loaded_idx: Option<usize> = None;
        let mut double_l1_loaded_zone = 0u32;

        let direct_blocks = Inode::DIRECT_SLOT_COUNT;
        let indirect_entries = block_size / 4;
        let double_start = direct_blocks + indirect_entries;
        let double_entries = indirect_entries * indirect_entries;

        let mut logic_block = lba / block_size;
        let mut current_offset_in_block = lba % block_size;
        let mut dev = device.lock();

        struct ReadOp {
            zone: u32,
            out_off: usize,
            block_off: usize,
            len: usize,
        }
        let mut read_ops: Vec<ReadOp> = Vec::with_capacity((max_to_read + block_size - 1) / block_size);

        while remaining > 0 {
            let zone = if logic_block < direct_blocks {
                self.inode.direct_slot_get(logic_block)
            } else if logic_block < double_start {
                if self.inode.single_indirect_get() == 0 {
                    0
                } else {
                    if !single_indirect_loaded {
                        read_tfs_block(
                            dev.as_mut(),
                            self.inode.single_indirect_get(),
                            &mut single_indirect_cache,
                        )
                        .map_err(|_| ())?;
                        single_indirect_loaded = true;
                    }
                    let idx = logic_block - direct_blocks;
                    u32::from_le_bytes(
                        single_indirect_cache[idx * 4..(idx + 1) * 4]
                            .try_into()
                            .unwrap(),
                    )
                }
            } else if logic_block < double_start + double_entries {
                if self.inode.double_indirect_get() == 0 {
                    0
                } else {
                    if !double_root_loaded {
                        read_tfs_block(
                            dev.as_mut(),
                            self.inode.double_indirect_get(),
                            &mut double_root_cache,
                        )
                        .map_err(|_| ())?;
                        double_root_loaded = true;
                    }

                    let rel = logic_block - double_start;
                    let l1_idx = rel / indirect_entries;
                    let l2_idx = rel % indirect_entries;
                    let l1_zone = u32::from_le_bytes(
                        double_root_cache[l1_idx * 4..(l1_idx + 1) * 4]
                            .try_into()
                            .unwrap(),
                    );
                    if l1_zone == 0 {
                        0
                    } else {
                        if double_l1_loaded_idx != Some(l1_idx) || double_l1_loaded_zone != l1_zone
                        {
                            read_tfs_block(dev.as_mut(), l1_zone, &mut double_l1_cache)
                                .map_err(|_| ())?;
                            double_l1_loaded_idx = Some(l1_idx);
                            double_l1_loaded_zone = l1_zone;
                        }
                        u32::from_le_bytes(
                            double_l1_cache[l2_idx * 4..(l2_idx + 1) * 4]
                                .try_into()
                                .unwrap(),
                        )
                    }
                }
            } else {
                0
            };
            if zone == 0 {
                break;
            }

            let available = block_size - current_offset_in_block;
            let to_copy = core::cmp::min(remaining, available);

            read_ops.push(ReadOp {
                zone,
                out_off: written,
                block_off: current_offset_in_block,
                len: to_copy,
            });

            written += to_copy;
            remaining -= to_copy;
            logic_block += 1;
            current_offset_in_block = 0;
        }

        let mut i = 0usize;
        while i < read_ops.len() {
            let op = &read_ops[i];
            if op.block_off == 0 && op.len == block_size {
                let mut j = i + 1;
                // Same way how write_ops are batches for continuous
                while j < read_ops.len()
                    && read_ops[j].block_off == 0
                    && read_ops[j].len == block_size
                    && read_ops[j].zone == read_ops[j - 1].zone + 1
                    && read_ops[j].out_off == read_ops[j - 1].out_off + block_size
                {
                    j += 1;
                }

                let run_blocks = j - i;
                let run_bytes = run_blocks * block_size;
                let start_zone = read_ops[i].zone;
                let out_start = read_ops[i].out_off;
                let out_slice = &mut buf[out_start..out_start + run_bytes];

                read_tfs_blocks(dev.as_mut(), start_zone, out_slice).map_err(|_| ())?;
                for b in 0..run_blocks {
                    let start = b * block_size;
                    let end = start + block_size;
                    self.apply_crypto((start_zone + b as u32) as u64, &mut out_slice[start..end]);
                }
                i = j;
            } else {
                if let Err(_) = read_tfs_block(dev.as_mut(), op.zone, &mut buffer) {
                    return Err(());
                }
                self.apply_crypto(op.zone as u64, &mut buffer);
                buf[op.out_off..op.out_off + op.len]
                    .copy_from_slice(&buffer[op.block_off..op.block_off + op.len]);
                i += 1;
            }
        }

        Ok(written)
    }

    fn write(&mut self, device: &mut BlockDev, lba: usize, data: &[u8]) -> Result<(), ()> {
        const BLOCK_SIZE: usize = 2048;

        let mut bytes_written: usize = 0;
        let mut remaining: usize = data.len();
        let mut pos: usize = lba;

        let mut preallocated = if pos >= self.inode.size as usize {
            let blks = (remaining + BLOCK_SIZE - 1) / BLOCK_SIZE;
            let meta = blks / 512 + 3;
            self.ctx
                .lock()
                .alloc_zones(blks + meta)
                .unwrap_or(Vec::new())
        } else {
            Vec::new()
        };
        // Preserve ascending allocation order while using pop().
        preallocated.reverse();

        let mut get_new_zone = |ctx: &mut Arc<Mutex<dyn FsCtx>>| -> Result<u32, ()> {
            if let Some(z) = preallocated.pop() {
                Ok(z)
            } else {
                ctx.lock().alloc_zone().map_err(|_| ())
            }
        };

        struct WriteOp {
            zone: u32,
            data_offset: usize,
            len: usize,
        }
        let mut write_ops: Vec<WriteOp> = Vec::with_capacity((data.len() / BLOCK_SIZE) + 2);

        let mut direct_zones = [0u32; Inode::DIRECT_SLOT_COUNT];
        for (i, slot) in direct_zones.iter_mut().enumerate() {
            *slot = self.inode.direct_slot_get(i);
        }

        while remaining > 0 {
            let block_idx = pos / BLOCK_SIZE;
            if block_idx >= direct_zones.len() {
                break;
            }

            let offset_in_block = pos % BLOCK_SIZE;
            let max_copy = BLOCK_SIZE - offset_in_block;
            let copy_size = core::cmp::min(remaining, max_copy);

            if direct_zones[block_idx] == 0 {
                let zone = get_new_zone(&mut self.ctx)?;
                direct_zones[block_idx] = zone;
                if offset_in_block > 0 || copy_size < BLOCK_SIZE {
                    let zero = [0u8; BLOCK_SIZE];
                    write_tfs_block(device.lock().as_mut(), zone, &zero).map_err(|_| ())?;
                }
            }

            write_ops.push(WriteOp {
                zone: direct_zones[block_idx],
                data_offset: bytes_written,
                len: copy_size,
            });

            bytes_written += copy_size;
            remaining -= copy_size;
            pos += copy_size;
        }

        if remaining > 0 {
            let ind_cap = (BLOCK_SIZE / 4) - 1;
            let direct_blocks = direct_zones.len();

            if self.inode.single_indirect_get() == 0 {
                let zone = get_new_zone(&mut self.ctx)?;
                self.inode.single_indirect_set(zone);
                let zero_block = [0u8; BLOCK_SIZE];
                write_tfs_block(device.lock().as_mut(), zone, &zero_block).map_err(|_| ())?;
            }

            let mut indirect_block = [0u8; BLOCK_SIZE];
            read_tfs_block(
                device.lock().as_mut(),
                self.inode.single_indirect_get(),
                &mut indirect_block,
            )
            .map_err(|_| ())?;
            let mut indirect_dirty = false;

            while remaining > 0 {
                let logical_block = pos / BLOCK_SIZE;
                if logical_block < direct_blocks {
                    break;
                }
                let idx = logical_block - direct_blocks;
                if idx >= ind_cap {
                    break;
                }

                let offset_in_block = pos % BLOCK_SIZE;
                let max_copy = BLOCK_SIZE - offset_in_block;
                let copy_size = core::cmp::min(remaining, max_copy);

                let entry_off = idx * 4;
                let mut zone = u32::from_le_bytes(
                    indirect_block[entry_off..entry_off + 4].try_into().unwrap(),
                );

                if zone == 0 {
                    zone = get_new_zone(&mut self.ctx)?;
                    indirect_block[entry_off..entry_off + 4].copy_from_slice(&zone.to_le_bytes());
                    indirect_dirty = true;
                    if offset_in_block > 0 || copy_size < BLOCK_SIZE {
                        let zero = [0u8; BLOCK_SIZE];
                        write_tfs_block(device.lock().as_mut(), zone, &zero).map_err(|_| ())?;
                    }
                }

                write_ops.push(WriteOp {
                    zone,
                    data_offset: bytes_written,
                    len: copy_size,
                });

                bytes_written += copy_size;
                remaining -= copy_size;
                pos += copy_size;
            }

            if indirect_dirty {
                write_tfs_block(
                    device.lock().as_mut(),
                    self.inode.single_indirect_get(),
                    &indirect_block,
                )
                .map_err(|_| ())?;
            }
        }

        if remaining > 0 {
            let zone_entries = BLOCK_SIZE / 4;
            let ind_cap = zone_entries - 1;

            if self.inode.double_indirect_get() == 0 {
                self.inode.double_indirect_set(get_new_zone(&mut self.ctx)?);
                let zero_block = [0u8; BLOCK_SIZE];
                write_tfs_block(
                    device.lock().as_mut(),
                    self.inode.double_indirect_get(),
                    &zero_block,
                )
                .map_err(|_| ())?;
            }

            let mut double_root = [0u8; BLOCK_SIZE];
            read_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_get(),
                &mut double_root,
            )
            .map_err(|_| ())?;
            let mut double_root_dirty = false;

            let mut logical_block = pos / BLOCK_SIZE;

            let double_start_blk = direct_zones.len() + ind_cap;

            let mut rel_blk = logical_block - double_start_blk;

            let mut l1_idx = rel_blk / zone_entries;

            while remaining > 0 && l1_idx < ind_cap {
                let l1_entry_off = l1_idx * 4;
                let mut l1_zone = u32::from_le_bytes(
                    double_root[l1_entry_off..l1_entry_off + 4]
                        .try_into()
                        .unwrap(),
                );

                if l1_zone == 0 {
                    l1_zone = get_new_zone(&mut self.ctx)?;
                    double_root[l1_entry_off..l1_entry_off + 4]
                        .copy_from_slice(&l1_zone.to_le_bytes());
                    double_root_dirty = true;
                    let zero = [0u8; BLOCK_SIZE];
                    write_tfs_block(device.lock().as_mut(), l1_zone, &zero).map_err(|_| ())?;
                }

                let mut l2_block = [0u8; BLOCK_SIZE];
                read_tfs_block(device.lock().as_mut(), l1_zone, &mut l2_block).map_err(|_| ())?;
                let mut l2_dirty = false;

                let mut l2_idx = rel_blk % zone_entries;

                while remaining > 0 && l2_idx < ind_cap {
                    let offset_in_block = pos % BLOCK_SIZE;
                    let max_copy = BLOCK_SIZE - offset_in_block;
                    let copy_size = core::cmp::min(remaining, max_copy);

                    let l2_entry_off = l2_idx * 4;
                    let mut data_zone = u32::from_le_bytes(
                        l2_block[l2_entry_off..l2_entry_off + 4].try_into().unwrap(),
                    );

                    if data_zone == 0 {
                        data_zone = get_new_zone(&mut self.ctx)?;
                        l2_block[l2_entry_off..l2_entry_off + 4]
                            .copy_from_slice(&data_zone.to_le_bytes());
                        l2_dirty = true;
                        if offset_in_block > 0 || copy_size < BLOCK_SIZE {
                            let zero = [0u8; BLOCK_SIZE];
                            write_tfs_block(device.lock().as_mut(), data_zone, &zero)
                                .map_err(|_| ())?;
                        }
                    }

                    write_ops.push(WriteOp {
                        zone: data_zone,
                        data_offset: bytes_written,
                        len: copy_size,
                    });

                    bytes_written += copy_size;
                    remaining -= copy_size;
                    pos += copy_size;

                    logical_block += 1;
                    rel_blk += 1;
                    l2_idx += 1;
                }

                if l2_dirty {
                    write_tfs_block(device.lock().as_mut(), l1_zone, &l2_block).map_err(|_| ())?;
                }

                l1_idx += 1;
            }

            if double_root_dirty {
                write_tfs_block(
                    device.lock().as_mut(),
                    self.inode.double_indirect_get(),
                    &double_root,
                )
                .map_err(|_| ())?;
            }
        }

        if !self.is_encrypted() {
            let mut i = 0;
            while i < write_ops.len() {
                let start_op = &write_ops[i];
                let mut j = i + 1;
                while j < write_ops.len() {
                    let prev = &write_ops[j - 1];
                    let curr = &write_ops[j];

                    if curr.zone == prev.zone + 1
                        && prev.len == BLOCK_SIZE
                        && curr.len == BLOCK_SIZE
                        && prev.data_offset + prev.len == curr.data_offset
                    {
                        j += 1;
                    } else {
                        break;
                    }
                }

                let count = j - i;
                let total_len: usize = write_ops[i..j].iter().map(|op| op.len).sum();

                if count > 1 {
                    let start_zone = start_op.zone;
                    let data_start = start_op.data_offset;
                    self.ctx
                        .lock()
                        .write_blocks(start_zone, &data[data_start..data_start + total_len])
                        .map_err(|_| ())?;
                } else {
                    let op = &write_ops[i];
                    let block_offset = if i == 0 && (lba % BLOCK_SIZE != 0) {
                        lba % BLOCK_SIZE
                    } else {
                        0
                    };

                    if op.len == BLOCK_SIZE && block_offset == 0 {
                        write_tfs_block(
                            device.lock().as_mut(),
                            op.zone,
                            <&[u8; 2048]>::try_from(
                                &data[op.data_offset..op.data_offset + BLOCK_SIZE],
                            )
                            .unwrap(),
                        )
                        .map_err(|_| ())?;
                    } else {
                        let mut buffer = [0u8; BLOCK_SIZE];
                        if read_tfs_block(device.lock().as_mut(), op.zone, &mut buffer).is_err() {
                            return Err(());
                        }
                        buffer[block_offset..block_offset + op.len]
                            .copy_from_slice(&data[op.data_offset..op.data_offset + op.len]);
                        write_tfs_block(device.lock().as_mut(), op.zone, &buffer)
                            .map_err(|_| ())?;
                    }
                }

                i = j;
            }
        } else {
            for (idx, op) in write_ops.iter().enumerate() {
                let mut buffer = [0u8; BLOCK_SIZE];
                let block_offset = if idx == 0 && (lba % BLOCK_SIZE != 0) {
                    lba % BLOCK_SIZE
                } else {
                    0
                };

                if op.len < BLOCK_SIZE {
                    read_tfs_block(device.lock().as_mut(), op.zone, &mut buffer).map_err(|_| ())?;
                    self.apply_crypto(op.zone as u64, &mut buffer);
                }

                buffer[block_offset..block_offset + op.len]
                    .copy_from_slice(&data[op.data_offset..op.data_offset + op.len]);
                self.apply_crypto(op.zone as u64, &mut buffer);
                write_tfs_block(device.lock().as_mut(), op.zone, &buffer).map_err(|_| ())?;
            }
        }

        for z in preallocated {
            let _ = self.ctx.lock().free_zone(z);
        }

        let end_pos = bytes_written + lba;
        if end_pos > self.inode.size as usize {
            self.inode.size = end_pos as u64;
        }
        for (i, zone) in direct_zones.iter().copied().enumerate() {
            self.inode.direct_slot_set(i, zone);
        }
        self.ctx
            .lock()
            .write_inode_twilight(self.inode_no, self.inode)
            .unwrap();
        self.shared.invalidate_file_inode(self.inode_no);
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        if self.inode.is_dir() {
            return Ok(-EISDIR);
        }
        if self
            .ctx
            .lock()
            .remove_file(self.full_path.as_str())
            .is_err()
        {
            Ok(-1)
        } else {
            Ok(0)
        }
    }

    fn truncate(&mut self, device: &mut BlockDev, len: usize) -> Result<(), i32> {
        let cur = self.inode.size as usize;
        if len == cur {
            return Ok(());
        }

        if len < cur {
            self.inode.size = len as u64;
            self.ctx
                .lock()
                .write_inode_twilight(self.inode_no, self.inode)
                .map_err(|_| -(EIO as i32))?;
            self.shared.invalidate_file_inode(self.inode_no);
            return Ok(());
        }

        let mut remaining = len - cur;
        let mut offset = cur;
        let zero = [0u8; 2048];
        while remaining > 0 {
            let n = core::cmp::min(remaining, zero.len());
            self.write(device, offset, &zero[..n])
                .map_err(|_| -(EIO as i32))?;
            offset += n;
            remaining -= n;
        }

        Ok(())
    }
}

impl TFSVfsNode {
    fn is_encrypted(&self) -> bool {
        (self.inode.flags & IFLAG_ENCRYPTED) != 0
    }

    fn get_encryption_key(&self) -> Option<[u8; 32]> {
        if !self.is_encrypted() {
            return None;
        }

        let current_uid = crate::sys::proc::user::get_uid() as u32;
        if current_uid != 0 && current_uid != self.inode.uid {
            return None;
        }

        crate::sys::syscall::crypto::get_user_key(self.inode.uid)
    }

    fn apply_crypto(&self, block_idx: u64, buf: &mut [u8]) {
        use chacha20::ChaCha20;
        use chacha20::cipher::{KeyIvInit, StreamCipher};

        if let Some(key) = self.get_encryption_key() {
            let mut nonce = [0u8; 12];
            nonce[0..4].copy_from_slice(&self.inode_no.to_le_bytes());
            nonce[4..12].copy_from_slice(&block_idx.to_le_bytes());

            let key_arr = key.into();
            let nonce_arr = nonce.into();

            let mut cipher = ChaCha20::new(&key_arr, &nonce_arr);
            cipher.apply_keystream(buf);
        }
    }
}

impl TFSVfsNode {
    fn read_all_file(&self, device: &mut BlockDev) -> Result<Vec<u8>, ()> {
        let file_size = self.inode.size as usize;
        let mut out = vec![0u8; file_size];

        let block_size = 2048;
        let temp_buf_len = 128 * block_size;
        let mut temp_buf = vec![0u8; temp_buf_len];

        let mut zones = Vec::new();

        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let zone = self.inode.direct_slot_get(i);
            if zone == 0 {
                break;
            }
            zones.push(zone);
        }

        if self.inode.single_indirect_get() != 0 {
            let mut buf = [0u8; 2048];
            if read_tfs_block(
                device.lock().as_mut(),
                self.inode.single_indirect_get(),
                &mut buf,
            )
            .is_ok()
            {
                for i in 0..(block_size / 4) {
                    let z = u32::from_le_bytes(buf[i * 4..(i + 1) * 4].try_into().unwrap());
                    if z == 0 {
                        break;
                    }
                    zones.push(z);
                }
            }
        }

        if self.inode.double_indirect_get() != 0 {
            let mut buf1 = [0u8; 2048];
            if read_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_get(),
                &mut buf1,
            )
            .is_ok()
            {
                for i in 0..(block_size / 4) {
                    let z1 = u32::from_le_bytes(buf1[i * 4..(i + 1) * 4].try_into().unwrap());
                    if z1 == 0 {
                        break;
                    }

                    let mut buf2 = [0u8; 2048];
                    if read_tfs_block(device.lock().as_mut(), z1, &mut buf2).is_ok() {
                        for j in 0..(block_size / 4) {
                            let z2 =
                                u32::from_le_bytes(buf2[j * 4..(j + 1) * 4].try_into().unwrap());
                            if z2 == 0 {
                                break;
                            }
                            zones.push(z2);
                        }
                    }
                }
            }
        }

        let mut offset = 0;
        let mut z_i = 0;
        while z_i < zones.len() {
            let start_zone = zones[z_i];
            let mut count = 1;
            while z_i + count < zones.len() {
                if zones[z_i + count] == start_zone + count as u32 {
                    count += 1;
                } else {
                    break;
                }
            }

            let max_blocks = temp_buf_len / block_size;
            let run_len = core::cmp::min(count, max_blocks);

            let bytes_to_read = run_len * block_size;
            if let Err(_) = read_tfs_blocks(
                device.lock().as_mut(),
                start_zone,
                &mut temp_buf[..bytes_to_read],
            ) {
                return Err(());
            }
            for j in 0..run_len {
                let start = j * block_size;
                let end = start + block_size;
                self.apply_crypto(zones[z_i + j] as u64, &mut temp_buf[start..end]);
            }

            let bytes_to_copy = core::cmp::min(bytes_to_read, file_size - offset);
            out[offset..offset + bytes_to_copy].copy_from_slice(&temp_buf[..bytes_to_copy]);

            offset += bytes_to_copy;
            z_i += run_len;

            if offset >= file_size {
                break;
            }
        }

        Ok(out)
    }
}
