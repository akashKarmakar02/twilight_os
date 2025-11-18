use core::convert::TryInto;

pub const SECTOR_SIZE: u32 = 512;
pub const MBR_SIGNATURE: [u8; 2] = [0x55, 0xAA];
pub const PARTITION_TABLE_OFFSET: usize = 446;
pub const PARTITION_ENTRY_SIZE: usize = 16;

pub const FAT32_LBA_PARTITION_TYPE: u8 = 0x0C;
pub const FAT16_CHS_PARTITION_TYPE: u8 = 0x06;
pub const FAT16_LBA_PARTITION_TYPE: u8 = 0x0E;
pub const TWILIGHT_PARTITION_TYPE: u8 = 0x99;

const LBA_CHS_PLACEHOLDER: [u8; 3] = [0xFE, 0xFF, 0xFF];

#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PartitionEntry {
    pub status: u8,
    pub chs_start: [u8; 3],
    pub partition_type: u8,
    pub chs_end: [u8; 3],
    pub lba_start: u32,
    pub sectors: u32,
}

impl PartitionEntry {
    pub const fn new(status: u8, partition_type: u8, lba_start: u32, sectors: u32) -> Self {
        Self {
            status,
            chs_start: LBA_CHS_PLACEHOLDER,
            partition_type,
            chs_end: LBA_CHS_PLACEHOLDER,
            lba_start,
            sectors,
        }
    }

    pub const fn empty() -> Self {
        Self {
            status: 0,
            chs_start: [0; 3],
            partition_type: 0,
            chs_end: [0; 3],
            lba_start: 0,
            sectors: 0,
        }
    }

    pub const fn is_present(&self) -> bool {
        self.partition_type != 0 && self.sectors != 0
    }
}

pub fn has_signature(mbr: &[u8; 512]) -> bool {
    mbr[510] == MBR_SIGNATURE[0] && mbr[511] == MBR_SIGNATURE[1]
}

pub fn write_signature(mbr: &mut [u8; 512]) {
    mbr[510] = MBR_SIGNATURE[0];
    mbr[511] = MBR_SIGNATURE[1];
}

pub fn decode_entries(mbr: &[u8; 512]) -> [PartitionEntry; 4] {
    let mut entries = [PartitionEntry::empty(); 4];

    for (index, entry) in entries.iter_mut().enumerate() {
        let base = PARTITION_TABLE_OFFSET + index * PARTITION_ENTRY_SIZE;
        entry.status = mbr[base];
        entry.chs_start.copy_from_slice(&mbr[base + 1..base + 4]);
        entry.partition_type = mbr[base + 4];
        entry.chs_end.copy_from_slice(&mbr[base + 5..base + 8]);
        entry.lba_start = u32::from_le_bytes(mbr[base + 8..base + 12].try_into().unwrap());
        entry.sectors = u32::from_le_bytes(mbr[base + 12..base + 16].try_into().unwrap());
    }

    entries
}

pub fn encode_entries(mbr: &mut [u8; 512], entries: &[PartitionEntry; 4]) {
    for (index, entry) in entries.iter().enumerate() {
        let base = PARTITION_TABLE_OFFSET + index * PARTITION_ENTRY_SIZE;

        mbr[base] = entry.status;
        mbr[base + 1..base + 4].copy_from_slice(&entry.chs_start);
        mbr[base + 4] = entry.partition_type;
        mbr[base + 5..base + 8].copy_from_slice(&entry.chs_end);
        mbr[base + 8..base + 12].copy_from_slice(&entry.lba_start.to_le_bytes());
        mbr[base + 12..base + 16].copy_from_slice(&entry.sectors.to_le_bytes());
    }
}

pub fn find_entry(entries: &[PartitionEntry; 4], partition_type: u8) -> Option<PartitionEntry> {
    entries
        .iter()
        .copied()
        .find(|entry| entry.partition_type == partition_type && entry.is_present())
}
