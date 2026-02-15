use crate::driver::disk::BlockDeviceIO;

const MBR_SIGNATURE: [u8; 2] = [0x55, 0xAA];
const LBA_CHS_PLACEHOLDER: [u8; 3] = [0xFE, 0xFF, 0xFF];

pub const PARTITION_TABLE_OFFSET: usize = 446;
pub const PARTITION_ENTRY_SIZE: usize = 16;

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

pub struct Mbr<'a> {
    buf: [u8; 512],
    dev: &'a mut dyn BlockDeviceIO
}

impl<'a> Mbr<'a> {
    pub fn new(buf: [u8; 512], dev: &'a mut (dyn BlockDeviceIO + 'static)) -> Option<Self> {
        if buf[510] == MBR_SIGNATURE[0] && buf[511] == MBR_SIGNATURE[1] {
            Some(Self { buf, dev })
        } else {
            None
        }
    }

    pub fn create_new(_buf: [u8; 512], dev: &'a mut (dyn BlockDeviceIO + 'static)) -> Self {
        let mut fresh = [0u8; 512];
        fresh[510] = MBR_SIGNATURE[0];
        fresh[511] = MBR_SIGNATURE[1];
        Self { buf: fresh, dev }
    }

    pub fn get_entries(&self) -> [PartitionEntry; 4] {
        let mut entries = [PartitionEntry::empty(); 4];

        for (index, entry) in entries.iter_mut().enumerate() {
            let base = PARTITION_TABLE_OFFSET + index * PARTITION_ENTRY_SIZE;
            entry.status = self.buf[base];
            entry.chs_start.copy_from_slice(&self.buf[base + 1..base + 4]);
            entry.partition_type = self.buf[base + 4];
            entry.chs_end.copy_from_slice(&self.buf[base + 5..base + 8]);
            entry.lba_start = u32::from_le_bytes(self.buf[base + 8..base + 12].try_into().unwrap());
            entry.sectors = u32::from_le_bytes(self.buf[base + 12..base + 16].try_into().unwrap());
        }

        entries
    }

    pub fn write_entries(&mut self, entries: &[PartitionEntry]) -> Result<(), ()> {
        self.encode_entries(entries);
        self.dev.write(0, self.buf.as_slice())
    }
    
    fn encode_entries(&mut self, entries: &[PartitionEntry]) {
        for (index, entry) in entries.iter().enumerate() {
            let base = PARTITION_TABLE_OFFSET + index * PARTITION_ENTRY_SIZE;
            
            self.buf[base] = entry.status;
            self.buf[base + 1..base + 4].copy_from_slice(&entry.chs_start);
            self.buf[base + 4] = entry.partition_type;
            self.buf[base + 5..base + 8].copy_from_slice(&entry.chs_end);
            self.buf[base + 8..base + 12].copy_from_slice(&entry.lba_start.to_le_bytes());
            self.buf[base + 12..base + 16].copy_from_slice(&entry.sectors.to_le_bytes());
        }
    }
}
