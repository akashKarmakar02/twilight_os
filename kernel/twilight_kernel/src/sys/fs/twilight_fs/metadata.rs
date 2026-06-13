const N: usize = 31;

#[allow(dead_code)]
#[repr(u8)]
enum MetadataBlockKind {
    Inode = 0,
    DirEntry = 1,
    IndexNode = 2,
    Free = 3,
}

#[repr(C)]
struct MetadataBlockHeader {
    kind: MetadataBlockKind,
    _pad1: [u8; 7], // alignment padding
    generation: u64,
    checksum: u32,
    _pad2: [u8; 4], // align to 8 bytes
}

#[repr(C)]
pub struct MetadataBlock {
    header: MetadataBlockHeader,
    data: [u8; 512 - size_of::<MetadataBlockHeader>()],
}

#[repr(C)]
pub struct TreeNode {
    level: u8,
    keys: [u64; N],
    children: [u64; N + 1],
}
