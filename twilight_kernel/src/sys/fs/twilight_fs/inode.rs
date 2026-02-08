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

pub const MODE_TYPE_MASK: u16 = 0xF000; // you can define your own layout
pub const MODE_PERM_MASK: u16 = 0x01FF; // rwxrwxrwx
pub const MODE_DIR: u16 = 0o040000;
pub const MODE_FILE: u16 = 0o100000;

pub type InodeFlags = u32;
pub const IFLAG_IMMUTABLE: InodeFlags   = 1 << 0;
pub const IFLAG_APPEND: InodeFlags      = 1 << 1;
pub const IFLAG_ENCRYPTED: InodeFlags   = 1 << 2;
pub const IFLAG_INLINE_DATA: InodeFlags = 1 << 3; // inline symlink/small file
pub const IFLAG_DIR_INDEXED: InodeFlags = 1 << 4; // has dir hash index block
pub const IFLAG_HAS_XATTR: InodeFlags   = 1 << 5;

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File            = 1,
    Directory       = 2,
    Symlink         = 3,
    BlockDevice     = 4,
    CharacterDevice = 5,
    Socket          = 6,
    Pipe            = 7,
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
    pub magic: u32,     // e.g. "XATR"
    pub used_bytes: u16,
    pub count: u16,
    pub checksum: u32,  // checksum over header+payload (excluding this field if you want)
    // followed by TLVs
    // [key_len:u16][val_len:u16][key_bytes][val_bytes]...
}

pub const INODE_INLINE_BYTES: usize = 64;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Inode {
    // permissions + type (POSIX-ish)
    pub mode: u16,     // includes type bits + permission bits
    pub nlinks: u16,
    pub uid: u32,
    pub gid: u32,

    // size & timestamps
    pub size: u64,
    pub access_time: u64,
    pub modified_time: u64,
    pub change_time: u64,
    pub created_time: u64, // creation time (nice to have)

    // inode metadata
    pub flags: InodeFlags,
    pub generation: u64, // bump when inode changes (useful for key derivation/consistency)

    // xattrs
    pub xattr_block: u32, // 0 = none
    pub _pad0: u32,

    // data mapping:
    // - if IFLAG_INLINE_DATA: `inline_data` holds payload (symlink target or small file)
    // - else: direct extents + indirect extent lists
    pub direct: [Extent32; 6],   // a few direct extents
    pub indirect: u32,           // block containing Extent32[]
    pub double_indirect: u32,    // block containing u32[] -> indirect blocks
    pub triple_indirect: u32,

    // inline payload area (used only when IFLAG_INLINE_DATA set)
    pub inline_data: [u8; INODE_INLINE_BYTES],

    // inode checksum (optional; if metadata csum feature enabled)
    pub inode_checksum: u32,
    pub _pad1: u32,
}

impl Inode {
    pub const DIRECT_SLOT_COUNT: usize = 6;

    fn base(mode: u16, now: u64) -> Self {
        Self {
            mode,
            nlinks: 1,
            uid: 0,
            gid: 0,
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
            return Ok(0); // nothing to read
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

        // We will iterate logically.
        let mut logic_block = lba / block_size;
        let mut current_offset_in_block = lba % block_size;

        while remaining > 0 {
            // Find zone for logic_block
            let zone = self.get_zone(device, logic_block)?;
            if zone == 0 {
                break;
            }

            // Only try to coalesce if we are starting at offset 0 of the block (full block read)
            // or if we are reading enough to cover the rest of this block.
            // But simplest is:

            if let Err(_) = read_tfs_block(device.lock().as_mut(), zone, &mut buffer) {
                return Err(());
            }

            let available = block_size - current_offset_in_block;
            let to_copy = core::cmp::min(remaining, available);

            buf[written..written + to_copy].copy_from_slice(
                &buffer[current_offset_in_block..current_offset_in_block + to_copy],
            );

            written += to_copy;
            remaining -= to_copy;
            logic_block += 1;
            current_offset_in_block = 0;
        }

        Ok(written)
    }

    fn write(&mut self, device: &mut BlockDev, lba: usize, data: &[u8]) -> Result<(), ()> {
        const BLOCK_SIZE: usize = 2048;

        let mut bytes_written: usize = 0;
        let mut remaining: usize = data.len();
        let mut pos: usize = lba;
        let mut direct_zones = [0u32; Inode::DIRECT_SLOT_COUNT];
        for (i, slot) in direct_zones.iter_mut().enumerate() {
            *slot = self.inode.direct_slot_get(i);
        }

        // ---- direct blocks ----
        while remaining > 0 {
            let block_idx = pos / BLOCK_SIZE;
            if block_idx >= direct_zones.len() {
                break;
            }

            let offset_in_block = pos % BLOCK_SIZE;
            let max_copy = BLOCK_SIZE - offset_in_block;
            let copy_size = core::cmp::min(remaining, max_copy);

            if direct_zones[block_idx] == 0 {
                let zone = self.ctx.lock().alloc_zone().unwrap();
                direct_zones[block_idx] = zone;
                let zero_block = [0u8; BLOCK_SIZE];
                if write_tfs_block(device.lock().as_mut(), zone, &zero_block).is_err() {
                    return Err(());
                }
            }

            let zone = direct_zones[block_idx];
            let mut buffer = [0u8; BLOCK_SIZE];

            // Preserve existing content for partial-block writes.
            if offset_in_block != 0 || copy_size < BLOCK_SIZE {
                if read_tfs_block(device.lock().as_mut(), zone, &mut buffer).is_err() {
                    return Err(());
                }
            }

            buffer[offset_in_block..offset_in_block + copy_size]
                .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);

            if write_tfs_block(device.lock().as_mut(), zone, &buffer).is_err() {
                return Err(());
            }

            bytes_written += copy_size;
            remaining -= copy_size;
            pos += copy_size;
        }

        // ---- single indirect blocks ----
        if remaining > 0 {
            let ind_cap = (BLOCK_SIZE / 4) - 1;
            let direct_blocks = direct_zones.len();

            if self.inode.single_indirect_get() == 0 {
                let zone = self.ctx.lock().alloc_zone().unwrap();
                self.inode.single_indirect_set(zone);
                let zero_block = [0u8; BLOCK_SIZE];
                if write_tfs_block(device.lock().as_mut(), zone, &zero_block).is_err() {
                    return Err(());
                }
            }

            let mut indirect_block = [0u8; BLOCK_SIZE];
            if read_tfs_block(
                device.lock().as_mut(),
                self.inode.single_indirect_get(),
                &mut indirect_block,
            )
            .is_err()
            {
                return Err(());
            }

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
                let entry = u32::from_le_bytes([
                    indirect_block[entry_off],
                    indirect_block[entry_off + 1],
                    indirect_block[entry_off + 2],
                    indirect_block[entry_off + 3],
                ]);

                let zone = if entry == 0 {
                    let new_zone = self.ctx.lock().alloc_zone().unwrap();
                    indirect_block[entry_off..entry_off + 4]
                        .copy_from_slice(&new_zone.to_le_bytes());
                    let zero_block = [0u8; BLOCK_SIZE];
                    if write_tfs_block(device.lock().as_mut(), new_zone, &zero_block).is_err() {
                        return Err(());
                    }
                    indirect_dirty = true;
                    new_zone
                } else {
                    entry
                };

                let mut buffer = [0u8; BLOCK_SIZE];

                if offset_in_block != 0 || copy_size < BLOCK_SIZE {
                    if read_tfs_block(device.lock().as_mut(), zone, &mut buffer).is_err() {
                        return Err(());
                    }
                }

                buffer[offset_in_block..offset_in_block + copy_size]
                    .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);

                if write_tfs_block(device.lock().as_mut(), zone, &buffer).is_err() {
                    return Err(());
                }

                bytes_written += copy_size;
                remaining -= copy_size;
                pos += copy_size;
            }

            if indirect_dirty {
                if write_tfs_block(
                    device.lock().as_mut(),
                    self.inode.single_indirect_get(),
                    &indirect_block,
                )
                .is_err()
                {
                    return Err(());
                }
            }
        }

        if remaining > 0 {
            const BLOCK_SIZE: usize = 2048;
            let zone_entries = BLOCK_SIZE / 4;
            let ind_cap = zone_entries - 1; // you iterate 0..(zone_entries - 1)
            let direct_bytes = direct_zones.len() * BLOCK_SIZE; // 7 * 2048
            let single_bytes = ind_cap * BLOCK_SIZE; // single-indirect payload

            // lba inside the "double-indirect region"
            let (first_block_idx, first_block_off) = if lba > direct_bytes + single_bytes {
                let delta = lba - (direct_bytes + single_bytes);
                (delta / BLOCK_SIZE, delta % BLOCK_SIZE)
            } else {
                (0, 0)
            };

            if self.inode.double_indirect_get() == 0 {
                self.inode
                    .double_indirect_set(self.ctx.lock().alloc_zone().unwrap());
                let zero_block = [0u8; BLOCK_SIZE];
                if let Err(_) = write_tfs_block(
                    device.lock().as_mut(),
                    self.inode.double_indirect_get(),
                    &zero_block,
                ) {
                    return Err(());
                }
            }

            let mut double_indirect_block = [0u8; BLOCK_SIZE];
            if let Err(_) = read_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_get(),
                &mut double_indirect_block,
            ) {
                return Err(());
            }

            let mut logical_idx: usize = 0; // index inside double-indirect payload

            for i in 0..ind_cap {
                if remaining == 0 {
                    break;
                }

                // get or alloc the indirect zone for this i
                let indirect_zone = {
                    let entry = u32::from_le_bytes([
                        double_indirect_block[i * 4],
                        double_indirect_block[i * 4 + 1],
                        double_indirect_block[i * 4 + 2],
                        double_indirect_block[i * 4 + 3],
                    ]);
                    if entry == 0 {
                        let new_zone = self.ctx.lock().alloc_zone().unwrap();
                        double_indirect_block[i * 4..i * 4 + 4]
                            .copy_from_slice(&new_zone.to_le_bytes());
                        let zero_block = [0u8; BLOCK_SIZE];
                        if let Err(_) =
                            write_tfs_block(device.lock().as_mut(), new_zone, &zero_block)
                        {
                            return Err(());
                        }
                        new_zone
                    } else {
                        entry
                    }
                };

                let mut indirect_block = [0u8; BLOCK_SIZE];
                if let Err(_) =
                    read_tfs_block(device.lock().as_mut(), indirect_zone, &mut indirect_block)
                {
                    return Err(());
                }

                for j in 0..ind_cap {
                    if remaining == 0 {
                        break;
                    }

                    // skip blocks that are before lba inside double-indirect region
                    if logical_idx < first_block_idx {
                        logical_idx += 1;
                        continue;
                    }

                    // get or alloc the actual data zone
                    let zone = {
                        let entry = u32::from_le_bytes([
                            indirect_block[j * 4],
                            indirect_block[j * 4 + 1],
                            indirect_block[j * 4 + 2],
                            indirect_block[j * 4 + 3],
                        ]);
                        if entry == 0 {
                            let new_zone = self.ctx.lock().alloc_zone().unwrap();
                            indirect_block[j * 4..j * 4 + 4]
                                .copy_from_slice(&new_zone.to_le_bytes());
                            let zero_block = [0u8; BLOCK_SIZE];
                            if let Err(_) =
                                write_tfs_block(device.lock().as_mut(), new_zone, &zero_block)
                            {
                                return Err(());
                            }
                            new_zone
                        } else {
                            entry
                        }
                    };

                    let mut buffer = [0u8; BLOCK_SIZE];

                    // For the very first block we may start in the middle.
                    let offset_in_block = if logical_idx == first_block_idx {
                        first_block_off
                    } else {
                        0
                    };

                    let max_copy = BLOCK_SIZE - offset_in_block;
                    let copy_size = core::cmp::min(remaining, max_copy);

                    // If we are not overwriting the full block, preserve existing contents.
                    if offset_in_block != 0 || copy_size < BLOCK_SIZE {
                        if let Err(_) = read_tfs_block(device.lock().as_mut(), zone, &mut buffer) {
                            return Err(());
                        }
                    }

                    buffer[offset_in_block..offset_in_block + copy_size]
                        .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);

                    if let Err(_) = write_tfs_block(device.lock().as_mut(), zone, &buffer) {
                        return Err(());
                    }

                    bytes_written += copy_size;
                    remaining -= copy_size;
                    logical_idx += 1;
                }

                // store updated indirect block
                if let Err(_) =
                    write_tfs_block(device.lock().as_mut(), indirect_zone, &indirect_block)
                {
                    return Err(());
                }
            }

            // store updated double indirect root
            if let Err(_) = write_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_get(),
                &double_indirect_block,
            ) {
                return Err(());
            }
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

        self.shared.invalidate_all();
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
            self.shared.invalidate_all();
            return Ok(());
        }

        // Extend by writing zeros from current size up to `len`.
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
    fn get_zone(&self, device: &mut BlockDev, logical_block: usize) -> Result<u32, ()> {
        let block_size = 2048;
        if logical_block < Inode::DIRECT_SLOT_COUNT {
            return Ok(self.inode.direct_slot_get(logical_block));
        }

        let indirect_start = Inode::DIRECT_SLOT_COUNT;
        let indirect_entries = block_size / 4; // 512

        if logical_block < indirect_start + indirect_entries {
            if self.inode.single_indirect_get() == 0 {
                return Ok(0);
            }
            let idx = logical_block - indirect_start;
            // Cache this? For now read it.
            let mut buf = [0u8; 2048];
            read_tfs_block(
                device.lock().as_mut(),
                self.inode.single_indirect_get(),
                &mut buf,
            )
                .map_err(|_| ())?;
            return Ok(u32::from_le_bytes(
                buf[idx * 4..(idx + 1) * 4].try_into().unwrap(),
            ));
        }

        let double_start = indirect_start + indirect_entries;
        let double_entries = indirect_entries * indirect_entries; // 512 * 512

        if logical_block < double_start + double_entries {
            if self.inode.double_indirect_get() == 0 {
                return Ok(0);
            }
            let rel = logical_block - double_start;
            let l1_idx = rel / indirect_entries;
            let l2_idx = rel % indirect_entries;

            let mut buf = [0u8; 2048];
            read_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_get(),
                &mut buf,
            )
            .map_err(|_| ())?;
            let l1_zone = u32::from_le_bytes(buf[l1_idx * 4..(l1_idx + 1) * 4].try_into().unwrap());
            if l1_zone == 0 {
                return Ok(0);
            }

            read_tfs_block(device.lock().as_mut(), l1_zone, &mut buf).map_err(|_| ())?;
            return Ok(u32::from_le_bytes(
                buf[l2_idx * 4..(l2_idx + 1) * 4].try_into().unwrap(),
            ));
        }

        Ok(0)
    }

    fn read_all_file(&self, device: &mut BlockDev) -> Result<Vec<u8>, ()> {
        let file_size = self.inode.size as usize;
        let mut out = vec![0u8; file_size];

        let block_size = 2048;
        let temp_buf_len = 128 * block_size; // Max 256KB temporary buffer
        let mut temp_buf = vec![0u8; temp_buf_len];
        // Note: allocating large vec in kernel can be risky if allocator is fragmented/limited,
        // but this is userspace cache read.
        // Better: use the output buffer directly for full block reads?
        // `out` is contiguous in virtual memory but not necessarily in physical.
        // `read_blocks` eventually calls `read` on BlockDeviceIO which might handle virt/phys translation
        // IF implemented. `ata_dma` does handle virt buffers.
        // So we can try to read directly into `out`.

        let mut zones = Vec::new();

        // Gather all zones

        // 1. Direct
        for i in 0..Inode::DIRECT_SLOT_COUNT {
            let zone = self.inode.direct_slot_get(i);
            if zone == 0 {
                break;
            }
            zones.push(zone);
        }

        // 2. Indirect
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

        // 3. Double indirect (simplified: read one by one to gather zones)
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

        // Clean implementation of coalesce loop
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

            // Limit by temp buf size
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
