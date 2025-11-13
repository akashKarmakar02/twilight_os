#[allow(dead_code)]
#[repr(C, packed)]
pub struct BlockGroupHeader {
    pub group_id: u32,
    pub block_start: u64,
    pub block_count: u64,

    pub free_blocks_count: u64,
    pub free_inodes_count: u64,

    pub block_bitmap_block: u64,
    pub inode_bitmap_block: u64,
    pub inode_table_block: u64,

    pub metadata_tree_block: u64,

    pub checksum: u32,
}
