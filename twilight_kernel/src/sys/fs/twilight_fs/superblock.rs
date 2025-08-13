use crate::driver::disk::{BlockDevice, BlockDeviceIO};
use crate::driver::timer::cmos::CMOS;
use crate::sys::fs::KERNEL_PADDING;

pub const MAGIC: [u8; 4] = [b'T', b'F', b'S', b'0'];
pub const VERSION: u32 = 0x000001;

#[allow(dead_code)]
#[repr(C, packed)]
pub struct Superblock {
    magic: [u8; 4],
    version: u32,
    uuid: [u8; 16],
    block_size: u32,
    total_blocks: u64,
    free_blocks: u64,
    alloc_bitmap_start: u64,
    metadata_root: u64,
    superblock_checksum: u32,
    checksum_algo: u8,
    created_at: u64,
    label: [u8; 32],
    reserved: [u8; 256],
}

#[allow(dead_code)]
impl Superblock {
    pub(crate) fn new(label: [u8; 32], block_size: u32, total_blocks: u64, alloc_bitmap_start: u64) -> Self {
        let mut cmos = CMOS::new();
        let time = cmos.unix_time();

        let mut sb = Self {
            magic: MAGIC,
            version: VERSION,
            created_at: time,
            block_size,
            label,
            free_blocks: 0,
            total_blocks,
            alloc_bitmap_start,
            metadata_root: 0,
            checksum_algo: 0,
            superblock_checksum: 0,
            uuid: [0; 16],
            reserved: [0; 256]
        };

        sb.superblock_checksum = sb.calculate_checksum();

        sb
    }

    fn write_to_disk(&self, disk: &'static mut BlockDevice, is_boot: bool) {
        let buf = unsafe { core::slice::from_raw_parts(self as *const _ as *const u8, size_of::<Self>()) };
        disk.write(if is_boot { KERNEL_PADDING as u32 }  else { 0 }, buf).unwrap();
    }

    fn calculate_checksum(&self) -> u32 {
        // todo: writing a checksum algo for Superblock::superblock_checksum value
        0
    }
}