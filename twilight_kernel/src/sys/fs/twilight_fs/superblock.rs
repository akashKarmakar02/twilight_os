// WARNING: some code in this file is AI generated
use crate::driver::disk::BlockDeviceIO;
use crate::println;
use crate::sys::fs::partition;
use crate::sys::fs::twilight_fs::inode::Inode;
use crate::sys::fs::twilight_fs::{FS_BLOCK_SIZE, read_tfs_block, write_tfs_block};

pub const MAGIC: [u8; 4] = [b'T', b'F', b'S', b'0'];
pub const VERSION: u32 = 0x000001;

pub type FeatureBits = u64;

// Safe to mount RW if unknown bits are set.
pub const FEAT_COMPAT_NONE: FeatureBits = 0;

// Must mount RO if unknown bits are set.
pub const FEAT_RO_COMPAT_CSUM_METADATA: FeatureBits = 1 << 0;
pub const FEAT_RO_COMPAT_DIR_INDEX: FeatureBits = 1 << 1;

// Must refuse mount if unknown bits are set.
pub const FEAT_INCOMPAT_EXTENTS: FeatureBits = 1 << 0;
pub const FEAT_INCOMPAT_XATTRS: FeatureBits = 1 << 1;
pub const FEAT_INCOMPAT_JOURNAL: FeatureBits = 1 << 2;
pub const FEAT_INCOMPAT_ENCRYPT: FeatureBits = 1 << 3;
pub const FEAT_INCOMPAT_DIR_V2: FeatureBits = 1 << 4;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Superblock {
    pub ninodes: u32,
    pub pad1: u16,
    pub imap_blocks: u32,
    pub zmap_blocks: u32,
    pub first_data_zone: u32,
    pub log_zone_size: u16,
    pub pad2: u16,
    pub max_size: u32,
    pub zones: u32,
    pub magic: u32,
    pub pad3: u16,
    pub block_size: u16,
    pub subversion: u8,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsumType {
    None = 0,
    Crc32c = 1,
    Blake3 = 2, // future
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoType {
    None = 0,
    XChaCha20Poly1305 = 1,
    Aes256Gcm = 2,         // future/hw accel
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Uuid128 {
    pub bytes: [u8; 16],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SuperblockV1 {
    // identity
    pub magic: [u8; 4],     // "TFS0"
    pub version: u32,       // 0x000001
    pub block_size: u16
    pub log_block_size: u8, // log2(block_size) convenience
    pub csum_type: u8,      // CsumType
    pub uuid: Uuid128,      // unique FS id
    pub generation: u64,    // increments every successful commit

    // compatibility flags
    pub compat_features: FeatureBits,
    pub ro_compat_features: FeatureBits,
    pub incompat_features: FeatureBits,

    pub ninodes: u32,
    pub imap_start: u32,  // block index
    pub imap_blocks: u32, // bitmap blocks
    pub zmap_start: u32,  // block index
    pub zmap_blocks: u32, // bitmap blocks
    pub first_data_block: u32,
    pub total_blocks: u64,  // replaces `zones` (supports >4B blocks)
    pub max_file_size: u64, // computed from mapping scheme, stored for sanity

    pub root_inode: u32,    // usually 1
    pub journal_inode: u32, // or 0 if using fixed journal region
    pub reserved_inodes: [u32; 6],

    pub journal_start: u64, // block index
    pub journal_blocks: u32,
    pub journal_seq: u64, // last committed tx sequence

    pub crypto_type: u8,  // CryptoType
    pub crypto_flags: u8, // e.g. data-only, metadata later
    pub _pad0: u16,
    pub key_derivation_salt: [u8; 16], // stored salt; master key comes from outside

    pub sb_checksum: u32, // CRC32C over bytes [0..offset(sb_checksum))
    pub _pad1: u32,
}

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

pub const MODE_TYPE_MASK: u16 = 0xF000; // you can define your own layout
pub const MODE_PERM_MASK: u16 = 0x01FF; // rwxrwxrwx

pub type InodeFlags = u32;
pub const IFLAG_IMMUTABLE: InodeFlags = 1 << 0;
pub const IFLAG_APPEND: InodeFlags = 1 << 1;
pub const IFLAG_ENCRYPTED: InodeFlags = 1 << 2;
pub const IFLAG_INLINE_DATA: InodeFlags = 1 << 3; // inline symlink/small file
pub const IFLAG_DIR_INDEXED: InodeFlags = 1 << 4; // has dir hash index block
pub const IFLAG_HAS_XATTR: InodeFlags = 1 << 5;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Extent32 {
    pub start_block: u32, // physical start block
    pub block_len: u32,   // length in blocks
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Extent64 {
    pub start_block: u64,
    pub block_len: u32,
    pub _pad: u32,
}

pub const INODE_INLINE_BYTES: usize = 64;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct InodeV1 {
    pub mode: u16, // includes type bits + permission bits
    pub nlinks: u16,
    pub uid: u32,
    pub gid: u32,

    pub size: u64,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub crtime: u64, // creation time (nice to have)

    // inode metadata
    pub flags: InodeFlags,
    pub generation: u64, // bump when inode changes (useful for key derivation/consistency)

    // xattrs
    pub xattr_block: u32, // 0 = none
    pub _pad0: u32,

    pub direct: [Extent32; 6], // a few direct extents
    pub indirect: u32,         // block containing Extent32[]
    pub double_indirect: u32,  // block containing u32[] -> indirect blocks
    pub triple_indirect: u32,

    // inline payload area (used only when IFLAG_INLINE_DATA set)
    pub inline_data: [u8; INODE_INLINE_BYTES],

    // inode checksum (optional; if metadata csum feature enabled)
    pub inode_checksum: u32,
    pub _pad1: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct XattrBlockHeader {
    pub magic: u32, // e.g. "XATR"
    pub used_bytes: u16,
    pub count: u16,
    pub checksum: u32, // checksum over header+payload (excluding this field if you want)
                       // followed by TLVs
                       // [key_len:u16][val_len:u16][key_bytes][val_bytes]...
}

pub const XATTR_MAGIC: u32 = 0x5254_4158; // "XATR" little-endian

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntryV1 {
    pub inode: u32,
    pub name: [u8; 60],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntryV2Header {
    pub inode: u32,
    pub rec_len: u16, // total record length
    pub name_len: u8,
    pub file_type: u8, // FileType as u8, optional hint
                       // followed by name[name_len], then padding to rec_len
}

// Optional directory hash index block (for fast lookups; built on demand).
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirIndexEntry {
    pub name_hash: u32, // e.g. fnv1a32 / xxhash32 (pick one)
    pub inode: u32,
    pub dirent_offset: u32, // offset within dir data (bytes) or entry number
    pub _pad: u32,
}

pub const JOURNAL_MAGIC: u32 = 0x4C4E_524A; // "JRNL" little-endian

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalRecType {
    Begin = 1,
    Write = 2, // write whole block
    Commit = 3,
    Abort = 4,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct JournalRecordHeader {
    pub magic: u32,    // JOURNAL_MAGIC
    pub rec_type: u8,  // JournalRecType
    pub csum_type: u8, // CsumType
    pub _pad0: u16,
    pub seq: u64,      // transaction sequence
    pub len: u32,      // bytes including header+payload
    pub checksum: u32, // checksum over header+payload excluding this field (or including with 0)
}

// Payload for Write record (simple “block image” redo log)
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct JournalWritePayload {
    pub block_index: u64, // which block to overwrite
    pub block_len: u32,   // bytes (normally block_size)
    pub _pad0: u32,
    // followed by block bytes[block_len]
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MetaBlockHeader {
    pub csum_type: u8,
    pub _pad0: [u8; 3],
    pub checksum: u32, // checksum for the rest of the block after this header
                       // followed by actual metadata payload
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

    pub fn write(
        device: &mut dyn BlockDeviceIO,
        partition_sector_count: u32,
    ) -> Result<Self, &'static str> {
        let block_size = FS_BLOCK_SIZE as u64; // 2048
        let bits_per_block = block_size * 8; // 16384
        let inode_size = size_of::<Inode>() as u64; // typically 64
        let log_zone_size = 0u16; // zone == block
        let blocks_per_zone = 1u64 << log_zone_size; // = 1
        let reserved_blocks = 1u64; // super at block 0

        // ---- device geometry (sector-level/IO Level) ----
        let dev_sector_size = device.block_size() as u64; // 512
        debug_assert_eq!(dev_sector_size, partition::SECTOR_SIZE as u64);
        let dev_sectors = partition_sector_count as u64; // limited to Twilight partition
        let sectors_per_fs_block = block_size / dev_sector_size; // 2048/512 = 4

        // total FS blocks & zones on the device
        let total_blocks = dev_sectors / sectors_per_fs_block; // floor
        let total_zones = total_blocks / blocks_per_zone; // = total_blocks

        // choose ninodes (here: 1 inode per 16 KiB of disk, like you had)
        let total_bytes = dev_sectors * dev_sector_size;
        let bpi = 16 * 1024u64;
        let ninodes = (total_bytes / bpi).max(1);

        // bitmaps & inode table sizes (in FS blocks)
        let imap_blocks = div_ceil(ninodes, bits_per_block);
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
            imap_blocks: imap_blocks as u32,
            zmap_blocks: zmap_blocks as u32,
            first_data_zone: first_data_zone as u32,
            log_zone_size,
            pad2: 0,
            max_size: 0x7FFF_FFFF, // mock limit don't know what i am going to do
            zones: total_zones as u32, // <-- TOTAL zones
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

        write_tfs_block(device, 0, &buffer)
            .map_err(|_| "ERROR: write failed while writing superblock")?;
        Ok(sb)
    }

    pub fn is_valid(&self) -> bool {
        self.magic == u32::from_le_bytes(MAGIC) && self.subversion == 0
    }
}

fn div_ceil(u: u64, d: u64) -> u64 {
    (u + d - 1) / d
}
