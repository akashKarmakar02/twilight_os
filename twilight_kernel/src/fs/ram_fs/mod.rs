use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use crate::framebuffer::{get_framebuffer};
use crate::fs::{Vfs, VfsError};

static NEXT_INODE: AtomicU64 = AtomicU64::new(2);

pub struct RamFS {
    inodes: BTreeMap<u64, RamFSNode>,
    paths: BTreeMap<String, u64>,
}


pub enum RamFSNode {
    File { inode: u64, data: Vec<u8> },
    Directory { inode: u64, children: BTreeMap<String, RamFSNode> },
    VfsNode { inode: u64, dev: Arc<Mutex<dyn super::VfsNode + Send + Sync>> },
}

impl RamFS {
    pub fn new() -> Self {
        let mut inodes = BTreeMap::new();
        let root_inode = 1;
        let dev_inode = NEXT_INODE.fetch_add(1, Ordering::SeqCst);

        inodes.insert(root_inode, RamFSNode::Directory { inode: root_inode, children: BTreeMap::new() });

        inodes.insert(dev_inode, RamFSNode::Directory { inode: dev_inode, children: BTreeMap::new() });

        if let Some(RamFSNode::Directory { children, .. }) = inodes.get_mut(&root_inode) {
            children.insert("dev".to_string(), RamFSNode::Directory { inode: dev_inode, children: BTreeMap::new() });
        }

        let mut paths = BTreeMap::from([
            ("/".to_string(), root_inode),
            ("/dev".to_string(), dev_inode),
        ]);

        let devices: &[(&str, Arc<Mutex<dyn super::VfsNode + Send + Sync>>)] = &[
            ("fb0", Arc::new(Mutex::new(get_framebuffer().clone()))),
        ];

        for (name, dev) in devices {
            let inode = NEXT_INODE.fetch_add(1, Ordering::SeqCst);
            inodes.insert(inode, RamFSNode::VfsNode { inode, dev: dev.clone() });
            paths.insert(format!("/dev/{name}"), inode);

            if let Some(RamFSNode::Directory { children, .. }) = inodes.get_mut(&dev_inode) {
                children.insert(name.to_string(), RamFSNode::VfsNode { inode, dev: dev.clone() });
            }
        }

        Self { inodes, paths }
    }

    fn get_inode(&self, path: &str) -> Option<u64> {
        self.paths.get(path).cloned()
    }
}

impl Vfs for RamFS {
    fn read(&self, inode: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        match self.inodes.get(&inode) {
            Some(RamFSNode::File { data,  .. }) => {
                let len = buffer.len().min(data.len() - offset as usize);
                buffer[..len].copy_from_slice(&data[offset as usize..len+ offset as usize]);
                Ok(len)
            }
            _ => Err(VfsError::NotFound),
        }
    }

    fn write(&mut self, inode: u64, offset: u64, buffer: &[u8]) -> Result<usize, VfsError> {

        match self.inodes.get_mut(&inode.clone()) {
            Some(RamFSNode::VfsNode{ dev, .. }) => {
                dev.lock().write(offset, buffer)
            }
            Some(RamFSNode::File { data,  .. }) => {
                let new_len = offset as usize + buffer.len();
                if data.len() < new_len {
                    data.resize(new_len, 0);
                }

                data[offset as usize..offset as usize+buffer.len()].copy_from_slice(buffer);
                Ok(buffer.len())
            }
            _ => Err(VfsError::NotFound),
        }
    }

    fn open(&self, path: &str) -> Result<u64, VfsError> {
        self.get_inode(path).ok_or(VfsError::NotFound)
    }

    fn close(&self, _path: &str) -> Result<(), VfsError> {
        Ok(())
    }

    fn create(&mut self, path: &str) -> Result<u64, VfsError> {
        if let Some(_inode) = self.get_inode(path) {
            return Err(VfsError::InvalidOperation);
        }


        let inode = NEXT_INODE.fetch_add(1, Ordering::SeqCst);
        let root_inode = 1;
        let node = RamFSNode::File { inode, data: Vec::new() };
        self.inodes.insert(inode, RamFSNode::File { inode, data: Vec::new() });
        if path.split("/").filter(|x| !x.is_empty()).count() == 1 || !path.starts_with("/") {
            match self.inodes.get_mut(&root_inode) {
                Some(RamFSNode::Directory { children, .. }) => {
                    children.insert(String::from(path), node);
                }
                _ => {}
            }
        }
        self.paths.insert(path.to_string(), inode);
        Ok(inode)
    }

    fn delete(&mut self, path: &str) -> Result<(), VfsError> {
        if let Some(inode) = self.paths.remove(path) {
            self.inodes.remove(&inode);
            Ok(())
        } else {
            Err(VfsError::NotFound)
        }
    }

    fn readdir(&self, inode: u64) -> Result<Vec<String>, VfsError> {
        match self.inodes.get(&inode) {
            Some(RamFSNode::Directory { children, .. }) => Ok(children.keys().cloned().collect()),
            _ => Err(VfsError::NotFound),
        }
    }

    fn mount(&mut self, _device: &str) -> Result<(), VfsError> {
        Ok(())
    }

    fn unmount(&mut self, _path: &str) -> Result<(), VfsError> {
        Ok(())
    }
}
