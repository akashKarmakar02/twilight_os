#[allow(dead_code)]
#[repr(C, packed)]
pub struct DirEntry {
    pub inode_id: u32,
    pub name_len: u8,   // Name length
    pub file_type: u8,  // File or directory
    pub name: [u8; 28],
}
