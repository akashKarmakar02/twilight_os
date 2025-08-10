#[repr(C, packed)]
#[allow(dead_code)]
pub struct Inode {
    pub id: u32,
    pub mode: u16,
    pub uid: u16,
    pub gid: u16,
    pub size: u32,
    pub access_time: u32,
    pub modification_time: u32,
    pub created_time: u32,
    pub blocks: u32,
    pub direct: [u32; 12],
    pub indirect: u32,
    pub double_indirect: u32,
    pub triple_indirect: u32,
    pub padding: u16,
}