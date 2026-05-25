use crate::driver::disk::dummy_blockdev;
use crate::sys::fs::vfs::{BlockDev, FileSystem, FileType, Metadata, VfsNode, VfsNodeOps};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::rwlock::RwLock;

mod nodes;

pub struct ProcFs {
    file_structure: Vec<(String, VfsNode)>,
}

impl ProcFs {
    pub fn new() -> Self {
        let mut files = Vec::new();

        let proc_dir = Metadata::dir(1000, "");
        files.push((
            "".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                proc_dir,
                Arc::new(RwLock::new(DirNodeOps)),
            ),
        ));

        let cpuinfo_meta = Metadata {
            ino: 1001,
            name: "cpuinfo".into(),
            file_type: FileType::File,
            size: 0,
            uid: 0,
            gid: 0,
            created_time: 0,
            access_time: 0,
            modified_time: 0,
        };
        files.push((
            "cpuinfo".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                cpuinfo_meta,
                Arc::new(RwLock::new(nodes::CpuInfoNode)),
            ),
        ));

        let meminfo_meta = Metadata {
            ino: 1002,
            name: "meminfo".into(),
            file_type: FileType::File,
            size: 0,
            uid: 0,
            gid: 0,
            created_time: 0,
            access_time: 0,
            modified_time: 0,
        };
        files.push((
            "meminfo".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                meminfo_meta,
                Arc::new(RwLock::new(nodes::MemInfoNode)),
            ),
        ));

        let uptime_meta = Metadata {
            ino: 1003,
            name: "uptime".into(),
            file_type: FileType::File,
            uid: 0,
            gid: 0,
            size: 0,
            created_time: 0,
            access_time: 0,
            modified_time: 0,
        };
        files.push((
            "uptime".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                uptime_meta,
                Arc::new(RwLock::new(nodes::UptimeNode)),
            ),
        ));

        let version_meta = Metadata {
            ino: 1004,
            name: "version".into(),
            file_type: FileType::File,
            uid: 0,
            gid: 0,
            size: 0,
            created_time: 0,
            access_time: 0,
            modified_time: 0,
        };
        files.push((
            "version".to_string(),
            VfsNode::new(
                dummy_blockdev(),
                version_meta,
                Arc::new(RwLock::new(nodes::VersionNode)),
            ),
        ));

        Self {
            file_structure: files,
        }
    }

    fn is_root(path: &str) -> bool {
        let p = path.trim_matches('/');
        p.is_empty()
    }

    fn root_metadata(&self) -> Metadata {
        Metadata::dir(1000, "")
    }

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
        self.file_structure
            .iter()
            .any(|(p, node)| p.as_str() == path && node.metadata.file_type == FileType::Dir)
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

impl FileSystem for ProcFs {
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

        if let Some((_, node)) = self.file_structure.iter().find(|(p, _)| p.as_str() == rel) {
            let mut out = node.clone();
            // Keep size updated for proc files.
            out.metadata.size = out
                .node
                .write()
                .ioctl(&mut out.device, nodes::IOCTL_PROC_GET_SIZE, 0)
                .ok()
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(0);
            return Ok(out);
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
            if Self::parent(entry_path) == parent && !entry_path.is_empty() {
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

        if let Some((_, node)) = self.file_structure.iter().find(|(p, _)| p.as_str() == rel) {
            let mut out = node.metadata.clone();
            // Update size dynamically.
            let mut dev = node.device.clone();
            let size = node
                .node
                .write()
                .ioctl(&mut dev, nodes::IOCTL_PROC_GET_SIZE, 0)
                .ok()
                .and_then(|v| usize::try_from(v).ok())
                .unwrap_or(0);
            out.size = size;
            return Ok(out);
        }

        Err(())
    }
}
