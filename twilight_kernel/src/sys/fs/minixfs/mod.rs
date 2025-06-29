use alloc::boxed::Box;
use alloc::format;
use alloc::vec::Vec;
use crate::driver::disk::{BlockDevice, BlockDeviceIO};
use core::mem::size_of;
use crate::{driver, println};
use crate::sys::fs::minixfs::FsError::{FileAlreadyExists, FileNameTooLong, FileNotFound, InvalidInode};

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
#[derive(Debug, Clone, Copy)]
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
#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub inode: u16,
    pub name: [u8; 60], // MINIX v1/v2 uses fixed 14-byte names
}

#[derive(Debug)]
pub enum FsError {
    FileAlreadyExists,
    FileNotFound,
    InvalidPath,
    InvalidInode,
    FileNameTooLong,
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

    // Start with an estimate
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
    pub fn resolve_path(&mut self, path: &str) -> Result<u16, FsError> {
        if path.is_empty() {
            return Err(FsError::InvalidPath);
        }

        // Start from root inode (assumed to be inode number 1)
        let mut current_inode = 1;

        // Skip empty and root path
        let path_parts = path.split('/').filter(|s| !s.is_empty());

        for part in path_parts {
            match self.find_dir_entry(current_inode, part).unwrap() {
                Some(inode) => current_inode = inode,
                None => return Err(FileNotFound),
            }
        }

        Ok(current_inode)
    }
    
    pub fn check_ata(bus: u8, dsk: u8) -> Result<MinixFs, &'static str> {
        let mut buf = [0u8; 1024];

        // Try to read block 0 (superblock)
        if crate::driver::disk::ata::read(bus, dsk, 0, &mut buf).is_err() {
            return Err("Failed to read block 0");
        }

        // Interpret as Superblock
        let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };

        // Validate magic number
        if sb.magic != 0x137F && sb.magic != 0x138F {
            return Err("Invalid MINIX superblock magic");
        }

        // Open the device
        if let Some(device) = driver::disk::AtaBlockDevice::new(bus, dsk) {
            let leaked = Box::leak(Box::new(device)); // &'static mut AtaBlockDevice
            let leaked_device = Box::leak(Box::new(BlockDevice::Ata(leaked)));
            
            Ok(MinixFs {
                superblock: sb,
                device: leaked_device,
            })
        } else { 
            Err("Failed to open ATA device")
        }
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
                    parent_inode.size += dir_entry_size as u32;
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

    pub fn create_file(&mut self, parent_inode_num: u16, name: &str) -> Result<u16, FsError> {
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

        // Initialize inode
        let mut inode = Inode {
            mode: 0o100777, // Regular file with full permissions
            uid: 0,
            size: 0,
            time: 0,
            gid: 0,
            nlinks: 0,
            zones: [0; 9],
        };
        inode.zones[0] = new_zone as u16;

        self.write_inode(new_inode_num, &inode).unwrap();

        // Add to the parent directory
        self.create_dir_entry(parent_inode_num, name, new_inode_num).unwrap();

        Ok(new_inode_num)
    }

    pub fn list_dir(&mut self, dir_inode_num: u16) -> Result<(), &'static str> {
        let dir_inode = self.read_inode(dir_inode_num)?;

        if dir_inode.zones[0] == 0 {
            return Err("Directory has no data block");
        }

        let mut buffer = [0u8; 1024];
        if self.device.read(dir_inode.zones[0] as u32, &mut buffer).is_err(){
            return Err("Failed to read directory block");       
        };
        
        let mut offset = 0;
        while offset + 16 <= dir_inode.size as usize {
            let entry = unsafe {
                core::ptr::read(buffer[offset..].as_ptr() as *const DirEntry)
            };

            let name = core::str::from_utf8(&entry.name)
                .unwrap_or("")
                .trim_end_matches('\0');
            let child_inode = self.read_inode(entry.inode)?;
            let file_type = match child_inode.mode & 0xF000 {
                0x4000 => "dir",
                0x8000 => "file",
                _ => "unknown",
            };

            if file_type == "dir" {
                println!("\x1b[94m{}\x1b[0m", name);    
            } else {
                println!("{}", name);    
            }
            
            
            offset += size_of::<DirEntry>();
        }

        Ok(())
    }

    pub fn create_dir(&mut self, parent_inode_num: u16, name: &str) -> Result<u16, FsError> {
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

        // Create the new directory inode
        let mut inode = Inode {
            mode: 0o040777, // Directory with full permissions
            uid: 0,
            size: 0,
            time: 0,
            gid: 0,
            nlinks: 2, // "." and ".."
            zones: [0; 9],
        };
        inode.zones[0] = new_zone as u16;
        self.write_inode(new_inode_num, &inode).unwrap();

        // Add directory entry to the parent directory
        self.create_dir_entry(parent_inode_num, name, new_inode_num).unwrap();

        // Create "." and ".." entries inside the new directory
        self.create_dir_entry(new_inode_num, ".", new_inode_num).unwrap();
        self.create_dir_entry(new_inode_num, "..", parent_inode_num).unwrap();

        Ok(new_inode_num)
    }

    pub fn write_file(&mut self, inode_num: u16, data: &[u8]) -> Result<(), FsError> {
        if inode_num == 0 || inode_num as usize > self.superblock.ninodes as usize {
            return Err(InvalidInode);
        }

        let mut inode = self.read_inode(inode_num).unwrap();
        let block_size = self.superblock.block_size as usize;

        let mut bytes_written = 0;
        let mut remaining = data.len();
        
        let zones = inode.zones;

        for i in 0..zones.len() {
            if remaining == 0 {
                break;
            }

            if inode.zones[i] == 0 {
                let zone = self.allocate_zone().unwrap();
                inode.zones[i] = zone as u16;
            }

            let block = inode.zones[i] as u32;
            let mut buffer = [0u8; 1024]; // assumes 1024-byte blocks

            let copy_size = core::cmp::min(block_size, remaining);
            buffer[..copy_size].copy_from_slice(&data[bytes_written..bytes_written + copy_size]);

            self.device.write(block, &buffer).unwrap();

            bytes_written += copy_size;
            remaining -= copy_size;
        }

        if remaining > 0 {
            return Err(FileNameTooLong);
        }

        inode.size = bytes_written as u32;
        self.write_inode(inode_num, &inode).unwrap();

        Ok(())
    }

    pub fn find_dir_entry(
        &mut self,
        parent_inode_num: u16,
        name: &str,
    ) -> Result<Option<u16>, &'static str> {
        let parent_inode = self.read_inode(parent_inode_num)?;

        if parent_inode.zones[0] == 0 {
            return Ok(None);
        }

        let dir_entry_size = size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;
        let mut buffer = [0u8; 1024];

        let zones = parent_inode.zones;
        
        for &zone in zones.iter() {
            if zone == 0 {
                continue;
            }

            self.device.read(zone as u32, &mut buffer).unwrap();

            for i in 0..entries_per_block {
                let offset = i * dir_entry_size;
                let entry = unsafe {
                    core::ptr::read(buffer[offset..].as_ptr() as *const DirEntry)
                };

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

    pub fn read_file(&mut self, inode_num: u16) -> Result<Vec<u8>, &'static str> {
        let inode = self.read_inode(inode_num)?;

        let mut content = Vec::new();
        let mut remaining = inode.size as usize;
        let block_size = self.superblock.block_size as usize;
        let mut buffer = [0u8; 1024];

        let zones = inode.zones;
        for &zone in zones.iter() {
            if zone == 0 {
                break;
            }

            let to_read = core::cmp::min(remaining, block_size);

            self.device.read(zone as u32, &mut buffer).unwrap();
            content.extend_from_slice(&buffer[..to_read]);

            remaining -= to_read;
            if remaining == 0 {
                break;
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
        let dir_entry_size = core::mem::size_of::<DirEntry>();
        let entries_per_block = self.superblock.block_size as usize / dir_entry_size;
        
        let zones = parent_inode.zones;

        for &zone in zones.iter() {
            if zone == 0 {
                continue;
            }

            let mut buf = [0u8; 1024];
            if self.device.read(zone as u32, &mut buf).is_err() {
                return Err(InvalidInode);
            }

            for i in 0..entries_per_block {
                let offset = i * dir_entry_size;
                let entry = unsafe {
                    core::ptr::read(buf[offset..].as_ptr() as *const DirEntry)
                };

                let entry_name = core::str::from_utf8(&entry.name)
                    .unwrap_or("")
                    .trim_end_matches('\0');

                if entry.inode != 0 && entry_name == target_name {
                    let inode_num = entry.inode;
                    let inode = self.read_inode(inode_num).unwrap();
                    
                    let i_zones = inode.zones;
                    
                    // Free all zones
                    for &z in i_zones.iter() {
                        if z != 0 {
                            self.free_zone(z as u32).unwrap();
                        }
                    }

                    // Free inode
                    self.free_inode(inode_num).unwrap();

                    buf[offset..offset + dir_entry_size].fill(0);
                    self.device.write(zone as u32, &buf).unwrap();

                    // Update parent inode size if large enough
                    if parent_inode.size >= dir_entry_size as u32 {
                        parent_inode.size -= dir_entry_size as u32;
                    }
                    self.write_inode(parent_inode_num, &parent_inode).unwrap();

                    return Ok(());
                }
            }
        }

        Err(FileNotFound)
    }

}