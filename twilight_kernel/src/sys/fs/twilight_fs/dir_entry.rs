#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct DirEntry {
    pub inode: u32,
    pub name: [u8; 60], // MINIX v2 uses fixed 60-byte names
}
