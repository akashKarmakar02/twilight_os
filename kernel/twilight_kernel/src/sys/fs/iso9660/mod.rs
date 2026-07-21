use crate::sys::fs::vfs::{BlockDev, FileSystem, FileType, FsStats, Metadata, VfsNode, VfsNodeOps};
use crate::utils::sync::{Mutex, RwLock};
use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

const ISO_BLOCK_SIZE: usize = 2048;
const PVD_LBA: usize = 16;
const VOLUME_DESCRIPTOR_ID: &[u8; 5] = b"CD001";
const FLAG_DIRECTORY: u8 = 0x02;
const FLAG_MULTI_EXTENT: u8 = 0x80;

#[derive(Clone, Debug)]
struct IsoEntry {
    name: String,
    extent: u32,
    size: u32,
    flags: u8,
}

impl IsoEntry {
    fn is_dir(&self) -> bool {
        self.flags & FLAG_DIRECTORY != 0
    }
}

pub struct Iso9660Fs {
    device: BlockDev,
    root: IsoEntry,
}

impl Iso9660Fs {
    pub fn probe(device: BlockDev) -> Result<Self, &'static str> {
        let mut pvd = [0u8; ISO_BLOCK_SIZE];
        read_exact_at(&device, PVD_LBA * ISO_BLOCK_SIZE, &mut pvd)
            .map_err(|_| "failed to read ISO9660 primary volume descriptor")?;
        if pvd[0] != 1 || &pvd[1..6] != VOLUME_DESCRIPTOR_ID || pvd[6] != 1 {
            return Err("ISO9660 primary volume descriptor not found");
        }
        for lba in PVD_LBA + 1..PVD_LBA + 32 {
            let mut descriptor = [0u8; ISO_BLOCK_SIZE];
            read_exact_at(&device, lba * ISO_BLOCK_SIZE, &mut descriptor)
                .map_err(|_| "failed to read ISO9660 volume descriptors")?;
            if &descriptor[1..6] != VOLUME_DESCRIPTOR_ID {
                return Err("malformed ISO9660 volume descriptor");
            }
            if descriptor[0] == 2 {
                return Err("Joliet and supplementary volumes are unsupported");
            }
            if descriptor[0] == 255 {
                break;
            }
        }
        let logical_block_size = u16::from_le_bytes([pvd[128], pvd[129]]) as usize;
        if logical_block_size != ISO_BLOCK_SIZE {
            return Err("unsupported ISO9660 logical block size");
        }
        let root = parse_record(&pvd[156..]).ok_or("invalid ISO9660 root directory")?;
        if !root.is_dir() {
            return Err("ISO9660 root record is not a directory");
        }
        Ok(Self { device, root })
    }

    fn find_entry(&self, path: &str) -> Result<IsoEntry, ()> {
        let components: Vec<String> = path
            .split('/')
            .filter(|part| !part.is_empty())
            .map(normalize_component)
            .collect();
        if components.is_empty() {
            return Ok(self.root.clone());
        }

        let mut current = self.root.clone();
        for component in components {
            if !current.is_dir() {
                return Err(());
            }
            current = self
                .read_directory(&current)?
                .into_iter()
                .find(|entry| normalize_component(&entry.name) == component)
                .ok_or(())?;
        }
        Ok(current)
    }

    fn read_directory(&self, dir: &IsoEntry) -> Result<Vec<IsoEntry>, ()> {
        if !dir.is_dir() {
            return Err(());
        }
        let mut bytes = vec![0u8; dir.size as usize];
        read_exact_at(
            &self.device,
            dir.extent as usize * ISO_BLOCK_SIZE,
            &mut bytes,
        )?;

        let mut entries = Vec::new();
        let mut offset = 0usize;
        while offset < bytes.len() {
            let record_len = bytes[offset] as usize;
            if record_len == 0 {
                offset = ((offset / ISO_BLOCK_SIZE) + 1) * ISO_BLOCK_SIZE;
                continue;
            }
            if offset + record_len > bytes.len() {
                return Err(());
            }
            let record = parse_record(&bytes[offset..offset + record_len]).ok_or(())?;
            if record.flags & FLAG_MULTI_EXTENT != 0 {
                return Err(());
            }
            if record.name != "." && record.name != ".." {
                entries.push(record);
            }
            offset += record_len;
        }
        Ok(entries)
    }

    fn metadata_for(entry: &IsoEntry) -> Metadata {
        Metadata {
            ino: entry.extent,
            uid: 0,
            gid: 0,
            name: entry.name.clone(),
            file_type: if entry.is_dir() {
                FileType::Dir
            } else {
                FileType::File
            },
            mode: if entry.is_dir() { 0o040555 } else { 0o100444 },
            size: entry.size as usize,
            created_time: 0,
            access_time: 0,
            modified_time: 0,
        }
    }
}

impl FileSystem for Iso9660Fs {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()> {
        let entry = self.find_entry(path)?;
        let metadata = Self::metadata_for(&entry);
        Ok(VfsNode::new(
            self.device.clone(),
            metadata,
            Arc::new(RwLock::new(IsoNode { entry })),
        ))
    }

    fn mkdir(&mut self, _parent_dir: &str, _path: &str, _mode: u16) -> Result<(), ()> {
        Err(())
    }

    fn rmdir(&mut self, _path: &str) -> Result<(), ()> {
        Err(())
    }

    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()> {
        let dir = self.find_entry(path)?;
        Ok(self
            .read_directory(&dir)?
            .iter()
            .map(Self::metadata_for)
            .collect())
    }

    fn rm(&mut self, _path: &str) -> Result<(), ()> {
        Err(())
    }

    fn touch(&mut self, _parent_path: &str, _filename: &str, _mode: u16) -> Result<(), ()> {
        Err(())
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        self.find_entry(path)
            .map(|entry| Self::metadata_for(&entry))
    }

    fn fs_type_name(&self) -> &'static str {
        "iso9660"
    }

    fn source_name(&self) -> &'static str {
        "/dev/cdrom"
    }

    fn stats(&mut self) -> Result<FsStats, ()> {
        Ok(FsStats {
            block_size: ISO_BLOCK_SIZE as u64,
            fragment_size: ISO_BLOCK_SIZE as u64,
            name_length: 31,
            flags: 1,
            ..FsStats::default()
        })
    }
}

struct IsoNode {
    entry: IsoEntry,
}

impl VfsNodeOps for IsoNode {
    fn read(&self, device: &mut BlockDev, offset: usize, out: &mut [u8]) -> Result<usize, ()> {
        if self.entry.is_dir() {
            return Err(());
        }
        if offset >= self.entry.size as usize {
            return Ok(0);
        }
        let count = core::cmp::min(out.len(), self.entry.size as usize - offset);
        read_exact_at(
            device,
            self.entry.extent as usize * ISO_BLOCK_SIZE + offset,
            &mut out[..count],
        )?;
        Ok(count)
    }

    fn write(&mut self, _device: &mut BlockDev, _offset: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Err(())
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Err(())
    }
}

fn parse_record(data: &[u8]) -> Option<IsoEntry> {
    if data.len() < 34 {
        return None;
    }
    let record_len = data[0] as usize;
    if record_len < 34 || record_len > data.len() {
        return None;
    }
    let name_len = data[32] as usize;
    if 33 + name_len > record_len {
        return None;
    }
    let extent = u32::from_le_bytes(data[2..6].try_into().ok()?);
    let size = u32::from_le_bytes(data[10..14].try_into().ok()?);
    let flags = data[25];
    let raw_name = &data[33..33 + name_len];
    let name = match raw_name {
        [0] => ".".to_string(),
        [1] => "..".to_string(),
        _ => core::str::from_utf8(raw_name).ok()?.to_string(),
    };
    Some(IsoEntry {
        name,
        extent,
        size,
        flags,
    })
}

fn normalize_component(name: &str) -> String {
    name.split(';')
        .next()
        .unwrap_or(name)
        .trim_end_matches('.')
        .to_ascii_uppercase()
}

fn read_exact_at(device: &BlockDev, offset: usize, out: &mut [u8]) -> Result<(), ()> {
    if out.is_empty() {
        return Ok(());
    }
    let mut dev = device.lock();
    let block_size = dev.block_size();
    if block_size == 0 {
        return Err(());
    }
    let first_block = offset / block_size;
    let last_byte = offset.checked_add(out.len()).ok_or(())?;
    let last_block = (last_byte + block_size - 1) / block_size;
    if last_block > dev.block_count() {
        return Err(());
    }

    let mut written = 0usize;
    let mut block = first_block;

    let head_offset = offset % block_size;
    if head_offset != 0 {
        let mut scratch = vec![0u8; block_size];
        dev.read(block as u32, &mut scratch)?;
        let count = core::cmp::min(block_size - head_offset, out.len());
        out[..count].copy_from_slice(&scratch[head_offset..head_offset + count]);
        written += count;
        block += 1;
    }

    let whole_blocks = (out.len() - written) / block_size;
    if whole_blocks != 0 {
        let bytes = whole_blocks * block_size;
        dev.read_blocks(block as u32, &mut out[written..written + bytes])?;
        written += bytes;
        block += whole_blocks;
    }

    if written < out.len() {
        let mut scratch = vec![0u8; block_size];
        dev.read(block as u32, &mut scratch)?;
        let count = out.len() - written;
        out[written..].copy_from_slice(&scratch[..count]);
    }
    Ok(())
}

pub fn boxed_device<T: crate::driver::disk::BlockDeviceIO + Send>(device: T) -> BlockDev {
    Arc::new(Mutex::new(
        Box::new(device) as Box<dyn crate::driver::disk::BlockDeviceIO + Send>
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BytesDevice(Vec<u8>);

    impl crate::driver::disk::BlockDeviceIO for BytesDevice {
        fn read(&mut self, addr: u32, out: &mut [u8]) -> Result<(), ()> {
            let start = addr as usize * 512;
            out.copy_from_slice(self.0.get(start..start + out.len()).ok_or(())?);
            Ok(())
        }

        fn write(&mut self, _addr: u32, _buf: &[u8]) -> Result<(), ()> {
            Err(())
        }

        fn block_size(&self) -> usize {
            512
        }

        fn block_count(&self) -> usize {
            self.0.len() / 512
        }
    }

    #[test]
    fn normalizes_iso_versions() {
        assert_eq!(normalize_component("SYSTEM.TFS;1"), "SYSTEM.TFS");
        assert_eq!(normalize_component("README.;1"), "README");
    }

    #[test]
    fn rejects_short_directory_record() {
        assert!(parse_record(&[0; 20]).is_none());
    }

    #[test]
    fn reads_unaligned_ranges_across_device_blocks() {
        let bytes = (0..1536).map(|value| value as u8).collect();
        let device = boxed_device(BytesDevice(bytes));
        let mut out = [0u8; 700];
        read_exact_at(&device, 400, &mut out).unwrap();
        for (index, byte) in out.iter().enumerate() {
            assert_eq!(*byte, (400 + index) as u8);
        }
    }
}
