use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cmp::min;
use crate::kernel_utils::install::INITRAMFS;
use crate::serial_println;
use crate::driver::disk::dummy_blockdev;
use crate::sys::fs::vfs::{BlockDev, FileSystem, FileType, Metadata, VfsNode, VfsNodeOps};
use crate::sys::fs::ram_fs::initramfs::CpioError;
use spin::rwlock::RwLock;

pub mod initramfs;

pub enum Node {
    File { data: Vec<u8> },
    Dir  { children: BTreeMap<String, Node> },
    Symlink { target: String },
}

pub struct InitramfsFs {
    root: Node, // Dir
}

impl InitramfsFs {
    pub fn new() -> Self {
        let mut initramfs = INITRAMFS.lock();

        let mut root_children: BTreeMap<String, Node> = BTreeMap::new();

        while let Some(entry_res) = initramfs.next() {
            match entry_res {
                Ok(entry) => {
                    let Some(name) = entry.filename() else { continue };
                    let path = name.trim_matches('/');
                    if path.is_empty() {
                        continue;
                    }

                    let mut comps = path.split('/').filter(|c| !c.is_empty()).peekable();
                    let mut current = &mut root_children;

                    while let Some(comp) = comps.next() {
                        let is_last = comps.peek().is_none();
                        if is_last {
                            if entry.header.is_directory() {
                                current
                                    .entry(comp.to_string())
                                    .or_insert_with(|| Node::Dir { children: BTreeMap::new() });
                            } else if entry.header.is_symlink() {
                                let target = core::str::from_utf8(entry.data).unwrap_or("").to_string();
                                current.insert(
                                    comp.to_string(),
                                    Node::Symlink { target },
                                );
                            } else if entry.header.is_regular_file() {
                                current.insert(
                                    comp.to_string(),
                                    Node::File {
                                        data: entry.data.to_vec(),
                                    },
                                );
                            }
                        } else {
                            current = match current
                                .entry(comp.to_string())
                                .or_insert_with(|| Node::Dir { children: BTreeMap::new() })
                            {
                                Node::Dir { children } => children,
                                _ => {
                                    // Path component collides with non-dir; skip this entry.
                                    serial_println!(
                                        "initramfs: path component {} is not a directory, skipping {}",
                                        comp,
                                        name
                                    );
                                    break;
                                }
                            };
                        }
                    }
                }
                Err(CpioError::Trailer) => break,
                Err(err) => {
                    serial_println!("failed to read initramfs entry: {:?}", err);
                }
            }
        }

        let root = Node::Dir {
            children: root_children,
        };

        Self {
            root,
        }
    }

    fn split_path<'a>(&self, path: &'a str) -> Vec<&'a str> {
        path.trim_matches('/')
            .split('/')
            .filter(|c| !c.is_empty())
            .collect()
    }

    fn inode_for(path: &str) -> u32 {
        if path.is_empty() {
            return 1;
        }
        let mut h: u32 = 5381;
        for b in path.as_bytes() {
            h = h
                .wrapping_shl(5)
                .wrapping_add(h)
                .wrapping_add(*b as u32);
        }
        h
    }

    fn get_node<'a>(&'a self, comps: &[&str]) -> Option<&'a Node> {
        let mut cur = &self.root;
        for c in comps {
            match cur {
                Node::Dir { children } => cur = children.get(*c)?,
                _ => return None,
            }
        }
        Some(cur)
    }

    fn node_metadata(&self, comps: &[&str]) -> Option<Metadata> {
        let node = self.get_node(comps)?;
        let name = comps.last().copied().unwrap_or("");
        let ino = Self::inode_for(&comps.join("/"));

        let (file_type, size) = match node {
            Node::Dir { .. } => (FileType::Dir, 0),
            Node::File { data } => (FileType::File, data.len()),
            Node::Symlink { target } => (FileType::File, target.len()),
        };

        Some(Metadata {
            ino,
            name: name.to_string(),
            file_type,
            size,
            created_time: 0,
            access_time: 0,
            modified_time: 0,
        })
    }
}

struct RamFile {
    data: Vec<u8>,
}

impl VfsNodeOps for RamFile {
    fn read(&self, _device: &mut BlockDev, lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        if lba >= self.data.len() {
            return Ok(0);
        }
        let n = min(buf.len(), self.data.len() - lba);
        buf[..n].copy_from_slice(&self.data[lba..lba + n]);
        Ok(n)
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

struct RamSymlink {
    target: String,
}

impl VfsNodeOps for RamSymlink {
    fn read(&self, _device: &mut BlockDev, lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let data = self.target.as_bytes();
        if lba >= data.len() {
            return Ok(0);
        }
        let n = min(buf.len(), data.len() - lba);
        buf[..n].copy_from_slice(&data[lba..lba + n]);
        Ok(n)
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

struct RamDir;

impl VfsNodeOps for RamDir {
    fn read(&self, _device: &mut BlockDev, _lba: usize, _buf: &mut [u8]) -> Result<usize, ()> {
        Err(())
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

impl FileSystem for InitramfsFs {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()> {
        let comps = self.split_path(path);
        let node = self.get_node(&comps).ok_or(())?;
        let meta = self.node_metadata(&comps).ok_or(())?;

        let ops: Arc<RwLock<dyn VfsNodeOps>> = match node {
            Node::Dir { .. } => Arc::new(RwLock::new(RamDir)),
            Node::File { data } => Arc::new(RwLock::new(RamFile { data: data.clone() })),
            Node::Symlink { target } => {
                Arc::new(RwLock::new(RamSymlink { target: target.clone() }))
            }
        };

        Ok(VfsNode::new(dummy_blockdev(), meta, ops))
    }

    fn mkdir(&mut self, parent_dir: &str, path: &str) -> Result<(), ()> {
        let _ = (parent_dir, path);
        Err(())
    }

    fn rmdir(&mut self, path: &str) -> Result<(), ()> {
        let _ = path;
        Err(())
    }

    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()> {
        let comps = self.split_path(path);
        let node = self.get_node(&comps).ok_or(())?;
        let Node::Dir { children } = node else { return Err(()) };

        let mut entries = Vec::new();
        for (name, _child) in children.iter() {
            let mut child_comps = comps.clone();
            child_comps.push(name.as_str());
            if let Some(meta) = self.node_metadata(&child_comps) {
                entries.push(meta);
            }
        }
        Ok(entries)
    }

    fn rm(&mut self, path: &str) -> Result<(), ()> {
        let _ = path;
        Err(())
    }

    fn touch(&mut self, parent_path: &str, filename: &str) -> Result<(), ()> {
        let _ = (parent_path, filename);
        Err(())
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        let comps = self.split_path(path);
        self.node_metadata(&comps).ok_or(())
    }
}
