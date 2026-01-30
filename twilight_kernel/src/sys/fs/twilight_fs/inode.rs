use crate::sys::fs::twilight_fs::{read_tfs_block, write_tfs_block, TwilightFsShared};
use crate::sys::fs::vfs::{BlockDev, FsCtx, VfsNodeOps};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use twilight_common::syscall::types::{EIO, EISDIR};

#[allow(dead_code)]
#[repr(u16)]
pub enum FileType {
    File,
    Directory,
    Symlink,
    BlockDevice,
    CharacterDevice,
    Socket,
    Pipe,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Inode {
    pub mode: u16,
    pub nlinks: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub access_time: u32,
    pub modified_time: u32,
    pub created_time: u32,
    pub zones: [u32; 7],
    pub indirect_zones: u32,
    pub double_indirect_zones: u32,
    pub triple_indirect_zones: u32,
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
        let mut buffer = [0u8; 2048];

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

        let zones = self.inode.zones;

        let start_block = lba / block_size;
        let mut block_offset = lba % block_size;

        for (idx, &zone) in zones.iter().enumerate() {
            if zone == 0 {
                break;
            }

            if idx < start_block {
                continue;
            }

            if let Err(_) = read_tfs_block(device.lock().as_mut(), zone, &mut buffer) {
                return Err(());
            }
            let start = block_offset;
            let available_in_block = block_size - start;
            let to_read = core::cmp::min(remaining, available_in_block);

            buf[written..written + to_read].copy_from_slice(&buffer[start..start + to_read]);

            remaining -= to_read;
            written += to_read;
            block_offset = 0;
            if remaining == 0 {
                return Ok(written);
            }
        }

        let mut block_index = zones.len(); // first indirect block index

        if self.inode.indirect_zones != 0 {
            if let Err(_) = read_tfs_block(
                device.lock().as_mut(),
                self.inode.indirect_zones,
                &mut buffer,
            ) {
                return Err(());
            };
            let zone_size = 2048 / 4;
            for i in 0..(zone_size - 1) {
                let zone_id_buf: [u8; 4] = buffer[i * 4..(i + 1) * 4]
                    .try_into()
                    .expect("invalid zone id size");
                let zone_id = u32::from_le_bytes(zone_id_buf);
                if zone_id == 0 {
                    break;
                }
                if block_index < start_block {
                    block_index += 1;
                    continue;
                }

                let mut indirect_content_buf = [0u8; 2048];

                if let Err(_) =
                    read_tfs_block(device.lock().as_mut(), zone_id, &mut indirect_content_buf)
                {
                    return Err(());
                }
                let start = block_offset;
                let available_in_block = block_size - start;
                let to_read = core::cmp::min(remaining, available_in_block);

                buf[written..written + to_read]
                    .copy_from_slice(&indirect_content_buf[start..start + to_read]);

                remaining -= to_read;
                written += to_read;
                block_offset = 0;
                block_index += 1;

                if remaining == 0 {
                    return Ok(written);
                }
            }
        }

        block_index = zones.len() + block_size / 4;

        if self.inode.double_indirect_zones != 0 {
            if let Err(_) = read_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_zones,
                &mut buffer,
            ) {
                return Err(());
            }
            let zone_size = 2048 / 4;
            for i in 0..(zone_size - 1) {
                let zone_id_buf: [u8; 4] = buffer[i * 4..(i + 1) * 4]
                    .try_into()
                    .expect("invalid zone id size");
                let zone_id = u32::from_le_bytes(zone_id_buf);
                if zone_id == 0 {
                    break;
                }

                let mut indirect_zones_buf = [0u8; 2048];
                if let Err(_) =
                    read_tfs_block(device.lock().as_mut(), zone_id, &mut indirect_zones_buf)
                {
                    return Err(());
                }

                for i in 0..(zone_size - 1) {
                    let zone_id_buf: [u8; 4] = indirect_zones_buf[i * 4..(i + 1) * 4]
                        .try_into()
                        .expect("invalid zone id size");
                    let zone_id = u32::from_le_bytes(zone_id_buf);
                    if zone_id == 0 {
                        break;
                    }
                    if block_index < start_block {
                        block_index += 1;
                        continue;
                    }

                    let mut indirect_content_buf = [0u8; 2048];
                    if let Err(_) =
                        read_tfs_block(device.lock().as_mut(), zone_id, &mut indirect_content_buf)
                    {
                        return Err(());
                    }
                    let start = block_offset;
                    let available_in_block = block_size - start;
                    let to_read = core::cmp::min(remaining, available_in_block);

                    buf[written..written + to_read]
                        .copy_from_slice(&indirect_content_buf[start..start + to_read]);

                    remaining -= to_read;
                    written += to_read;
                    block_offset = 0;
                    block_index += 1;
                    if remaining == 0 {
                        return Ok(written)
                    }
                }
            }
        }

        Ok(written)
    }

    fn write(&mut self, device: &mut BlockDev, lba: usize, data: &[u8]) -> Result<(), ()> {
        const BLOCK_SIZE: usize = 2048;

        let mut bytes_written: usize = 0;
        let mut remaining: usize = data.len();
        let mut pos: usize = lba;
        let mut direct_zones = self.inode.zones;

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

            if self.inode.indirect_zones == 0 {
                let zone = self.ctx.lock().alloc_zone().unwrap();
                self.inode.indirect_zones = zone;
                let zero_block = [0u8; BLOCK_SIZE];
                if write_tfs_block(device.lock().as_mut(), zone, &zero_block).is_err() {
                    return Err(());
                }
            }

            let mut indirect_block = [0u8; BLOCK_SIZE];
            if read_tfs_block(
                device.lock().as_mut(),
                self.inode.indirect_zones,
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
                    indirect_block[entry_off..entry_off + 4].copy_from_slice(&new_zone.to_le_bytes());
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
                    self.inode.indirect_zones,
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

            if self.inode.double_indirect_zones == 0 {
                self.inode.double_indirect_zones = self.ctx.lock().alloc_zone().unwrap();
                let zero_block = [0u8; BLOCK_SIZE];
                if let Err(_) = write_tfs_block(
                    device.lock().as_mut(),
                    self.inode.double_indirect_zones,
                    &zero_block,
                ) {
                    return Err(());
                }
            }

            let mut double_indirect_block = [0u8; BLOCK_SIZE];
            if let Err(_) = read_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_zones,
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
                self.inode.double_indirect_zones,
                &double_indirect_block,
            ) {
                return Err(());
            }
        }

        let end_pos = bytes_written + lba;
        if end_pos > self.inode.size as usize {
            self.inode.size = end_pos as u64;
        }
        self.inode.zones = direct_zones;
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
        if self.inode.mode == 0o040777 {
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
    fn read_all_file(&self, device: &mut BlockDev) -> Result<Vec<u8>, ()> {
        let file_size = self.inode.size as usize;
        let mut out = Vec::with_capacity(file_size);
        let mut remaining = file_size;

        if remaining == 0 {
            return Ok(out);
        }

        let mut block_buf = [0u8; 2048];

        let zones = self.inode.zones;
        for &zone in zones.iter() {
            if remaining == 0 {
                break;
            }
            if zone == 0 {
                break;
            }
            read_tfs_block(device.lock().as_mut(), zone, &mut block_buf).map_err(|_| ())?;
            let n = core::cmp::min(remaining, block_buf.len());
            out.extend_from_slice(&block_buf[..n]);
            remaining -= n;
        }

        if remaining == 0 {
            return Ok(out);
        }

        let indirect_zones = self.inode.indirect_zones;
        if indirect_zones != 0 {
            read_tfs_block(
                device.lock().as_mut(),
                indirect_zones,
                &mut block_buf,
            )
            .map_err(|_| ())?;

            let zone_entries = (block_buf.len() / 4) - 1;
            for i in 0..zone_entries {
                if remaining == 0 {
                    break;
                }
                let zone_id = u32::from_le_bytes(
                    block_buf[i * 4..(i + 1) * 4]
                        .try_into()
                        .map_err(|_| ())?,
                );
                if zone_id == 0 {
                    break;
                }
                let mut data_buf = [0u8; 2048];
                read_tfs_block(device.lock().as_mut(), zone_id, &mut data_buf).map_err(|_| ())?;
                let n = core::cmp::min(remaining, data_buf.len());
                out.extend_from_slice(&data_buf[..n]);
                remaining -= n;
            }
        }

        if remaining == 0 {
            return Ok(out);
        }

        let double_indirect_zones = self.inode.double_indirect_zones;
        if double_indirect_zones != 0 {
            read_tfs_block(
                device.lock().as_mut(),
                double_indirect_zones,
                &mut block_buf,
            )
            .map_err(|_| ())?;

            let zone_entries = (block_buf.len() / 4) - 1;
            for i in 0..zone_entries {
                if remaining == 0 {
                    break;
                }
                let indirect_zone = u32::from_le_bytes(
                    block_buf[i * 4..(i + 1) * 4]
                        .try_into()
                        .map_err(|_| ())?,
                );
                if indirect_zone == 0 {
                    break;
                }

                let mut indirect_buf = [0u8; 2048];
                read_tfs_block(device.lock().as_mut(), indirect_zone, &mut indirect_buf)
                    .map_err(|_| ())?;

                for j in 0..zone_entries {
                    if remaining == 0 {
                        break;
                    }
                    let zone_id = u32::from_le_bytes(
                        indirect_buf[j * 4..(j + 1) * 4]
                            .try_into()
                            .map_err(|_| ())?,
                    );
                    if zone_id == 0 {
                        break;
                    }
                    let mut data_buf = [0u8; 2048];
                    read_tfs_block(device.lock().as_mut(), zone_id, &mut data_buf)
                        .map_err(|_| ())?;
                    let n = core::cmp::min(remaining, data_buf.len());
                    out.extend_from_slice(&data_buf[..n]);
                    remaining -= n;
                }
            }
        }

        Ok(out)
    }
}
