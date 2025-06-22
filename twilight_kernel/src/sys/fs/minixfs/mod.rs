use alloc::vec::Vec;
use crate::driver::disk::{BlockDevice, BlockDeviceIO};
use core::mem::size_of;
use crate::println;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub ninodes: u16,
    pub pad1: u16,
    pub imap_blocks: u16,
    pub zmap_blocks: u16,
    pub first_data_zone: u16,
    pub log_zone_size: u16,
    pub pad2: u16,
    pub max_size: u32,
    pub zones: u32,
    pub magic: u16,
    pub pad3: u16,
    pub block_size: u16,
    pub subversion: u8,
}

#[repr(C, packed)]
pub struct Inode {
    pub mode: u16,
    pub uid: u16,
    pub size: u32,
    pub time: u32,
    pub gid: u8,
    pub nlinks: u8,
    pub zones: [u16; 9],
}

#[repr(C, packed)]
pub struct DirEntry {
    pub inode: u16,
    pub name: [u8; 60], // MINIX v1/v2 uses fixed 14-byte names
}


fn ceil_div(a: usize, b: usize) -> usize {
    a.div_ceil(b)
}

fn calculate_superblock(
    disk_size: usize,
    block_size: usize,
    ninodes: usize,
) -> Superblock {
    let bits_per_block = block_size * 8;
    let inode_size = 64;

    let imap_blocks = ceil_div(ninodes, bits_per_block);
    let inode_blocks = ceil_div(ninodes * inode_size, block_size);

    // Start with a rough estimate
    let reserved_fixed = 1; // superblock (no boot block)
    let mut zmap_blocks = 1;

    let mut total_blocks;
    let mut zones;

    loop {
        total_blocks = disk_size / block_size;
        let reserved = reserved_fixed + imap_blocks + zmap_blocks + inode_blocks;
        zones = total_blocks - reserved;
        let required_zmap_blocks = ceil_div(zones, bits_per_block);

        if required_zmap_blocks == zmap_blocks {
            break;
        }

        zmap_blocks = required_zmap_blocks;
    }

    let first_data_zone = (reserved_fixed + imap_blocks + zmap_blocks + inode_blocks) as u16;

    Superblock {
        ninodes: ninodes as u16,
        pad1: 0,
        imap_blocks: imap_blocks as u16,
        zmap_blocks: zmap_blocks as u16,
        first_data_zone,
        log_zone_size: 0,
        pad2: 0,
        max_size: 0x7FFF_FFFF,
        zones: zones as u32,
        magic: 0x138F,
        pad3: 0,
        block_size: block_size as u16,
        subversion: 0,
    }
}
pub fn format_superblock(
    block_device: &'static mut BlockDevice,
    disk_size: usize,
    ninodes: u16,
    block_size: u16,
) -> Result<MinixFs, &'static str> {
    let sb = calculate_superblock(disk_size, block_size as usize, ninodes as usize);

    let mut buffer = [0u8; 1024];
    let sb_bytes = unsafe {
        core::slice::from_raw_parts(
            &sb as *const _ as *const u8,
            size_of::<Superblock>(),
        )
    };
    buffer[..sb_bytes.len()].copy_from_slice(sb_bytes);

    if block_device.write(0, &buffer).is_err() {
        println!("ERROR: write failed while formatting supperblock");
    }
    Ok(MinixFs{superblock: sb, device: block_device})
}


pub fn read_superblock(device: &mut BlockDevice) -> Result<Superblock, &'static str> {
    let mut buf = [0u8; 1024];
    if device.read(0, &mut buf[0..1024]).is_err() {
        
    } // Superblock is usually at block 0
    let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };

    if sb.magic != 0x137F && sb.magic != 0x138F {
        return Err("Invalid MINIX magic");
    }
    Ok(sb)
}

pub struct MinixFs {
    pub superblock: Superblock,
    pub device: &'static mut BlockDevice,
}

impl MinixFs {
    pub fn check_ata(bus: u8, dsk: u8) -> bool {
        let mut buf = [0u8; 1024];
        if crate::driver::disk::ata::read(bus, dsk, 0, &mut buf).is_err() {
            return false;
        }
        let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };
        sb.magic == 0x137F || sb.magic == 0x138F
    }
    
    pub fn allocate_zone(&mut self) -> Result<u32, &'static str> {
        let bits_per_block = self.superblock.block_size as usize * 8;
        let zmap_start = self.superblock.imap_blocks + 2;

        let mut buf = [0u8; 1024];
        for i in 0..self.superblock.zmap_blocks {
            if self.device.read((zmap_start + i) as u32, &mut buf).is_err() {
                return Err("Failed to read zone bitmap");
            }

            for byte_idx in 0..buf.len() {
                if buf[byte_idx] != 0xFF {
                    for bit in 0..8 {
                        if buf[byte_idx] & (1 << bit) == 0 {
                            buf[byte_idx] |= 1 << bit;
                            if self.device.write((zmap_start + i) as u32, &buf).is_err() {
                                return Err("Failed to write zone bitmap");
                            }

                            let zone = i as u32 * bits_per_block as u32 + (byte_idx * 8 + bit) as u32;
                            return Ok(zone + self.superblock.first_data_zone as u32);
                        }
                    }
                }
            }
        }

        Err("No free Zone")
    }

    pub fn allocate_inode(&mut self) -> Result<u16, &'static str> {
        let bits_per_block = self.superblock.block_size as usize * 8;
        let total_inodes = self.superblock.ninodes as usize;

        for block_idx in 0..self.superblock.imap_blocks {
            let imap_block_lba = 1 + block_idx;
            let mut buf = [0u8; 1024];
            if self.device.read(imap_block_lba as u32, &mut buf).is_err() {
                return Err("Failed to read inode bitmap");
            }

            for byte_idx in 0..self.superblock.block_size as usize {
                let byte = buf[byte_idx];
                
                if byte != 0xFF {
                    for bit in 0..8 {
                        if byte & (1 << bit) == 0 {
                            let inode_idx = (block_idx as usize * bits_per_block) + (byte_idx * 8) + bit;
                            if inode_idx >= total_inodes {
                                break;
                            }
                            
                            buf[byte_idx] |= 1 << bit;
                            if self.device.write(imap_block_lba as u32, &buf).is_err() {
                                return Err("Failed to write inode bitmap");
                            }
                            return Ok(inode_idx as u16);
                        }
                    }
                }
            }
        }

        Err("No free inode available")
    }

    pub fn free_zone(&mut self, zone: u32) -> Result<(), &'static str> {
        let first_zone = self.superblock.first_data_zone as u32;

        if zone < first_zone {
            return Err("Zone is before first data zone");
        }

        let relative_zone = zone - first_zone;
        let bits_per_block = self.superblock.block_size as usize * 8;
        let block_index = (relative_zone as usize) / bits_per_block;
        let bit_index = (relative_zone as usize) % bits_per_block;
        let byte_index = bit_index / 8;
        let bit = bit_index % 8;

        if block_index >= self.superblock.zmap_blocks as usize {
            return Err("Zone bitmap block index out of bounds");
        }

        let zmap_start = 2 + self.superblock.imap_blocks as u32;
        let zmap_block = zmap_start + block_index as u32;

        let mut buf = [0u8; 1024];
        if self.device.read(zmap_block, &mut buf).is_err() {
            return Err("Failed to read zone bitmap");
        }

        buf[byte_index] &= !(1 << bit);

        if self.device.write(zmap_block, &buf).is_err() {
            return Err("Failed to write zone bitmap");
        }

        Ok(())
    }

    pub fn free_inode(&mut self, inode: u16) -> Result<(), &'static str> {
        if inode == 0 || inode as usize > self.superblock.ninodes as usize {
            return Err("Invalid inode number");
        }

        let inode_index = inode as usize - 1; // MINIX inodes are 1-based
        let bits_per_block = self.superblock.block_size as usize * 8;

        let block_index = inode_index / bits_per_block;
        let bit_index = inode_index % bits_per_block;
        let byte_index = bit_index / 8;
        let bit_in_byte = bit_index % 8;

        let imap_block_lba = 1 + block_index as u32;
        let mut buffer = [0u8; 1024];
        if self.device.read(imap_block_lba, &mut buffer).is_err() {
            return Err("Failed to read inode bitmap");
        }

        buffer[byte_index] &= !(1 << bit_in_byte); // clear the bit

        if self.device.write(imap_block_lba, &buffer).is_err() {
            return Err("Failed to write inode bitmap");
        }
        
        Ok(())
    }

    pub fn write_inode(&mut self, inode_num: u16, inode: &Inode) -> Result<(), &'static str> {
        if inode_num == 0 || inode_num as usize > self.superblock.ninodes as usize {
            return Err("Invalid inode number");
        }

        let inode_index = (inode_num - 1) as usize;
        let inode_size = size_of::<Inode>();
        let block_size = self.superblock.block_size as usize;
        let inodes_per_block = block_size / inode_size;

        let inode_table_start = self.superblock.imap_blocks + self.superblock.zmap_blocks + 2;
        let block_offset = inode_index / inodes_per_block;
        let byte_offset = (inode_index % inodes_per_block) * inode_size;
        let block_num = inode_table_start + block_offset as u16;

        let mut buffer = [0u8; 1024];
        if self.device.read(block_num as u32, &mut buffer).is_err() {
            return Err("Failed to read inode block");
        }

        let inode_bytes = unsafe {
            core::slice::from_raw_parts(
                inode as *const _ as *const u8,
                size_of::<Inode>(),
            )
        };
        buffer[byte_offset..byte_offset + inode_size].copy_from_slice(inode_bytes);
        
        if self.device.write(block_num as u32, &buffer).is_err() {
            return Err("Failed to write inode block");
        }
        
        Ok(())
    }
    
    pub fn read_inode(&mut self, inode_num: u16) -> Result<Inode, &'static str> {
        if inode_num == 0 || inode_num as usize > self.superblock.ninodes as usize {
            return Err("Invalid inode number");
        }


        let inode_index = (inode_num - 1) as usize;
        let inode_size = size_of::<Inode>();
        let block_size = self.superblock.block_size as usize;
        let inodes_per_block = block_size / inode_size;

        let inode_table_start = self.superblock.imap_blocks + self.superblock.zmap_blocks + 2;
        let block_offset = inode_index / inodes_per_block;
        let byte_offset = (inode_index % inodes_per_block) * inode_size;
        let block_num = inode_table_start + block_offset as u16;

        let mut buffer = [0u8; 1024];
        if self.device.read(block_num as u32, &mut buffer).is_err() {
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

    pub fn create_dir_entry(
        &mut self,
        parent_inode_num: u16,
        name: &str,
        child_inode_num: u16,
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
        
        let zones = parent_inode.zones;

        for i in 0..zones.len() {
            if parent_inode.zones[i] == 0 {
                let zone = self.allocate_zone()?;
                parent_inode.zones[i] = zone as u16;
                self.write_inode(parent_inode_num, &parent_inode)?;
            }

            let block = parent_inode.zones[i];
            let mut buf = [0u8; 1024];
            if self.device.read(block.into(), &mut buf).is_err() {
                return Err("Failed to read block"); 
            }

            for j in 0..entries_per_block {
                let offset = j * dir_entry_size;
                let inode_field = u16::from_le_bytes([buf[offset], buf[offset + 1]]);
                if inode_field == 0 {
                    // Found empty slot
                    let entry_bytes = unsafe {
                        core::slice::from_raw_parts(
                            &entry as *const _ as *const u8,
                            dir_entry_size,
                        )
                    };
                    buf[offset..offset + dir_entry_size].copy_from_slice(entry_bytes);
                    if self.device.write(block.into(), &buf).is_err() {
                        return Err("Failed to write block"); 
                    }
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

    pub fn read_dir_entries(&mut self, inode: &Inode) -> Result<Vec<DirEntry>, &'static str> {
        let dir_entry_size = size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;
        let mut entries = Vec::new();

        let mut buf = [0u8; 1024];
        
        let zones = inode.zones;

        for &zone in zones.iter() {
            if zone == 0 {
                continue;
            }

            if self.device.read(zone.into(), &mut buf).is_err() {
                return Err("Failed to read block");
            }
            for i in 0..entries_per_block {
                let offset = i * dir_entry_size;
                let raw = &buf[offset..offset + dir_entry_size];
                let inode = u16::from_le_bytes([raw[0], raw[1]]);
                if inode == 0 {
                    continue;
                }

                let mut name = [0u8; 60];
                name.copy_from_slice(&raw[2..62]);

                entries.push(DirEntry { inode, name });
            }
        }

        Ok(entries)
    }

}