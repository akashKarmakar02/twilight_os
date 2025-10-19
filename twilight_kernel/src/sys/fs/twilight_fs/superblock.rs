use crate::driver::disk::BlockDeviceIO;
use crate::println;
use crate::sys::fs::twilight_fs::inode::Inode;
use crate::sys::fs::twilight_fs::{read_tfs_block, write_tfs_block, FS_BLOCK_SIZE};

pub const MAGIC: [u8; 4] = [b'T', b'F', b'S', b'0'];
pub const VERSION: u32 = 0x000001;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub ninodes: u32,
    pub pad1: u16,
    pub imap_blocks: u16,
    pub zmap_blocks: u16,
    pub first_data_zone: u16,
    pub log_zone_size: u16,
    pub pad2: u16,
    pub max_size: u32,
    pub zones: u32,
    pub magic: u32,
    pub pad3: u16,
    pub block_size: u16,
    pub subversion: u8,
}

impl Superblock {
    pub fn read(device: &mut dyn BlockDeviceIO) -> Result<(), &'static str> {
        let mut buf = [0u8; FS_BLOCK_SIZE];
        if read_tfs_block(device, 0, &mut buf).is_err() {
            return Err("ERROR: read failed while reading superblock");
        }
        let sb: Superblock = unsafe { core::ptr::read(buf.as_ptr() as *const _) };

        println!("{:?}", sb);

        Ok(())
    }

    pub fn write(device: &mut dyn BlockDeviceIO) -> Result<Self, &'static str> {
        let block_size = FS_BLOCK_SIZE as u64;          // 2048
        let bits_per_block = block_size * 8;            // 16384
        let inode_size = size_of::<Inode>() as u64;     // typically 64
        let log_zone_size = 0u16;                       // zone == block
        let blocks_per_zone = 1u64 << log_zone_size;    // = 1
        let reserved_blocks = 1u64;                     // super at block 0

        // ---- device geometry (sector-level) ----
        let dev_sector_size = device.block_size() as u64;       // 512
        let dev_sectors     = device.block_count() as u64;      // in 512B units
        let sectors_per_fs_block = block_size / dev_sector_size; // 2048/512 = 4

        // total FS blocks & zones on the device
        let total_blocks = dev_sectors / sectors_per_fs_block;   // floor
        let total_zones  = total_blocks / blocks_per_zone;       // = total_blocks

        // choose ninodes (here: 1 inode per 16 KiB of disk, like you had)
        let total_bytes = dev_sectors * dev_sector_size;
        let bpi = 16 * 1024u64;
        let ninodes = (total_bytes / bpi).max(1);

        // bitmaps & inode table sizes (in FS blocks)
        let imap_blocks  = div_ceil(ninodes, bits_per_block);
        let inode_blocks = div_ceil(ninodes * inode_size, block_size);

        // small fixed-point iteration to resolve zmap <-> first_data_zone
        let mut zmap_blocks = 0u64;
        let mut first_data_zone = 0u64;
        for _ in 0..4 {
            let first_data_block = reserved_blocks + imap_blocks + zmap_blocks + inode_blocks;
            let new_first_data_zone = div_ceil(first_data_block, blocks_per_zone); // == first_data_block

            let data_zones = total_zones.saturating_sub(new_first_data_zone);
            let new_zmap_blocks = div_ceil(data_zones, bits_per_block);

            if new_first_data_zone == first_data_zone && new_zmap_blocks == zmap_blocks {
                break;
            }
            first_data_zone = new_first_data_zone;
            zmap_blocks = new_zmap_blocks;
        }

        let sb = Superblock {
            ninodes: ninodes as u32,
            pad1: 0,
            imap_blocks: imap_blocks as u16,
            zmap_blocks: zmap_blocks as u16,
            first_data_zone: first_data_zone as u16,
            log_zone_size,
            pad2: 0,
            max_size: 0x7FFF_FFFF,            // keep if that’s your limit
            zones: total_zones as u32,        // <-- TOTAL zones, not data_zones
            magic: u32::from_le_bytes(MAGIC), // 'T','F','S','0'
            pad3: 0,
            block_size: FS_BLOCK_SIZE as u16, // 2048
            subversion: 0,
        };

        // serialize & write the superblock at FS block 0
        let mut buffer = [0u8; FS_BLOCK_SIZE];
        let sb_bytes = unsafe {
            core::slice::from_raw_parts(&sb as *const _ as *const u8, size_of::<Superblock>())
        };
        buffer[..sb_bytes.len()].copy_from_slice(sb_bytes);

        write_tfs_block(device, 0, &buffer).map_err(|_| "ERROR: write failed while writing superblock")?;
        Ok(sb)
    }
    
    pub fn is_valid(&self) -> bool {
        self.magic == u32::from_le_bytes(MAGIC) && self.subversion == 0
    }
}

fn div_ceil(u: u64, d: u64) -> u64 { (u + d - 1) / d }