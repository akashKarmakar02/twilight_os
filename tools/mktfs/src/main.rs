use std::env;
use std::fs;
use std::io;
use std::mem::size_of;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const BLOCK_SIZE: usize = 2048;
const IMAGE_ALIGNMENT: usize = 4 * 1024 * 1024;
const IMAGE_GROWTH: usize = 8 * 1024 * 1024;
const IMAGE_HEADROOM: usize = 16 * 1024 * 1024;
const MAGIC: u32 = u32::from_le_bytes(*b"TFS0");
const MODE_DIR: u16 = 0o040000;
const MODE_FILE: u16 = 0o100000;
const MODE_PERM_MASK: u16 = 0o7777;
const DIRECT_SLOTS: usize = 6;
const POINTERS_PER_BLOCK: usize = BLOCK_SIZE / 4;

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct Superblock {
    ninodes: u32,
    pad1: u16,
    imap_blocks: u32,
    zmap_blocks: u32,
    first_data_zone: u32,
    log_zone_size: u16,
    pad2: u16,
    max_size: u32,
    zones: u32,
    magic: u32,
    pad3: u16,
    block_size: u16,
    subversion: u8,
}

#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
struct Extent32 {
    start_block: u32,
    block_len: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Inode {
    mode: u16,
    nlinks: u16,
    uid: u32,
    gid: u32,
    size: u64,
    access_time: u64,
    modified_time: u64,
    change_time: u64,
    created_time: u64,
    flags: u32,
    generation: u64,
    xattr_block: u32,
    pad0: u32,
    direct: [Extent32; DIRECT_SLOTS],
    indirect: u32,
    double_indirect: u32,
    triple_indirect: u32,
    inline_data: [u8; 64],
    inode_checksum: u32,
    pad1: u32,
}

impl Default for Inode {
    fn default() -> Self {
        Self {
            mode: 0,
            nlinks: 1,
            uid: 0,
            gid: 0,
            size: 0,
            access_time: 0,
            modified_time: 0,
            change_time: 0,
            created_time: 0,
            flags: 0,
            generation: 0,
            xattr_block: 0,
            pad0: 0,
            direct: [Extent32::default(); DIRECT_SLOTS],
            indirect: 0,
            double_indirect: 0,
            triple_indirect: 0,
            inline_data: [0; 64],
            inode_checksum: 0,
            pad1: 0,
        }
    }
}

#[derive(Clone)]
enum NodeKind {
    Directory,
    File(PathBuf),
}

#[derive(Clone)]
struct Node {
    name: String,
    parent: usize,
    mode: u16,
    kind: NodeKind,
    children: Vec<usize>,
}

struct Builder {
    image: Vec<u8>,
    sb: Superblock,
    next_zone: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let source = PathBuf::from(args.next().ok_or("usage: mktfs <source-dir> <output>")?);
    let output = PathBuf::from(args.next().ok_or("usage: mktfs <source-dir> <output>")?);
    if args.next().is_some() {
        return Err("usage: mktfs <source-dir> <output>".into());
    }

    let nodes = collect_tree(&source)?;
    let payload = nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::File(path) => fs::metadata(path).ok().map(|meta| meta.len() as usize),
            NodeKind::Directory => None,
        })
        .sum::<usize>();
    let mut image_size =
        align_up(payload.saturating_add(IMAGE_HEADROOM), IMAGE_ALIGNMENT).max(IMAGE_ALIGNMENT);

    loop {
        match build_image(&nodes, image_size) {
            Ok(image) => {
                fs::write(&output, image)?;
                println!(
                    "mktfs: wrote {} bytes with {} entries to {}",
                    image_size,
                    nodes.len(),
                    output.display()
                );
                return Ok(());
            }
            Err(BuildError::NoSpace) => {
                image_size = image_size
                    .checked_add(IMAGE_GROWTH)
                    .ok_or("TwilightFS image size overflow")?;
            }
            Err(BuildError::Invalid(message)) => return Err(message.into()),
            Err(BuildError::Io(error)) => return Err(error.into()),
        }
    }
}

fn collect_tree(source: &Path) -> io::Result<Vec<Node>> {
    let mut nodes = vec![Node {
        name: String::new(),
        parent: 0,
        mode: 0o755,
        kind: NodeKind::Directory,
        children: Vec::new(),
    }];
    collect_directory(source, 0, "/", &mut nodes)?;
    Ok(nodes)
}

fn collect_directory(
    directory: &Path,
    parent_id: usize,
    parent_path: &str,
    nodes: &mut Vec<Node>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let metadata = entry.metadata()?;
        let file_type = metadata.file_type();
        if !file_type.is_dir() && !file_type.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported file type: {}", entry.path().display()),
            ));
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.as_bytes().len() > 60 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("TwilightFS name exceeds 60 bytes: {name}"),
            ));
        }
        let path = if parent_path == "/" {
            format!("/{name}")
        } else {
            format!("{parent_path}/{name}")
        };
        let id = nodes.len();
        nodes.push(Node {
            name,
            parent: parent_id,
            mode: metadata.permissions().mode() as u16 & MODE_PERM_MASK,
            kind: if file_type.is_dir() {
                NodeKind::Directory
            } else {
                NodeKind::File(entry.path())
            },
            children: Vec::new(),
        });
        nodes[parent_id].children.push(id);
        if file_type.is_dir() {
            collect_directory(&entry.path(), id, &path, nodes)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
enum BuildError {
    NoSpace,
    Invalid(&'static str),
    Io(io::Error),
}

impl From<io::Error> for BuildError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn build_image(nodes: &[Node], size: usize) -> Result<Vec<u8>, BuildError> {
    if size % BLOCK_SIZE != 0 {
        return Err(BuildError::Invalid("image size is not block aligned"));
    }
    let blocks = size / BLOCK_SIZE;
    let ninodes = (size / (16 * 1024)).max(nodes.len() + 1);
    let bits_per_block = BLOCK_SIZE * 8;
    let imap_blocks = div_ceil(ninodes, bits_per_block);
    let inode_blocks = div_ceil(ninodes * size_of::<Inode>(), BLOCK_SIZE);
    let mut zmap_blocks = 0usize;
    let mut first_data_zone = 0usize;
    for _ in 0..8 {
        first_data_zone = 1 + imap_blocks + zmap_blocks + inode_blocks;
        let data_zones = blocks.saturating_sub(first_data_zone);
        let next = div_ceil(data_zones, bits_per_block);
        if next == zmap_blocks {
            break;
        }
        zmap_blocks = next;
    }
    if first_data_zone >= blocks {
        return Err(BuildError::NoSpace);
    }

    let sb = Superblock {
        ninodes: ninodes as u32,
        imap_blocks: imap_blocks as u32,
        zmap_blocks: zmap_blocks as u32,
        first_data_zone: first_data_zone as u32,
        max_size: i32::MAX as u32,
        zones: blocks as u32,
        magic: MAGIC,
        block_size: BLOCK_SIZE as u16,
        ..Superblock::default()
    };
    let mut builder = Builder {
        image: vec![0u8; size],
        sb,
        next_zone: first_data_zone as u32,
    };
    builder.write_struct(0, &sb)?;

    let mut inodes = vec![Inode::default(); nodes.len()];
    for (index, node) in nodes.iter().enumerate() {
        builder.mark_inode(index)?;
        let inode = match &node.kind {
            NodeKind::Directory => builder.write_directory(nodes, index)?,
            NodeKind::File(path) => builder.write_file(path)?,
        };
        inodes[index] = inode;
    }
    for (index, inode) in inodes.iter().enumerate() {
        builder.write_inode(index + 1, inode)?;
    }
    Ok(builder.image)
}

impl Builder {
    fn write_directory(&mut self, nodes: &[Node], id: usize) -> Result<Inode, BuildError> {
        let node = &nodes[id];
        let mut entries = Vec::with_capacity(node.children.len() + 2);
        entries.push((id + 1, "."));
        entries.push((nodes[id].parent + 1, ".."));
        for child in &node.children {
            entries.push((*child + 1, nodes[*child].name.as_str()));
        }
        let bytes_len = entries.len() * 64;
        let blocks = div_ceil(bytes_len, BLOCK_SIZE);
        if blocks > DIRECT_SLOTS {
            return Err(BuildError::Invalid(
                "directory exceeds TwilightFS direct block capacity",
            ));
        }
        let mut inode = Inode {
            mode: MODE_DIR | node.mode,
            nlinks: 2,
            size: bytes_len as u64,
            ..Inode::default()
        };
        for block_index in 0..blocks {
            let zone = self.alloc_zone()?;
            inode.direct[block_index] = Extent32 {
                start_block: zone,
                block_len: 1,
            };
        }
        for (entry_index, (inode_no, name)) in entries.into_iter().enumerate() {
            let block = entry_index / (BLOCK_SIZE / 64);
            let slot = entry_index % (BLOCK_SIZE / 64);
            let zone = inode.direct[block].start_block;
            let offset = zone as usize * BLOCK_SIZE + slot * 64;
            self.image[offset..offset + 4].copy_from_slice(&(inode_no as u32).to_le_bytes());
            let name_bytes = name.as_bytes();
            self.image[offset + 4..offset + 4 + name_bytes.len()].copy_from_slice(name_bytes);
        }
        Ok(inode)
    }

    fn write_file(&mut self, source: &Path) -> Result<Inode, BuildError> {
        let data = fs::read(source)?;
        let required = div_ceil(data.len(), BLOCK_SIZE);
        let max = DIRECT_SLOTS + POINTERS_PER_BLOCK + POINTERS_PER_BLOCK * POINTERS_PER_BLOCK;
        if required > max {
            return Err(BuildError::Invalid(
                "file exceeds TwilightFS maximum mapping capacity",
            ));
        }
        let mode = fs::metadata(source)?.permissions().mode() as u16 & MODE_PERM_MASK;
        let mut inode = Inode {
            mode: MODE_FILE | mode,
            size: data.len() as u64,
            ..Inode::default()
        };
        let mut data_zones = Vec::with_capacity(required);
        for _ in 0..required {
            data_zones.push(self.alloc_zone()?);
        }
        for (index, zone) in data_zones.iter().copied().enumerate() {
            let start = index * BLOCK_SIZE;
            let end = (start + BLOCK_SIZE).min(data.len());
            let image_offset = zone as usize * BLOCK_SIZE;
            self.image[image_offset..image_offset + end - start].copy_from_slice(&data[start..end]);
        }
        for (slot, zone) in data_zones.iter().take(DIRECT_SLOTS).copied().enumerate() {
            inode.direct[slot] = Extent32 {
                start_block: zone,
                block_len: 1,
            };
        }
        let mut cursor = DIRECT_SLOTS;
        if cursor < data_zones.len() {
            inode.indirect = self.alloc_zone()?;
            for (slot, zone) in data_zones[cursor..]
                .iter()
                .take(POINTERS_PER_BLOCK)
                .copied()
                .enumerate()
            {
                self.write_pointer(inode.indirect, slot, zone)?;
                cursor += 1;
            }
        }
        if cursor < data_zones.len() {
            inode.double_indirect = self.alloc_zone()?;
            let remaining = &data_zones[cursor..];
            for (group, zones) in remaining.chunks(POINTERS_PER_BLOCK).enumerate() {
                let indirect = self.alloc_zone()?;
                self.write_pointer(inode.double_indirect, group, indirect)?;
                for (slot, zone) in zones.iter().copied().enumerate() {
                    self.write_pointer(indirect, slot, zone)?;
                }
            }
        }
        Ok(inode)
    }

    fn alloc_zone(&mut self) -> Result<u32, BuildError> {
        if self.next_zone as usize >= self.sb.zones as usize {
            return Err(BuildError::NoSpace);
        }
        let zone = self.next_zone;
        self.next_zone += 1;
        let relative = zone - self.sb.first_data_zone;
        let bit = relative as usize;
        let zmap_start = 1 + self.sb.imap_blocks as usize;
        self.image[zmap_start * BLOCK_SIZE + bit / 8] |= 1 << (bit % 8);
        Ok(zone)
    }

    fn mark_inode(&mut self, index: usize) -> Result<(), BuildError> {
        if index >= self.sb.ninodes as usize {
            return Err(BuildError::NoSpace);
        }
        self.image[BLOCK_SIZE + index / 8] |= 1 << (index % 8);
        Ok(())
    }

    fn write_inode(&mut self, inode_no: usize, inode: &Inode) -> Result<(), BuildError> {
        let inodes_per_block = BLOCK_SIZE / size_of::<Inode>();
        let index = inode_no
            .checked_sub(1)
            .ok_or(BuildError::Invalid("invalid inode"))?;
        let table = 1 + self.sb.imap_blocks as usize + self.sb.zmap_blocks as usize;
        let offset = (table + index / inodes_per_block) * BLOCK_SIZE
            + (index % inodes_per_block) * size_of::<Inode>();
        self.write_struct(offset, inode)
    }

    fn write_pointer(&mut self, zone: u32, slot: usize, value: u32) -> Result<(), BuildError> {
        if slot >= POINTERS_PER_BLOCK {
            return Err(BuildError::NoSpace);
        }
        let offset = zone as usize * BLOCK_SIZE + slot * 4;
        self.image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn write_struct<T: Copy>(&mut self, offset: usize, value: &T) -> Result<(), BuildError> {
        let end = offset
            .checked_add(size_of::<T>())
            .ok_or(BuildError::NoSpace)?;
        if end > self.image.len() {
            return Err(BuildError::NoSpace);
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(value as *const T as *const u8, size_of::<T>()) };
        self.image[offset..end].copy_from_slice(bytes);
        Ok(())
    }
}

fn div_ceil(value: usize, divisor: usize) -> usize {
    if value == 0 {
        0
    } else {
        1 + (value - 1) / divisor
    }
}

fn align_up(value: usize, alignment: usize) -> usize {
    div_ceil(value, alignment) * alignment
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_disk_sizes_are_stable() {
        assert_eq!(size_of::<Superblock>(), 39);
        assert_eq!(size_of::<Extent32>(), 8);
        assert_eq!(size_of::<Inode>(), 204);
    }
}
