mod null;

use crate::driver::disk::dummy_blockdev;
use crate::fs::vfs::Metadata;
use crate::sys::fs::devfs::null::Null;
use crate::sys::fs::vfs::{BlockDev, FileSystem, VfsNode, VfsNodeOps};
use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::rwlock::RwLock;

pub struct DevFs {
    file_structure: Vec<VfsNode>,
}

impl DevFs {
    pub fn new() -> Self {
        let mut devices = Vec::new();
        let null_meta = Metadata::chr(2, "null");
        devices.push(VfsNode::new(dummy_blockdev(), null_meta, Arc::new(RwLock::new(Null))));

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
    fn read(&self, _device: &mut BlockDev, _lba: usize, _buf: &mut [u8]) -> Result<Vec<u8>, ()> {
        Err(())
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&self, _device: &mut BlockDev, _cmd: u32, _arg: usize) -> Result<i64, ()> {
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
            return Ok(VfsNode::new(dummy_blockdev(), meta, Arc::new(RwLock::new(DirNodeOps))));
        }

        for dev in &self.file_structure {
            if format!("/{}", dev.metadata.name).as_str() == path.to_string() {
                return Ok(VfsNode{ device: dev.device.clone(), metadata: dev.metadata.clone(), node: dev.node.clone() });
            }
        }

        Err(())
    }

    fn mkdir(&mut self, _parent_dir: &str, _path: &str) -> Result<(), ()> { Err(()) }
    fn rmdir(&mut self, _path: &str) -> Result<(), ()> { Err(()) }
    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()> {
        if Self::is_root(path) {
            let mut devices = Vec::new();
            for dev in &self.file_structure {
                devices.push(dev.metadata.clone());
            }
            Ok(devices)
        } else {
            Err(())
        }
    }
    fn rm(&mut self, _path: &str) -> Result<(), ()> { Err(()) }

    fn touch(&mut self, _parent_path: &str, _filename: &str) -> Result<(), ()> { Err(()) }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        if Self::is_root(path) {
            return Ok(self.root_metadata());
        }

        for dev in &self.file_structure {
            if dev.metadata.name == path.to_string() {
                return Ok(dev.metadata.clone());
            }
        }

        Err(())
    }
}
