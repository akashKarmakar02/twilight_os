use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cmp;
use crate::driver::disk::BlockDeviceIO;
use crate::sys::fs::partition;

pub enum FatEntry<'a> {
    Directory(&'a str),
    File { path: &'a str, data: &'a [u8] },
}

#[derive(Debug)]
pub enum FatError {
    InvalidPath,
    DuplicateEntry,
    DeviceError,
    NoSpace,
}

const BYTES_PER_SECTOR: u16 = 512;
const SECTORS_PER_CLUSTER: u32 = 8; // 4 KiB clusters
const RESERVED_SECTORS: u16 = 32;
const NUM_FATS: u32 = 2;
const FAT32_EOC: u32 = 0x0FFF_FFFF;

pub fn format_partition(
    device: &mut dyn BlockDeviceIO,
    start_lba: u32,
    sector_count: u32,
    entries: &[FatEntry<'_>],
) -> Result<(), FatError> {
    if sector_count < u32::from(RESERVED_SECTORS) + 10 {
        return Err(FatError::NoSpace);
    }

    let mut builder = FatBuilder::new();

    for entry in entries {
        match entry {
            FatEntry::Directory(path) => builder.add_directory(path)?,
            FatEntry::File { path, data } => builder.add_file(path, data)?,
        }
    }

    let layout = Fat32Layout::new(start_lba, sector_count)?;
    builder.assign_short_names();
    builder.compute_directory_clusters(layout.cluster_size_bytes)?;

    let mut formatter = Fat32Formatter::new(device, layout, builder)?;
    formatter.write_volume()
}

struct FatBuilder<'a> {
    directories: Vec<Directory>,
    files: Vec<FileEntry<'a>>,
    path_map: BTreeMap<String, usize>,
}

impl<'a> FatBuilder<'a> {
    fn new() -> Self {
        let mut directories = Vec::new();
        directories.push(Directory::root());
        let mut path_map = BTreeMap::new();
        path_map.insert("/".to_string(), 0);
        Self {
            directories,
            files: Vec::new(),
            path_map,
        }
    }

    fn add_directory(&mut self, path: &str) -> Result<(), FatError> {
        let components = normalize_path(path)?;
        self.ensure_directory(&components)?;
        Ok(())
    }

    fn add_file(&mut self, path: &str, data: &'a [u8]) -> Result<(), FatError> {
        let components = normalize_path(path)?;
        if components.is_empty() {
            return Err(FatError::InvalidPath);
        }

        let file_name = components
            .last()
            .ok_or(FatError::InvalidPath)?
            .to_string();
        let parent_components = &components[..components.len() - 1];
        let parent_id = self.ensure_directory(parent_components)?;

        if self.has_child_named(parent_id, &file_name) {
            return Err(FatError::DuplicateEntry);
        }

        let forced_short = forced_short_for_file(&file_name);
        let file_id = self.files.len();
        self.files
            .push(FileEntry::new(file_name, data, forced_short));
        self.directories[parent_id].entries.push(DirItem::File(file_id));

        Ok(())
    }

    fn ensure_directory(&mut self, components: &[String]) -> Result<usize, FatError> {
        let mut current_id = 0usize;
        let mut current_path = "/".to_string();

        for component in components {
            if component.is_empty() {
                continue;
            }

            let next_path = if current_path == "/" {
                format!("/{}", component)
            } else {
                format!("{}/{}", current_path, component)
            };

            if let Some(&dir_id) = self.path_map.get(&next_path) {
                current_id = dir_id;
                current_path = next_path;
                continue;
            }

            if self.has_child_named(current_id, component) {
                return Err(FatError::DuplicateEntry);
            }

            let new_id = self.directories.len();
            self.directories
                .push(Directory::new(component.to_string(), Some(current_id)));
            self.directories[current_id]
                .entries
                .push(DirItem::Dir(new_id));
            self.path_map.insert(next_path.clone(), new_id);
            current_id = new_id;
            current_path = next_path;
        }

        Ok(current_id)
    }

    fn has_child_named(&self, dir_id: usize, name: &str) -> bool {
        self.directories[dir_id]
            .entries
            .iter()
            .any(|entry| match *entry {
                DirItem::Dir(child) => self.directories[child].name.eq_ignore_ascii_case(name),
                DirItem::File(file_id) => self.files[file_id].name.eq_ignore_ascii_case(name),
            })
    }

    fn assign_short_names(&mut self) {
        self.assign_short_names_recursive(0);
    }

    fn assign_short_names_recursive(&mut self, dir_id: usize) {
        let entries = self.directories[dir_id].entries.clone();
        let mut registry = ShortNameRegistry::new();

        for entry in entries.iter() {
            match *entry {
                DirItem::Dir(child) => {
                    let name = self.directories[child].name.clone();
                    let short = registry.generate(&name, true);
                    self.directories[child].short_name = Some(short);
                }
                DirItem::File(file_id) => {
                    if let Some(forced) = self.files[file_id].forced_short {
                        self.files[file_id].short_name = Some(forced);
                        registry.reserve(forced);
                    } else {
                        let name = self.files[file_id].name.clone();
                        let short = registry.generate(&name, false);
                        self.files[file_id].short_name = Some(short);
                    }
                }
            }
        }

        for entry in entries {
            if let DirItem::Dir(child) = entry {
                self.assign_short_names_recursive(child);
            }
        }
    }

    fn compute_directory_clusters(&mut self, cluster_bytes: usize) -> Result<(), FatError> {
        self.compute_directory_clusters_recursive(0, cluster_bytes)
    }

    fn compute_directory_clusters_recursive(
        &mut self,
        dir_id: usize,
        cluster_bytes: usize,
    ) -> Result<(), FatError> {
        let mut entry_count = 2; // '.' and '..'
        let entries = self.directories[dir_id].entries.clone();

        for entry in entries.iter() {
            entry_count += 1; // short entry
            let name = match *entry {
                DirItem::Dir(child) => &self.directories[child].name,
                DirItem::File(file_id) => &self.files[file_id].name,
            };
            entry_count += long_name_entry_slots(name);
        }

        entry_count += 1; // end marker
        let total_bytes = entry_count * 32;
        let clusters = cmp::max(
            1,
            (total_bytes + cluster_bytes - 1) / cluster_bytes,
        );
        self.directories[dir_id].cluster_count = clusters as u32;

        for entry in entries {
            if let DirItem::Dir(child) = entry {
                self.compute_directory_clusters_recursive(child, cluster_bytes)?;
            }
        }

        Ok(())
    }
}

#[allow(dead_code)]
struct Directory {
    name: String,
    parent: Option<usize>,
    entries: Vec<DirItem>,
    short_name: Option<[u8; 11]>,
    cluster: Option<u32>,
    cluster_count: u32,
}

impl Directory {
    fn root() -> Self {
        Self {
            name: "/".to_string(),
            parent: None,
            entries: Vec::new(),
            short_name: None,
            cluster: None,
            cluster_count: 1,
        }
    }

    fn new(name: String, parent: Option<usize>) -> Self {
        Self {
            name,
            parent,
            entries: Vec::new(),
            short_name: None,
            cluster: None,
            cluster_count: 1,
        }
    }
}

struct FileEntry<'a> {
    name: String,
    data: &'a [u8],
    short_name: Option<[u8; 11]>,
    first_cluster: Option<u32>,
    cluster_count: u32,
    forced_short: Option<[u8; 11]>,
}

impl<'a> FileEntry<'a> {
    fn new(name: String, data: &'a [u8], forced_short: Option<[u8; 11]>) -> Self {
        Self {
            name,
            data,
            short_name: None,
            first_cluster: None,
            cluster_count: 0,
            forced_short,
        }
    }
}

#[derive(Clone, Copy)]
enum DirItem {
    Dir(usize),
    File(usize),
}

struct Fat32Layout {
    start_lba: u32,
    total_sectors: u32,
    fat_size_sectors: u32,
    data_start_lba: u32,
    total_clusters: u32,
    cluster_size_bytes: usize,
}

impl Fat32Layout {
    fn new(start_lba: u32, total_sectors: u32) -> Result<Self, FatError> {
        let (fat_size, total_clusters) =
            compute_fat32_size(total_sectors)?;
        let data_start =
            start_lba + u32::from(RESERVED_SECTORS) + NUM_FATS * fat_size;
        Ok(Self {
            start_lba,
            total_sectors,
            fat_size_sectors: fat_size,
            data_start_lba: data_start,
            total_clusters,
            cluster_size_bytes: (u32::from(BYTES_PER_SECTOR) * SECTORS_PER_CLUSTER) as usize,
        })
    }
}

struct Fat32Formatter<'a, 'b> {
    device: &'a mut dyn BlockDeviceIO,
    layout: Fat32Layout,
    builder: FatBuilder<'b>,
    fat: Vec<u32>,
    next_free_cluster: u32,
}

impl<'a, 'b> Fat32Formatter<'a, 'b> {
    fn new(
        device: &'a mut dyn BlockDeviceIO,
        layout: Fat32Layout,
        builder: FatBuilder<'b>,
    ) -> Result<Self, FatError> {
        let fat_len = layout.total_clusters as usize + 2;
        let mut fat = vec![0u32; fat_len];
        fat[0] = 0x0FFFFFF8;
        fat[1] = FAT32_EOC;

        Ok(Self {
            device,
            layout,
            builder,
            fat,
            next_free_cluster: 2,
        })
    }

    fn write_volume(&mut self) -> Result<(), FatError> {
        self.write_boot_region()?;
        self.process_directory(0, None)?;
        self.write_fats()
    }

    fn process_directory(
        &mut self,
        dir_id: usize,
        parent_cluster: Option<u32>,
    ) -> Result<(), FatError> {
        if self.builder.directories[dir_id].cluster.is_none() {
            let start_cluster =
                self.allocate_chain(self.builder.directories[dir_id].cluster_count)?;
            self.builder.directories[dir_id].cluster = Some(start_cluster);
        }

        let entries = self.builder.directories[dir_id].entries.clone();

        for entry in entries.iter() {
            match *entry {
                DirItem::Dir(child) => {
                    self.process_directory(
                        child,
                        self.builder.directories[dir_id].cluster,
                    )?;
                }
                DirItem::File(file_id) => {
                    self.write_file(file_id)?;
                }
            }
        }

        let buffer = self.build_directory_buffer(
            dir_id,
            parent_cluster.unwrap_or(0),
        )?;
        let cluster = self.builder.directories[dir_id]
            .cluster
            .expect("cluster assigned");
        self.write_chain(
            cluster,
            self.builder.directories[dir_id].cluster_count,
            &buffer,
        )
    }

    fn write_file(&mut self, file_id: usize) -> Result<(), FatError> {
        let data = self.builder.files[file_id].data;
        if data.is_empty() {
            self.builder.files[file_id].first_cluster = None;
            self.builder.files[file_id].cluster_count = 0;
            return Ok(());
        }

        let clusters = clusters_for_size(
            data.len(),
            self.layout.cluster_size_bytes,
        );
        let start = self.allocate_chain(clusters)?;
        self.builder.files[file_id].first_cluster = Some(start);
        self.builder.files[file_id].cluster_count = clusters;
        self.write_chain(start, clusters, data)
    }

    fn allocate_chain(&mut self, clusters: u32) -> Result<u32, FatError> {
        if clusters == 0 {
            return Ok(0);
        }
        let start = self.next_free_cluster;
        let end = start + clusters;
        if (end as usize) >= self.fat.len() {
            return Err(FatError::NoSpace);
        }
        for current in start..end {
            let next = if current + 1 == end {
                FAT32_EOC
            } else {
                current + 1
            };
            self.fat[current as usize] = next;
        }
        self.next_free_cluster = end;
        Ok(start)
    }

    fn write_chain(
        &mut self,
        start_cluster: u32,
        clusters: u32,
        data: &[u8],
    ) -> Result<(), FatError> {
        if clusters == 0 {
            return Ok(());
        }

        let mut current = start_cluster;
        let mut remaining = clusters;
        let mut offset = 0usize;
        let cluster_size = self.layout.cluster_size_bytes;
        let mut buffer = vec![0u8; cluster_size];

        while remaining > 0 {
            buffer.fill(0);
            let to_copy = cmp::min(cluster_size, data.len().saturating_sub(offset));
            if to_copy > 0 {
                buffer[..to_copy].copy_from_slice(&data[offset..offset + to_copy]);
                offset += to_copy;
            }
            self.write_cluster(current, &buffer)?;
            remaining -= 1;
            if remaining == 0 {
                break;
            }
            let next = self.fat[current as usize];
            if next >= FAT32_EOC {
                return Err(FatError::NoSpace);
            }
            current = next;
        }
        Ok(())
    }

    fn write_cluster(&mut self, cluster: u32, buf: &[u8]) -> Result<(), FatError> {
        let mut lba = self.layout.data_start_lba + (cluster - 2) * SECTORS_PER_CLUSTER;
        for chunk in buf.chunks(partition::SECTOR_SIZE as usize) {
            self.device
                .write(lba, chunk)
                .map_err(|_| FatError::DeviceError)?;
            lba += 1;
        }
        Ok(())
    }

    fn build_directory_buffer(
        &self,
        dir_id: usize,
        parent_cluster: u32,
    ) -> Result<Vec<u8>, FatError> {
        let cluster_bytes =
            (self.builder.directories[dir_id].cluster_count as usize)
                * self.layout.cluster_size_bytes;
        let mut buffer = vec![0u8; cluster_bytes];
        let mut offset = 0usize;

        let self_cluster = self.builder.directories[dir_id]
            .cluster
            .unwrap_or(0);

        let dot_entry = make_short_entry(b".          ", 0x10, self_cluster, 0);
        buffer[offset..offset + 32].copy_from_slice(&dot_entry);
        offset += 32;

        let mut dotdot = [0u8; 32];
        dotdot[..11].copy_from_slice(b"..         ");
        dotdot[11] = 0x10;
        set_first_cluster(&mut dotdot, parent_cluster);
        buffer[offset..offset + 32].copy_from_slice(&dotdot);
        offset += 32;

        for entry in self.builder.directories[dir_id].entries.iter() {
            match *entry {
                DirItem::Dir(child) => {
                    let name = &self.builder.directories[child].name;
                    let short = self.builder.directories[child]
                        .short_name
                        .unwrap_or_else(|| fallback_short_name(name, true));
                    let lfn = build_lfn_entries(name, short);
                    offset = write_entries(&mut buffer, offset, &lfn)?;
                    let dir_cluster = self.builder.directories[child]
                        .cluster
                        .unwrap_or(0);
                    let short_entry =
                        make_short_entry(&short, 0x10, dir_cluster, 0);
                    if offset + 32 > buffer.len() {
                        return Err(FatError::NoSpace);
                    }
                    buffer[offset..offset + 32].copy_from_slice(&short_entry);
                    offset += 32;
                }
                DirItem::File(file_id) => {
                    let file = &self.builder.files[file_id];
                    let short = file
                        .short_name
                        .unwrap_or_else(|| fallback_short_name(&file.name, false));
                    let lfn = build_lfn_entries(&file.name, short);
                    offset = write_entries(&mut buffer, offset, &lfn)?;
                    let start_cluster = file.first_cluster.unwrap_or(0);
                    let size = file.data.len() as u32;
                    let short_entry =
                        make_short_entry(&short, 0x20, start_cluster, size);
                    if offset + 32 > buffer.len() {
                        return Err(FatError::NoSpace);
                    }
                    buffer[offset..offset + 32].copy_from_slice(&short_entry);
                    offset += 32;
                }
            }
        }

        if offset + 32 <= buffer.len() {
            buffer[offset] = 0x00;
        }

        Ok(buffer)
    }

    fn write_boot_region(&mut self) -> Result<(), FatError> {
        let mut sector = [0u8; partition::SECTOR_SIZE as usize];
        sector[0] = 0xEB;
        sector[1] = 0x58;
        sector[2] = 0x90;
        sector[3..11].copy_from_slice(b"TWILIGHT");
        sector[11..13].copy_from_slice(&BYTES_PER_SECTOR.to_le_bytes());
        sector[13] = SECTORS_PER_CLUSTER as u8;
        sector[14..16].copy_from_slice(&RESERVED_SECTORS.to_le_bytes());
        sector[16] = NUM_FATS as u8;
        sector[17..19].fill(0);
        sector[19..21].fill(0);
        sector[21] = 0xF8;
        sector[22..24].copy_from_slice(&0u16.to_le_bytes());
        sector[24..26].copy_from_slice(&0x20u16.to_le_bytes());
        sector[26..28].copy_from_slice(&0x40u16.to_le_bytes());
        sector[28..32].copy_from_slice(&self.layout.start_lba.to_le_bytes());
        sector[32..36].copy_from_slice(&self.layout.total_sectors.to_le_bytes());
        sector[36..40].copy_from_slice(&self.layout.fat_size_sectors.to_le_bytes());
        sector[40..42].copy_from_slice(&0u16.to_le_bytes());
        sector[42..44].copy_from_slice(&0u16.to_le_bytes());
        sector[44..48].copy_from_slice(&2u32.to_le_bytes());
        sector[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo
        sector[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup
        sector[64..76].fill(0);
        sector[64] = 0x00;
        sector[66] = 0x29;
        sector[67..71].copy_from_slice(&0x12345678u32.to_le_bytes());
        sector[71..82].copy_from_slice(b"TWILIGHTOS ");
        sector[82..90].copy_from_slice(b"FAT32   ");
        sector[510] = 0x55;
        sector[511] = 0xAA;

        self.device
            .write(self.layout.start_lba, &sector)
            .map_err(|_| FatError::DeviceError)?;

        let mut fs_info = [0u8; partition::SECTOR_SIZE as usize];
        fs_info[0..4].copy_from_slice(&0x41615252u32.to_le_bytes());
        fs_info[484..488].copy_from_slice(&0x61417272u32.to_le_bytes());
        fs_info[488..492].copy_from_slice(&(self.layout.total_clusters - 1).to_le_bytes());
        fs_info[492..496].copy_from_slice(&2u32.to_le_bytes());
        fs_info[510] = 0x55;
        fs_info[511] = 0xAA;

        self.device
            .write(self.layout.start_lba + 1, &fs_info)
            .map_err(|_| FatError::DeviceError)?;

        self.device
            .write(self.layout.start_lba + 6, &sector)
            .map_err(|_| FatError::DeviceError)?;
        self.device
            .write(self.layout.start_lba + 7, &fs_info)
            .map_err(|_| FatError::DeviceError)?;

        let zero_sector = [0u8; partition::SECTOR_SIZE as usize];
        for offset in 2..u32::from(RESERVED_SECTORS) {
            if offset == 6 || offset == 7 {
                continue;
            }
            self.device
                .write(self.layout.start_lba + offset, &zero_sector)
                .map_err(|_| FatError::DeviceError)?;
        }
        Ok(())
    }

    fn write_fats(&mut self) -> Result<(), FatError> {
        let fat_bytes = self.layout.fat_size_sectors as usize * partition::SECTOR_SIZE as usize;
        let mut buffer = vec![0u8; fat_bytes];
        for (idx, entry) in self.fat.iter().enumerate() {
            let offset = idx * 4;
            if offset + 4 > buffer.len() {
                break;
            }
            buffer[offset..offset + 4].copy_from_slice(&entry.to_le_bytes());
        }

        let mut fat_lba = self.layout.start_lba + u32::from(RESERVED_SECTORS);
        for _ in 0..NUM_FATS {
            for (index, chunk) in buffer.chunks(partition::SECTOR_SIZE as usize).enumerate() {
                let lba = fat_lba + index as u32;
                self.device
                    .write(lba, chunk)
                    .map_err(|_| FatError::DeviceError)?;
            }
            fat_lba += self.layout.fat_size_sectors;
        }
        Ok(())
    }
}

fn write_entries(
    buffer: &mut [u8],
    mut offset: usize,
    entries: &[[u8; 32]],
) -> Result<usize, FatError> {
    for entry in entries {
        if offset + 32 > buffer.len() {
            return Err(FatError::NoSpace);
        }
        buffer[offset..offset + 32].copy_from_slice(entry);
        offset += 32;
    }
    Ok(offset)
}

fn make_short_entry(
    short: &[u8; 11],
    attr: u8,
    first_cluster: u32,
    size: u32,
) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[..11].copy_from_slice(short);
    entry[11] = attr;
    set_first_cluster(&mut entry, first_cluster);
    entry[28..32].copy_from_slice(&size.to_le_bytes());
    entry
}

fn set_first_cluster(entry: &mut [u8; 32], cluster: u32) {
    entry[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    entry[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
}

fn build_lfn_entries(name: &str, short: [u8; 11]) -> Vec<[u8; 32]> {
    let mut utf16: Vec<u16> = name.encode_utf16().collect();
    utf16.push(0);
    while utf16.len() % 13 != 0 {
        utf16.push(0xFFFF);
    }
    let parts = utf16.len() / 13;
    let checksum = lfn_checksum(&short);
    let mut entries = Vec::with_capacity(parts);
    for chunk_index in (0..parts).rev() {
        let mut entry = [0u8; 32];
        let order = (chunk_index + 1) as u8;
        entry[0] = if chunk_index + 1 == parts {
            order | 0x40
        } else {
            order
        };
        entry[11] = 0x0F;
        entry[12] = 0x00;
        entry[13] = checksum;
        entry[26..28].copy_from_slice(&0u16.to_le_bytes());
        fill_name_chunk(
            &mut entry,
            &utf16[chunk_index * 13..(chunk_index + 1) * 13],
        );
        entries.push(entry);
    }
    entries
}

fn fill_name_chunk(entry: &mut [u8; 32], chunk: &[u16]) {
    for i in 0..5 {
        let val = chunk[i];
        entry[1 + i * 2..1 + i * 2 + 2].copy_from_slice(&val.to_le_bytes());
    }
    for i in 0..6 {
        let val = chunk[5 + i];
        entry[14 + i * 2..14 + i * 2 + 2].copy_from_slice(&val.to_le_bytes());
    }
    for i in 0..2 {
        let val = chunk[11 + i];
        entry[28 + i * 2..28 + i * 2 + 2].copy_from_slice(&val.to_le_bytes());
    }
}

fn lfn_checksum(short: &[u8; 11]) -> u8 {
    let mut checksum = 0u8;
    for &byte in short {
        checksum = ((checksum & 1) << 7) + (checksum >> 1) + byte;
    }
    checksum
}

fn normalize_path(path: &str) -> Result<Vec<String>, FatError> {
    if path.is_empty() {
        return Err(FatError::InvalidPath);
    }
    let mut components = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(FatError::InvalidPath);
        }
        components.push(part.to_string());
    }
    Ok(components)
}

fn long_name_entry_slots(name: &str) -> usize {
    let chars = name.encode_utf16().count() + 1;
    (chars + 12) / 13
}

fn clusters_for_size(size: usize, cluster_bytes: usize) -> u32 {
    if size == 0 {
        0
    } else {
        ((size + cluster_bytes - 1) / cluster_bytes) as u32
    }
}

fn compute_fat32_size(total_sectors: u32) -> Result<(u32, u32), FatError> {
    let reserved = u32::from(RESERVED_SECTORS);
    let mut fat_size = ((total_sectors - reserved) / (SECTORS_PER_CLUSTER * NUM_FATS + 1))
        .max(1);
    loop {
        let data_sectors = total_sectors
            .checked_sub(reserved + NUM_FATS * fat_size)
            .ok_or(FatError::NoSpace)?;
        let total_clusters = data_sectors / SECTORS_PER_CLUSTER;
        if total_clusters < 65525 {
            fat_size += 1;
            continue;
        }
        let fat_bytes = (total_clusters + 2) * 4;
        let required = (fat_bytes + u32::from(BYTES_PER_SECTOR) - 1) / u32::from(BYTES_PER_SECTOR);
        if required == fat_size {
            return Ok((fat_size, total_clusters));
        }
        fat_size = required;
    }
}

fn fallback_short_name(name: &str, is_dir: bool) -> [u8; 11] {
    let (base_raw, ext_raw) = split_filename(name, is_dir);
    let base = sanitize_component(&base_raw, 8);
    let ext = if is_dir {
        String::new()
    } else {
        sanitize_component(&ext_raw, 3)
    };
    compose_short(&base, &ext)
}

fn split_filename(name: &str, is_dir: bool) -> (String, String) {
    if is_dir {
        return (name.to_string(), String::new());
    }

    let trimmed = name.trim_matches('/');
    if trimmed.is_empty() {
        return ("_".into(), String::new());
    }

    if let Some(dot) = trimmed.rfind('.') {
        if dot == 0 {
            return ("_".into(), trimmed[1..].to_string());
        }
        let base = &trimmed[..dot];
        let ext = &trimmed[dot + 1..];
        (base.to_string(), ext.to_string())
    } else {
        (trimmed.to_string(), String::new())
    }
}

fn sanitize_component(part: &str, max_len: usize) -> String {
    let mut out = String::new();
    for ch in part.chars() {
        if out.len() >= max_len {
            break;
        }
        let up = ch.to_ascii_uppercase();
        if is_valid_short_char(up) {
            out.push(up);
        } else {
            out.push('_');
        }
    }
    out
}

fn compose_short(base: &str, ext: &str) -> [u8; 11] {
    let mut short = [b' '; 11];
    for (idx, byte) in base.as_bytes().iter().take(8).enumerate() {
        short[idx] = *byte;
    }
    for (idx, byte) in ext.as_bytes().iter().take(3).enumerate() {
        short[8 + idx] = *byte;
    }
    short
}

fn is_valid_short_char(ch: char) -> bool {
    matches!(
        ch,
        'A'..='Z'
            | '0'..='9'
            | '!'
            | '#'
            | '$'
            | '%'
            | '&'
            | '\''
            | '('
            | ')'
            | '-'
            | '@'
            | '^'
            | '_'
            | '`'
            | '{'
            | '}'
            | '~'
    )
}

fn forced_short_for_file(name: &str) -> Option<[u8; 11]> {
    if equals_ignore_case(name, "limine-bios.sys") {
        Some(*b"LIMINE  SYS")
    } else {
        None
    }
}

fn equals_ignore_case(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.chars().zip(b.chars()).all(|(x, y)| x.eq_ignore_ascii_case(&y))
}

struct ShortNameRegistry {
    counts: BTreeMap<(String, String, bool), u32>,
    used: BTreeSet<[u8; 11]>,
}

impl ShortNameRegistry {
    fn new() -> Self {
        Self {
            counts: BTreeMap::new(),
            used: BTreeSet::new(),
        }
    }

    fn generate(&mut self, original: &str, is_dir: bool) -> [u8; 11] {
        let (base_raw, ext_raw) = split_filename(original, is_dir);
        let base_clean = {
            let mut base = sanitize_component(&base_raw, 8);
            if base.is_empty() {
                base.push('_');
            }
            base
        };
        let ext = if is_dir {
            String::new()
        } else {
            sanitize_component(&ext_raw, 3)
        };

        let key = (base_clean.clone(), ext.clone(), is_dir);
        let count_entry = self.counts.entry(key).or_insert(0);
        let mut attempt = *count_entry;

        loop {
            let mut current = base_clean.clone();
            if attempt > 0 {
                let suffix = format!("~{}", attempt);
                let avail = 8usize.saturating_sub(suffix.len());
                if avail == 0 {
                    current = suffix
                        .chars()
                        .rev()
                        .take(8)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                } else {
                    current.truncate(avail.max(1));
                    current.push_str(&suffix);
                }
            }

            let short = compose_short(&current, &ext);
            if self.used.insert(short) {
                attempt += 1;
                *count_entry = attempt;
                return short;
            }

            attempt += 1;
        }
    }

    fn reserve(&mut self, short: [u8; 11]) {
        self.used.insert(short);
    }
}
