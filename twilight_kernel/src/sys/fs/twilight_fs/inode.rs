use crate::sys::fs::twilight_fs::{read_tfs_block, write_tfs_block};
use crate::sys::fs::vfs::{BlockDev, FsCtx, VfsNodeOps};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use twilight_common::syscall::types::EISDIR;

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
}

unsafe impl Send for TFSVfsNode {}
unsafe impl Sync for TFSVfsNode {}

#[allow(dead_code)]
impl VfsNodeOps for TFSVfsNode {
    fn read(&self, device: &mut BlockDev, _lba: usize, _buf: &mut [u8]) -> Result<Vec<u8>, ()> {
        let mut content = Vec::new();
        let mut remaining = self.inode.size as usize;
        let block_size = 2048;
        let mut buffer = [0u8; 2048];

        let zones = self.inode.zones;
        for &zone in zones.iter() {
            if zone == 0 {
                break;
            }

            let to_read = core::cmp::min(remaining, block_size);

            if let Err(_) = read_tfs_block(device.lock().as_mut(), zone, &mut buffer) {
                return Err(());
            }
            content.extend_from_slice(&buffer[..to_read]);

            remaining -= to_read;
            if remaining == 0 {
                break;
            }
        }

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

                let to_read = core::cmp::min(remaining, block_size);

                let mut indirect_content_buf = [0u8; 2048];

                if let Err(_) =
                    read_tfs_block(device.lock().as_mut(), zone_id, &mut indirect_content_buf)
                {
                    return Err(());
                }
                content.extend_from_slice(&indirect_content_buf[..to_read]);

                remaining -= to_read;
                if remaining == 0 {
                    break;
                }
            }
        }

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

                    let to_read = core::cmp::min(remaining, block_size);

                    let mut indirect_content_buf = [0u8; 2048];
                    if let Err(_) =
                        read_tfs_block(device.lock().as_mut(), zone_id, &mut indirect_content_buf)
                    {
                        return Err(());
                    }
                    content.extend_from_slice(&indirect_content_buf);

                    remaining -= to_read;
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }

        Ok(content)
    }

    fn write(&mut self, device: &mut BlockDev, lba: usize, data: &[u8]) -> Result<(), ()> {
        let block_size = 2048;
        let mut bytes_written = 0;
        let mut remaining = data.len();

        let zones = self.inode.zones;

        for i in 0..zones.len() {
            if lba / 2048 < i {
                continue;
            }
            if remaining == 0 {
                break;
            }

            if self.inode.zones[i] == 0 {
                let zones = self.ctx.lock().alloc_zone().unwrap();
                self.inode.zones[i] = zones;
            }

            let block = self.inode.zones[i];
            let mut buffer = [0u8; 2048];

            let copy_size = core::cmp::min(remaining, block_size);
            if lba != 0 && lba > i * 2048 {
                if let Err(_) = read_tfs_block(device.lock().as_mut(), block, &mut buffer) {
                    return Err(());
                }
                buffer[(lba % 2048)..((lba % 2048) + copy_size)]
                    .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);
            } else {
                buffer[..copy_size]
                    .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);
            }
            if let Err(_) = write_tfs_block(device.lock().as_mut(), block, &buffer) {
                return Err(());
            }

            bytes_written += copy_size;
            remaining -= copy_size;
        }

        if remaining > 0 {
            if self.inode.indirect_zones == 0 {
                let zones = self.ctx.lock().alloc_zone().unwrap();
                self.inode.indirect_zones = zones;
                let zero_buf = [0u8; 2048];
                if let Err(_) = write_tfs_block(device.lock().as_mut(), zones, &zero_buf) {
                    return Err(());
                }
            }

            let mut indirect_block = [0u8; 2048];
            self.ctx
                .lock()
                .read_block(self.inode.indirect_zones, &mut indirect_block)?;

            let zone_entries = 2048 / 4;
            for i in 0..(zone_entries - 1) {
                if lba / 2048 < i + 7 {
                    continue;
                }
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
                    let zones = self.ctx.lock().alloc_zone().unwrap();
                    indirect_block[i * 4..i * 4 + 4].copy_from_slice(&zones.to_le_bytes());
                    zones
                } else {
                    entry
                };

                let mut buffer = [0u8; 2048];
                let copy_size = core::cmp::min(remaining, block_size);
                if lba != 0 && lba > (i + 7) * 2048 {
                    if let Err(_) = read_tfs_block(device.lock().as_mut(), zone, &mut buffer) {
                        return Err(());
                    }
                    buffer[(lba % 2048)..((lba % 2048) + copy_size)]
                        .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);
                } else {
                    buffer[..copy_size]
                        .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);
                }
                if let Err(_) = write_tfs_block(device.lock().as_mut(), zone, &buffer) {
                    return Err(());
                }

                bytes_written += copy_size;
                remaining -= copy_size;
            }
        }

        if remaining > 0 {
            if self.inode.double_indirect_zones == 0 {
                self.inode.double_indirect_zones = self.ctx.lock().alloc_zone().unwrap();
                let zero_block = [0u8; 2048];
                if let Err(_) = write_tfs_block(
                    device.lock().as_mut(),
                    self.inode.double_indirect_zones,
                    &zero_block,
                ) {
                    return Err(());
                }
            }

            let mut double_indirect_block = [0u8; 2048];
            if let Err(_) = read_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_zones,
                &mut double_indirect_block,
            ) {
                return Err(());
            }

            let zone_entries = 2048 / 4;
            for i in 0..(zone_entries - 1) {
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
                        let new_zone = self.ctx.lock().alloc_zone().unwrap();
                        double_indirect_block[i * 4..i * 4 + 4]
                            .copy_from_slice(&new_zone.to_le_bytes());
                        let zero_block = [0u8; 2048];
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

                let mut indirect_block = [0u8; 2048];
                if let Err(_) =
                    read_tfs_block(device.lock().as_mut(), indirect_zone, &mut indirect_block)
                {
                    return Err(());
                }

                let zone_entries = 2048 / 4;
                for j in 0..(zone_entries - 1) {
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
                            let new_zone = self.ctx.lock().alloc_zone().unwrap();
                            indirect_block[j * 4..j * 4 + 4]
                                .copy_from_slice(&new_zone.to_le_bytes());
                            let zero_block = [0u8; 2048];
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

                    let mut buffer = [0u8; 2048];
                    let copy_size = core::cmp::min(block_size, remaining);

                    buffer[..copy_size]
                        .copy_from_slice(&data[bytes_written..bytes_written + copy_size]);
                    if let Err(_) = write_tfs_block(device.lock().as_mut(), zone, &buffer) {
                        return Err(());
                    }

                    bytes_written += copy_size;
                    remaining -= copy_size;
                }

                // store updated indirect block
                if let Err(_) =
                    write_tfs_block(device.lock().as_mut(), indirect_zone, &indirect_block)
                {
                    return Err(());
                }
            }

            // store updated double indirect block
            if let Err(_) = write_tfs_block(
                device.lock().as_mut(),
                self.inode.double_indirect_zones,
                &double_indirect_block,
            ) {
                return Err(());
            }
        }

        self.inode.size = (bytes_written + lba) as u64;
        self.ctx
            .lock()
            .write_inode_twilight(self.inode_no, self.inode)
            .unwrap();

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
}
