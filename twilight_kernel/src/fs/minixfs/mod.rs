use crate::driver::disk::BlockDevice;
use core::mem::size_of;

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
    pub name: [u8; 14], // MINIX v1/v2 uses fixed 14-byte names
}


fn ceil_div(a: usize, b: usize) -> usize {
    (a + b - 1) / b
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
    block_device: &mut dyn BlockDevice,
    disk_size: usize,
    ninodes: u16,
    block_size: u16,
) -> Result<(), &'static str> {
    let sb = calculate_superblock(disk_size, block_size as usize, ninodes as usize);

    let mut buffer = [0u8; 1024];
    let sb_bytes = unsafe {
        core::slice::from_raw_parts(
            &sb as *const _ as *const u8,
            size_of::<Superblock>(),
        )
    };
    buffer[..sb_bytes.len()].copy_from_slice(sb_bytes);

    block_device.write_block(0, &buffer)?;
    Ok(())
}


pub fn read_superblock(device: &mut dyn BlockDevice) -> Result<Superblock, &'static str> {
    let mut buf = [0u8; 1024];
    device.read_block(0, &mut buf[0..1024])?; // Superblock is usually at block 0
    let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };

    if sb.magic != 0x137F && sb.magic != 0x138F {
        return Err("Invalid MINIX magic");
    }
    Ok(sb)
}

pub struct MinixFs {
    pub superblock: Superblock,
    pub device: &'static mut dyn BlockDevice,
}

impl MinixFs {
    pub fn allocate_zone(&mut self) -> Result<u32, &'static str> {
        let bits_per_block = self.superblock.block_size as usize * 8;
        let zmap_start = self.superblock.imap_blocks + 2;

        let mut buf = [0u8; 1024];
        for i in 0..self.superblock.zmap_blocks {
            self.device.read_block((zmap_start + i) as u64, &mut buf)?;

            for byte_idx in 0..buf.len() {
                if buf[byte_idx] != 0xFF {
                    for bit in 0..8 {
                        if buf[byte_idx] & (1 << bit) == 0 {
                            buf[byte_idx] |= 1 << bit;
                            self.device.write_block((zmap_start + i) as u64, &buf)?;

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
            let imap_block_lba = 1 + block_idx as u64;
            let mut buf = [0u8; 1024];
            self.device.read_block(imap_block_lba, &mut buf)?;

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
                            self.device.write_block(imap_block_lba, &buf)?;
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

        let zmap_start = 2 + self.superblock.imap_blocks as u64;
        let zmap_block = zmap_start + block_index as u64;

        let mut buf = [0u8; 1024];
        self.device.read_block(zmap_block, &mut buf)?;

        buf[byte_index] &= !(1 << bit);

        self.device.write_block(zmap_block, &buf)?;

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

        let imap_block_lba = 1 + block_index as u64;
        let mut buffer = [0u8; 1024];
        self.device.read_block(imap_block_lba, &mut buffer)?;

        buffer[byte_index] &= !(1 << bit_in_byte); // clear the bit

        self.device.write_block(imap_block_lba, &buffer)?;
        Ok(())
    }

}