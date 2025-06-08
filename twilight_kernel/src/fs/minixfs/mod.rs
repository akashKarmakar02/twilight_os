use crate::driver::disk::BlockDevice;

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


fn calculate_total_zones(disk_size: usize, block_size: usize, ninodes: usize) -> usize {
    let total_blocks = disk_size / block_size;
    let inode_size = 32;

    let imap_blocks = (ninodes + (block_size * 8 - 1)) / (block_size * 8);
    let zmap_blocks = (total_blocks + (block_size * 8 - 1)) / (block_size * 8);
    let inode_blocks = (ninodes * inode_size + (block_size - 1)) / block_size;

    let reserved = 1 + 1 + imap_blocks + zmap_blocks + inode_blocks; // boot + superblock + maps + inodes
    total_blocks - reserved
}

pub fn format_superblock(
    block_device: &mut dyn BlockDevice,
    disk_size: usize,
    ninodes: u16,
    block_size: u16,
) -> Result<(), &'static str> {
    let imap_blocks = 1; // could be calculated dynamically
    let zmap_blocks = 1; // same as above
    let first_data_zone = 2 + imap_blocks + zmap_blocks + (ninodes as usize * 32 / block_size as usize) as u16;
    let total_zones = calculate_total_zones(disk_size, block_size as usize, ninodes as usize);

    let superblock = Superblock {
        ninodes,
        pad1: 0,
        imap_blocks,
        zmap_blocks,
        first_data_zone,
        log_zone_size: 0,
        pad2: 0,
        max_size: 0x7FFF_FFFF, // Max file size
        zones: total_zones as u32,
        magic: 0x138F, // MINIX v2 magic number
        pad3: 0,
        block_size,
        subversion: 0,
    };
    
    let mut buffer = [0u8; 1024]; // assuming 1K blocks for simplicity
    let sb_bytes = unsafe {
        core::slice::from_raw_parts(
            &superblock as *const _ as *const u8,
            size_of::<Superblock>(),
        )
    };
    buffer[..sb_bytes.len()].copy_from_slice(sb_bytes);

    block_device.write_block(0, &buffer).expect("minix fs formatting failed"); // Superblock goes to block 1
    Ok(())
}

pub fn read_superblock(device: &mut dyn BlockDevice) -> Result<Superblock, &'static str> {
    let mut buf = [0u8; 1024]; // Assuming block size is 1KiB
    device.read_block(0, &mut buf[0..1024])?; // Superblock is usually at block 1
    let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };

    if sb.magic != 0x137F && sb.magic != 0x138F {
        return Err("Invalid MINIX magic");
    }
    Ok(sb)
}
