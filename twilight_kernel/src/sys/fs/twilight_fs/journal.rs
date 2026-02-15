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
    pub magic: u32,     // JOURNAL_MAGIC
    pub rec_type: u8,   // JournalRecType
    pub csum_type: u8,  // CsumType
    pub _pad0: u16,
    pub seq: u64,       // transaction sequence
    pub len: u32,       // bytes including header+payload
    pub checksum: u32,  // checksum over header+payload excluding this field (or including with 0)
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