mod full;
mod null;
mod random;
mod zero;

use crate::driver::disk::dummy_blockdev;
use crate::driver::keyboard::KeyboardDev;
use crate::driver::mouse::MouseDev;
use crate::fs::vfs::Metadata;
use crate::sys::console::tty::TtyDev;
use crate::sys::framebuffer::FramebufferDev;
use crate::sys::fs::devfs::full::Full;
use crate::sys::fs::devfs::null::Null;
use crate::sys::fs::devfs::random::{RandomDev, URandomDev};
use crate::sys::fs::devfs::zero::Zero;
use crate::sys::fs::vfs::{BlockDev, FileSystem, FileType, VfsNode, VfsNodeOps};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::rwlock::RwLock;

pub struct DevFs {
    file_structure: Vec<(String, VfsNode)>,
}

impl DevFs {
    pub fn new() -> Self {
        let mut devices = Vec::new();
        let null_meta = Metadata::chr(2, "null");
        devices.push((
            "null".to_string(),
            VfsNode::new(dummy_blockdev(), null_meta, Arc::new(RwLock::new(Null))),
        ));
        let fb_meta = Metadata::chr(3, "fb0");
        devices.push((
            "fb0".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                fb_meta,
                Arc::new(RwLock::new(FramebufferDev)),
            ),
        ));
        let tty_meta = Metadata::chr(4, "tty");
        devices.push((
            "tty".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                tty_meta,
                Arc::new(RwLock::new(TtyDev)),
            ),
        ));
        let zero_meta = Metadata::chr(5, "zero");
        devices.push((
            "zero".to_string(),
            VfsNode::new(dummy_blockdev(), zero_meta, Arc::new(RwLock::new(Zero))),
        ));
        let random_meta = Metadata::chr(6, "random");
        devices.push((
            "random".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                random_meta,
                Arc::new(RwLock::new(RandomDev)),
            ),
        ));
        let urandom_meta = Metadata::chr(7, "urandom");
        devices.push((
            "urandom".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                urandom_meta,
                Arc::new(RwLock::new(URandomDev)),
            ),
        ));
        let full_meta = Metadata::chr(8, "full");
        devices.push((
            "full".to_string(),
            VfsNode::new(dummy_blockdev(), full_meta, Arc::new(RwLock::new(Full))),
        ));
        let input_dir_meta = Metadata::dir(9, "input");
        devices.push((
            "input".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                input_dir_meta,
                Arc::new(RwLock::new(DirNodeOps)),
            ),
        ));
        let mice_meta = Metadata::chr(10, "mice");
        devices.push((
            "input/mice".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                mice_meta,
                Arc::new(RwLock::new(MouseDev::new())),
            ),
        ));
        let keyboard_meta = Metadata::chr(11, "event0");
        devices.push((
            "input/event0".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                keyboard_meta,
                Arc::new(RwLock::new(KeyboardDev)),
            ),
        ));

        DevFs {
            file_structure: devices,
        }
    }

    fn is_root(path: &str) -> bool {
        let p = path.trim_matches('/');
        p.is_empty()
    }

    fn root_metadata(&self) -> Metadata {
        Metadata::dir(1, "")
    }
}

struct DirNodeOps;

impl VfsNodeOps for DirNodeOps {
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

impl FileSystem for DevFs {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()> {
        if Self::is_root(path) {
            let meta = self.root_metadata();
            return Ok(VfsNode::new(
                dummy_blockdev(),
                meta,
                Arc::new(RwLock::new(DirNodeOps)),
            ));
        }

        let rel = path.trim_matches('/');
        if rel.is_empty() {
            return Err(());
        }

        if let Some((_, node)) = self
            .file_structure
            .iter()
            .find(|(p, _)| p.as_str() == rel)
        {
            return Ok(node.clone());
        }

        Err(())
    }

    fn mkdir(&mut self, _parent_dir: &str, _path: &str) -> Result<(), ()> {
        Err(())
    }
    fn rmdir(&mut self, _path: &str) -> Result<(), ()> {
        Err(())
    }
    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()> {
        let rel = path.trim_matches('/');

        if !Self::is_root(path) && !self.is_directory(rel) {
            return Err(());
        }

        let parent = if rel.is_empty() { None } else { Some(rel) };
        let mut entries = Vec::new();
        for (entry_path, node) in &self.file_structure {
            if Self::parent(entry_path) == parent {
                entries.push(node.metadata.clone());
            }
        }

        Ok(entries)
    }
    fn rm(&mut self, _path: &str) -> Result<(), ()> {
        Err(())
    }

    fn touch(&mut self, _parent_path: &str, _filename: &str) -> Result<(), ()> {
        Err(())
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        if Self::is_root(path) {
            return Ok(self.root_metadata());
        }

        let rel = path.trim_matches('/');
        if rel.is_empty() {
            return Err(());
        }

        if let Some((_, node)) = self
            .file_structure
            .iter()
            .find(|(p, _)| p.as_str() == rel)
        {
            return Ok(node.metadata.clone());
        }

        Err(())
    }
}

impl DevFs {
    fn parent(path: &str) -> Option<&str> {
        path.rsplit_once('/').and_then(|(parent, _)| {
            if parent.is_empty() {
                None
            } else {
                Some(parent)
            }
        })
    }

    fn is_directory(&self, path: &str) -> bool {
        if path.is_empty() {
            return true;
        }

        self.file_structure.iter().any(|(p, node)| {
            p.as_str() == path && node.metadata.file_type == FileType::Dir
        })
    }
}
