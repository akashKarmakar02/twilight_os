use crate::driver;
use crate::driver::disk::AtaBlockDevice;
use crate::sys::fs::partition;
use crate::sys::fs::partition::PartitionEntry;
use crate::sys::fs::vfs::{BlockDev, FileSystem, FileType, Metadata, VfsNode, VfsNodeOps};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::{Mutex, RwLock};

const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_LFN: u8 = 0x0F;
const FAT16_EOC: u16 = 0xFFF8;

#[derive(Clone)]
struct FatDirEntry {
    name: String,
    cluster: u16,
    size: u32,
    attr: u8,
}

impl FatDirEntry {
    fn is_dir(&self) -> bool {
        (self.attr & ATTR_DIRECTORY) != 0
    }
}

pub struct Fat16Fs {
    inner: Arc<Mutex<Fat16Inner>>,
}

#[allow(dead_code)]
struct Fat16Inner {
    device: BlockDev,
    start_lba: u32,
    partition_sectors: u32,
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    num_fats: u8,
    sectors_per_fat: u16,
    root_entry_count: u16,
    root_dir_sectors: u32,
    fat_start_sector: u32,
    root_dir_sector: u32,
    first_data_sector: u32,
}

impl Fat16Fs {
    pub fn from_partition(bus: u8, dsk: u8, entry: PartitionEntry) -> Result<Self, &'static str> {
        let device = AtaBlockDevice::new(bus, dsk).ok_or("failed to open ATA device")?;
        let block: BlockDev = Arc::new(Mutex::new(Box::new(device)));
        let inner = Fat16Inner::new(block.clone(), entry.lba_start, entry.sectors)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }
}

impl Fat16Inner {
    fn new(device: BlockDev, start_lba: u32, sectors: u32) -> Result<Self, &'static str> {
        let mut bpb = [0u8; 512];
        {
            let mut dev = device.lock();
            dev.read(start_lba, &mut bpb)
                .map_err(|_| "failed to read FAT16 boot sector")?;
        }

        let bytes_per_sector = u16::from_le_bytes([bpb[11], bpb[12]]);
        if bytes_per_sector != 512 {
            return Err("unsupported FAT16 bytes/sector");
        }
        let sectors_per_cluster = bpb[13];
        if sectors_per_cluster == 0 {
            return Err("invalid sectors per cluster");
        }
        let reserved_sectors = u16::from_le_bytes([bpb[14], bpb[15]]);
        let num_fats = bpb[16];
        let root_entry_count = u16::from_le_bytes([bpb[17], bpb[18]]);
        let total_sectors_16 = u16::from_le_bytes([bpb[19], bpb[20]]);
        let sectors_per_fat = u16::from_le_bytes([bpb[22], bpb[23]]);
        let total_sectors_32 = u32::from_le_bytes([bpb[32], bpb[33], bpb[34], bpb[35]]);
        let total_sectors = if total_sectors_16 != 0 {
            total_sectors_16 as u32
        } else {
            total_sectors_32
        };

        if total_sectors == 0 {
            return Err("invalid FAT16 partition size");
        }

        let root_dir_sectors = ((root_entry_count as u32 * 32) + (bytes_per_sector as u32 - 1))
            / bytes_per_sector as u32;
        let fat_start_sector = reserved_sectors as u32;
        let root_dir_sector = fat_start_sector + (num_fats as u32 * sectors_per_fat as u32);
        let first_data_sector = root_dir_sector + root_dir_sectors;

        Ok(Self {
            device,
            start_lba,
            partition_sectors: sectors.min(total_sectors),
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            sectors_per_fat,
            root_entry_count,
            root_dir_sectors,
            fat_start_sector,
            root_dir_sector,
            first_data_sector,
        })
    }

    fn read_sector(&mut self, rel_sector: u32, buf: &mut [u8]) -> Result<(), ()> {
        if rel_sector >= self.partition_sectors {
            return Err(());
        }
        let abs = self.start_lba + rel_sector;
        self.device.lock().read(abs, buf)
    }

    fn cluster_to_sector(&self, cluster: u16) -> u32 {
        self.first_data_sector + (cluster as u32 - 2) * self.sectors_per_cluster as u32
    }

    fn read_cluster(&mut self, cluster: u16) -> Result<Vec<u8>, ()> {
        let mut out = vec![0u8; self.cluster_size_bytes()];
        let mut offset = 0;
        let first_sector = self.cluster_to_sector(cluster);
        for i in 0..self.sectors_per_cluster {
            let mut sector = [0u8; 512];
            self.read_sector(first_sector + i as u32, &mut sector)?;
            out[offset..offset + 512].copy_from_slice(&sector);
            offset += 512;
        }
        Ok(out)
    }

    fn cluster_size_bytes(&self) -> usize {
        self.bytes_per_sector as usize * self.sectors_per_cluster as usize
    }

    fn read_fat_entry(&mut self, cluster: u16) -> Result<u16, ()> {
        let fat_offset = (cluster as usize) * 2;
        let sector_offset = fat_offset / self.bytes_per_sector as usize;
        let byte_offset = fat_offset % self.bytes_per_sector as usize;
        let mut sector = [0u8; 512];
        self.read_sector(self.fat_start_sector + sector_offset as u32, &mut sector)?;
        Ok(u16::from_le_bytes([
            sector[byte_offset],
            sector[byte_offset + 1],
        ]))
    }

    fn read_directory(&mut self, cluster: Option<u16>) -> Result<Vec<FatDirEntry>, ()> {
        let mut raw = Vec::new();
        if let Some(cluster) = cluster.filter(|c| *c >= 2) {
            let mut current = cluster;
            loop {
                raw.extend_from_slice(&self.read_cluster(current)?);
                let next = self.read_fat_entry(current)?;
                if next >= FAT16_EOC {
                    break;
                }
                if next < 2 {
                    break;
                }
                current = next;
            }
        } else {
            for i in 0..self.root_dir_sectors {
                let mut sector = [0u8; 512];
                self.read_sector(self.root_dir_sector + i, &mut sector)?;
                raw.extend_from_slice(&sector);
            }
        }

        Ok(parse_directory_entries(&raw))
    }

    fn find_entry(&mut self, path: &str) -> Result<FatDirEntry, ()> {
        if path == "/" {
            return Ok(FatDirEntry {
                name: "/".into(),
                cluster: 0,
                size: 0,
                attr: ATTR_DIRECTORY,
            });
        }

        let parts: Vec<&str> = path.split('/').filter(|seg| !seg.is_empty()).collect();
        if parts.is_empty() {
            return Err(());
        }
        let mut current_cluster: Option<u16> = None;

        for (idx, component) in parts.iter().enumerate() {
            let entries = self.read_directory(current_cluster)?;
            let entry = entries
                .into_iter()
                .find(|item| item.name.eq_ignore_ascii_case(component))
                .ok_or(())?;
            if idx == parts.len() - 1 {
                return Ok(entry);
            }
            if entry.is_dir() {
                current_cluster = if entry.cluster == 0 {
                    None
                } else {
                    Some(entry.cluster)
                };
            } else {
                return Err(());
            }
        }

        Err(())
    }

    fn list_directory(&mut self, path: &str) -> Result<Vec<FatDirEntry>, ()> {
        if path == "/" {
            return self.read_directory(None);
        }
        let entry = self.find_entry(path)?;
        if !entry.is_dir() {
            return Err(());
        }
        if entry.cluster == 0 {
            self.read_directory(None)
        } else {
            self.read_directory(Some(entry.cluster))
        }
    }

    fn read_file(&mut self, entry: &FatDirEntry) -> Result<Vec<u8>, ()> {
        if entry.size == 0 {
            return Ok(Vec::new());
        }
        if entry.cluster < 2 {
            return Err(());
        }

        let mut remaining = entry.size as usize;
        let mut data = Vec::with_capacity(entry.size as usize);
        let mut current = entry.cluster;
        while current >= 2 && remaining > 0 {
            let mut cluster_data = self.read_cluster(current)?;
            if cluster_data.len() > remaining {
                cluster_data.truncate(remaining);
            }
            data.extend_from_slice(&cluster_data);
            remaining = entry.size as usize - data.len();
            if remaining == 0 {
                break;
            }
            let next = self.read_fat_entry(current)?;
            if next >= FAT16_EOC {
                break;
            }
            if next < 2 {
                break;
            }
            current = next;
        }
        data.truncate(entry.size as usize);
        Ok(data)
    }
}

fn parse_directory_entries(buf: &[u8]) -> Vec<FatDirEntry> {
    let mut entries = Vec::new();
    let mut lfn_parts: Vec<String> = Vec::new();
    for chunk in buf.chunks(32) {
        if chunk[0] == 0x00 {
            break;
        }
        if chunk[0] == 0xE5 {
            lfn_parts.clear();
            continue;
        }
        let attr = chunk[11];
        if attr == ATTR_LFN {
            lfn_parts.push(decode_lfn_part(chunk));
            continue;
        }
        if (attr & ATTR_VOLUME_ID) != 0 {
            lfn_parts.clear();
            continue;
        }

        let name = if !lfn_parts.is_empty() {
            let mut assembled = String::new();
            for part in lfn_parts.iter().rev() {
                assembled.push_str(part);
            }
            lfn_parts.clear();
            assembled.trim_matches('\u{0000}').to_string()
        } else {
            decode_short_name(chunk)
        };
        let cluster = u16::from_le_bytes([chunk[26], chunk[27]]);
        let size = u32::from_le_bytes([chunk[28], chunk[29], chunk[30], chunk[31]]);
        entries.push(FatDirEntry {
            name,
            cluster,
            size,
            attr,
        });
    }
    entries
}

fn decode_lfn_part(entry: &[u8]) -> String {
    let mut chars = Vec::new();
    let positions = [(1, 5), (14, 6), (28, 2)];
    for (start, count) in positions {
        for idx in 0..count {
            let lo = entry[start + idx * 2];
            let hi = entry[start + idx * 2 + 1];
            let val = u16::from_le_bytes([lo, hi]);
            if val == 0xFFFF || val == 0x0000 {
                continue;
            }
            if let Some(ch) = char::from_u32(val as u32) {
                chars.push(ch);
            }
        }
    }
    chars.iter().collect()
}

fn decode_short_name(entry: &[u8]) -> String {
    let base = trim_ascii(&entry[0..8]);
    let ext = trim_ascii(&entry[8..11]);
    if ext.is_empty() {
        base
    } else if base.is_empty() {
        ext
    } else {
        format!("{}.{}", base, ext)
    }
}

fn trim_ascii(bytes: &[u8]) -> String {
    let mut out = String::new();
    for &b in bytes {
        if b == b' ' || b == 0 {
            continue;
        }
        out.push(b as char);
    }
    out
}

impl FileSystem for Fat16Fs {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()> {
        let entry = self.inner.lock().find_entry(path)?;
        let metadata = Metadata {
            file_type: if entry.is_dir() {
                FileType::Dir
            } else {
                FileType::File
            },
            size: entry.size as usize,
            name: entry.name.clone(),
            ino: 0,
            access_time: 0,
            created_time: 0,
            modified_time: 0,
        };
        let node = Fat16Node {
            entry,
            inner: self.inner.clone(),
        };
        Ok(VfsNode::new(
            self.inner.lock().device.clone(),
            metadata,
            Arc::new(RwLock::new(node)),
        ))
    }

    fn mkdir(&mut self, _parent_dir: &str, _path: &str) -> Result<(), ()> {
        Err(())
    }

    fn rmdir(&mut self, _path: &str) -> Result<(), ()> {
        Err(())
    }

    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()> {
        let entries = self.inner.lock().list_directory(path)?;
        Ok(entries
            .into_iter()
            .map(|entry| Metadata {
                file_type: if entry.is_dir() {
                    FileType::Dir
                } else {
                    FileType::File
                },
                size: entry.size as usize,
                name: entry.name,
                ino: 0,
                access_time: 0,
                created_time: 0,
                modified_time: 0,
            })
            .collect())
    }

    fn rm(&mut self, _path: &str) -> Result<(), ()> {
        Err(())
    }

    fn touch(&mut self, _parent_path: &str, _filename: &str) -> Result<(), ()> {
        Err(())
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        let entry = self.inner.lock().find_entry(path)?;
        Ok(Metadata {
            file_type: if entry.is_dir() {
                FileType::Dir
            } else {
                FileType::File
            },
            size: entry.size as usize,
            name: entry.name,
            ino: 0,
            access_time: 0,
            created_time: 0,
            modified_time: 0,
        })
    }
}

struct Fat16Node {
    entry: FatDirEntry,
    inner: Arc<Mutex<Fat16Inner>>,
}

impl VfsNodeOps for Fat16Node {
    fn read(&self, _device: &mut BlockDev, lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        if self.entry.is_dir() {
            return Err(());
        }
        let content = self.inner.lock().read_file(&self.entry)?;

        buf.copy_from_slice(&content[lba..(lba + buf.len())]);

        Ok(buf.len())
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(false)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Err(())
    }
}

fn is_fat16_type(part_type: u8) -> bool {
    matches!(
        part_type,
        partition::FAT16_CHS_PARTITION_TYPE | partition::FAT16_LBA_PARTITION_TYPE
    )
}

pub fn detect_fat16_partition(bus: u8, dsk: u8) -> Option<PartitionEntry> {
    let mut sector = [0u8; 512];
    if driver::disk::ata::read(bus, dsk, 0, &mut sector).is_err() {
        return None;
    }
    if !partition::has_signature(&sector) {
        return None;
    }
    let entries = partition::decode_entries(&sector);
    entries.into_iter().find(|entry| {
        entry.is_present()
            && is_fat16_type(entry.partition_type)
            && validate_fat16_boot_sector(bus, dsk, entry)
    })
}

fn validate_fat16_boot_sector(bus: u8, dsk: u8, entry: &PartitionEntry) -> bool {
    if entry.lba_start == 0 {
        return false;
    }
    let mut sector = [0u8; 512];
    if driver::disk::ata::read(bus, dsk, entry.lba_start, &mut sector).is_err() {
        return false;
    }
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return false;
    }
    let bytes_per_sector = u16::from_le_bytes([sector[11], sector[12]]);
    let sectors_per_cluster = sector[13];
    let reserved_sectors = u16::from_le_bytes([sector[14], sector[15]]);
    let num_fats = sector[16];
    let root_entries = u16::from_le_bytes([sector[17], sector[18]]);
    let total_16 = u16::from_le_bytes([sector[19], sector[20]]);
    let total_32 = u32::from_le_bytes([sector[32], sector[33], sector[34], sector[35]]);
    let sectors_per_fat = u16::from_le_bytes([sector[22], sector[23]]);

    if bytes_per_sector == 0
        || sectors_per_cluster == 0
        || reserved_sectors == 0
        || num_fats == 0
        || sectors_per_fat == 0
        || root_entries == 0
    {
        return false;
    }

    let total_sectors = if total_16 != 0 {
        total_16 as u32
    } else {
        total_32
    };
    if total_sectors == 0 {
        return false;
    }

    true
}
