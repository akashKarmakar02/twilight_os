use crate::driver::disk::BlockDeviceIO;
use crate::sys::fs::twilight_fs::TfsError;
use crate::sys::fs::twilight_fs::inode::Inode;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::mutex::Mutex;
use spin::rwlock::RwLock;

pub static mut VFS: RwLock<Vfs> = RwLock::new(Vfs::new());

#[derive(Debug, Clone, Copy)]
pub enum FileType {
    File,
    Dir,
    CharDevice,
    BlockDevice,
}

impl PartialEq for FileType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FileType::File, FileType::File) => true,
            (FileType::Dir, FileType::Dir) => true,
            _ => false,
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
#[repr(u16)]
pub enum VfsError {
    NotFound,
    NotDir,
    AlreadyExists,
    Io,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct Metadata {
    pub ino: u32,
    pub name: String,
    pub file_type: FileType,
    pub size: usize,
    pub created_time: u32,
    pub access_time: u32,
    pub modified_time: u32,
}

impl Metadata {
    pub(crate) fn dir(ino: u32, name: &str) -> Self {
        Metadata {
            ino,
            name: name.into(),
            file_type: FileType::Dir,
            size: 0,
            access_time: 0,
            created_time: 0,
            modified_time: 0,
        }
    }
    pub(crate) fn chr(ino: u32, name: &str) -> Self {
        Metadata {
            ino,
            name: name.into(),
            file_type: FileType::CharDevice,
            size: 0,
            access_time: 0,
            created_time: 0,
            modified_time: 0,
        }
    }
}
pub type BlockDev = Arc<Mutex<Box<dyn BlockDeviceIO + Send>>>;

pub trait FsCtx {
    fn block_size(&self) -> usize;

    fn read_block(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()>;
    fn write_block(&mut self, lba: u32, buf: &[u8]) -> Result<(), ()>;

    fn alloc_zone(&mut self) -> Result<u32, TfsError>;
    fn free_zone(&mut self, zone: u32) -> Result<(), TfsError>;
    fn write_inode_twilight(&mut self, ino: u32, inode: Inode) -> Result<(), &'static str>;

    fn remove_file(&mut self, path: &str) -> Result<(), ()>;
}

#[allow(dead_code)]
pub struct VfsNode {
    pub device: BlockDev,
    pub metadata: Metadata,
    pub node: Arc<RwLock<dyn VfsNodeOps>>,
}

impl VfsNode {
    pub fn new(device: BlockDev, metadata: Metadata, node: Arc<RwLock<dyn VfsNodeOps>>) -> Self {
        Self {
            device,
            metadata,
            node,
        }
    }

    pub fn read(&mut self, lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        self.node.read().read(&mut self.device, lba, buf)
    }

    pub fn write(&mut self, lba: usize, data: &[u8]) -> Result<(), ()> {
        self.node.write().write(&mut self.device, lba, data)
    }

    pub fn poll(&mut self) -> Result<bool, ()> {
        self.node.read().poll(&mut self.device)
    }

    pub fn unlink(&mut self) -> Result<i32, ()> {
        self.node.write().unlink(&mut self.device)
    }

    pub fn ioctl(&mut self, cmd: u64, arg: usize) -> Result<i64, ()> {
        self.node.write().ioctl(&mut self.device, cmd, arg)
    }
}

impl Clone for VfsNode {
    fn clone(&self) -> Self {
        Self {
            device: self.device.clone(),
            metadata: self.metadata.clone(),
            node: self.node.clone(),
        }
    }
}

pub trait VfsNodeOps: Send + Sync + 'static {
    fn read(&self, device: &mut BlockDev, lba: usize, buf: &mut [u8]) -> Result<usize, ()>;
    fn write(&mut self, device: &mut BlockDev, lba: usize, data: &[u8]) -> Result<(), ()>;
    fn poll(&self, device: &mut BlockDev) -> Result<bool, ()>;
    fn ioctl(&mut self, device: &mut BlockDev, cmd: u64, arg: usize) -> Result<i64, ()>;
    fn unlink(&mut self, device: &mut BlockDev) -> Result<i32, ()>;
}

pub trait FileSystem: Send + Sync + 'static {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()>;
    fn mkdir(&mut self, parent_dir: &str, path: &str) -> Result<(), ()>;
    fn rmdir(&mut self, path: &str) -> Result<(), ()>;
    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()>;
    fn rm(&mut self, path: &str) -> Result<(), ()>;
    fn touch(&mut self, parent_path: &str, filename: &str) -> Result<(), ()>;
    fn metadata(&mut self, path: &str) -> Result<Metadata, ()>;
}

pub struct Vfs {
    pub mount_points: Vec<(&'static str, Arc<Mutex<dyn FileSystem>>)>,
}

unsafe impl Send for Vfs {}
unsafe impl Sync for Vfs {}

#[allow(dead_code)]
impl Vfs {
    pub const fn new() -> Self {
        Self {
            mount_points: Vec::new(),
        }
    }

    pub fn mount(&mut self, prefix: &'static str, fs: Arc<Mutex<dyn FileSystem>>) {
        self.mount_points.push((prefix, fs));
        self.mount_points
            .sort_by(|(a, _), (b, _)| b.len().cmp(&a.len()));
    }

    pub fn unmount(&mut self, prefix: &str) -> bool {
        if let Some(i) = self.mount_points.iter().position(|(p, _)| *p == prefix) {
            self.mount_points.remove(i);
            true
        } else {
            false
        }
    }

    #[inline]
    fn route<'a>(&self, path: &'a str) -> Option<(&'a str, &Arc<Mutex<dyn FileSystem>>)> {
        self.mount_points
            .iter()
            .find(|(p, _)| path.starts_with(*p))
            .map(|(prefix, fs)| {
                let rel = &path[prefix.len()..];
                (if rel.is_empty() { "/" } else { rel }, fs)
            })
    }

    pub fn open(&self, path: &str) -> Result<VfsNode, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.open(rel)
    }

    pub fn mkdir(&self, parent_path: &str, path: &str) -> Result<(), ()> {
        let (rel, fs) = self.route(parent_path).ok_or(())?;
        let mut guard = fs.lock();
        guard.mkdir(rel, path)
    }

    pub fn rmdir(&self, path: &str) -> Result<(), ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.rmdir(rel)
    }

    pub fn ls(&self, path: &str) -> Result<Vec<Metadata>, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.ls(rel)
    }

    pub fn rm(&self, path: &str) -> Result<(), ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.rm(rel)
    }

    pub fn touch(&self, parent_path: &str, filename: &str, _mode: u32) -> Result<(), ()> {
        let (rel, fs) = self.route(parent_path).ok_or(())?;
        let mut guard = fs.lock();
        guard.touch(rel, filename)
    }

    pub fn metadata(&self, path: &str) -> Result<Metadata, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.metadata(rel)
    }
}
