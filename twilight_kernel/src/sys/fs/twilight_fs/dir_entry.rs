#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub inode: u32,
    pub name: [u8; 60], // MINIX v2 uses fixed 60-byte names
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirIndexEntry {
    pub name_hash: u32,     // e.g. fnv1a32 / xxhash32 (pick one)
    pub inode: u32,
    pub dirent_offset: u32, // offset within dir data (bytes) or entry number
    pub _pad: u32,
}
