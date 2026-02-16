pub const JOURNAL_MAGIC: u32 = 0x4C4E_524A; // "JRNL" little-endian

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JournalRecType {
    Begin   = 1,
    Write   = 2, // write whole block
    Commit  = 3,
    Abort   = 4,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct JournalRecordHeader {
    pub magic: u32,
    pub rec_type: u8,
    pub csum_type: u8,
    pub _pad0: u16,
    pub seq: u64,
    pub len: u32,
    pub checksum: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct JournalWritePayload {
    pub block_index: u64,
    pub block_len: u32,
    pub _pad0: u32,
}