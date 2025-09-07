use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::DerefMut;
use spin::mutex::Mutex;
use spin::rwlock::RwLock;
use crate::driver::disk::BlockDeviceIO;

#[allow(dead_code)]
pub static mut VFS: RwLock<Vfs> = RwLock::new(Vfs::new());

#[derive(Debug, Clone, Copy)]
pub enum FileType {
    File,
    Dir,
}

#[derive(Debug)]
pub enum VfsError {
    NotFound,
    NotDir,
    AlreadyExists,
    Io,
    Invalid,
}

#[derive(Debug, Clone, Copy)]
pub struct Metadata {
    pub file_type: FileType,
    pub size: usize,
}

#[allow(dead_code)]
pub struct VfsNode {
    device: Arc<Mutex<dyn BlockDeviceIO>>,
    metadata: Metadata,
    node: Box<dyn VfsNodeOps>,
}

impl VfsNode {
    pub fn new(device: Arc<Mutex<dyn BlockDeviceIO>>, metadata: Metadata, node: Box<dyn VfsNodeOps>) -> Self {
        Self { device, metadata, node }
    }

    pub fn read(&mut self) -> Result<Vec<u8>, ()> {
        self.node.read(self.device.lock().deref_mut())
    }

    pub fn write(&mut self, data: &[u8]) -> Result<(), ()> {
        self.node.write(self.device.lock().deref_mut(),data)
    }
}

pub trait VfsNodeOps: Send + Sync + 'static {
    fn read(&self, device: &mut dyn BlockDeviceIO) -> Result<Vec<u8>, ()>;
    fn write(&self, device: &mut dyn BlockDeviceIO, data: &[u8]) -> Result<(), ()>;
}

pub trait FileSystem: Send + Sync + 'static {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()>;
    fn read(&mut self, path: &str) -> Result<Vec<u8>, ()>;
    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), ()>;
    fn mkdir(&mut self, parent_dir: &str, path: &str) -> Result<(), ()>;
    fn rmdir(&mut self, path: &str) -> Result<(), ()>;
    fn ls(&mut self, path: &str) -> Result<Vec<String>, ()>;
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
    pub const fn new() -> Self { Self { mount_points: Vec::new() } }

    pub fn mount(&mut self, prefix: &'static str, fs: Arc<Mutex<dyn FileSystem>>) {
        self.mount_points.push((prefix, fs));
        self.mount_points.sort_by(|(a,_),(b,_)| b.len().cmp(&a.len()));
    }

    pub fn unmount(&mut self, prefix: &str) -> bool {
        if let Some(i) = self.mount_points.iter().position(|(p,_)| *p == prefix) {
            self.mount_points.remove(i); true
        } else { false }
    }

    #[inline]
    fn route<'a>(&self, path: &'a str) -> Option<(&'a str, &Arc<Mutex<dyn FileSystem>>)> {
        self.mount_points.iter()
            .find(|(p, _)| path.starts_with(*p))
            .map(|(prefix, fs)| {
                let rel = &path[prefix.len()..];
                (if rel.is_empty() { "/" } else { rel }, fs)
            })
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.read(rel)
    }

    pub fn write(&self, path: &str, data: &[u8]) -> Result<(), ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.write(rel, data)
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

    pub fn ls(&self, path: &str) -> Result<Vec<String>, ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.ls(rel)
    }

    pub fn rm(&self, path: &str) -> Result<(), ()> {
        let (rel, fs) = self.route(path).ok_or(())?;
        let mut guard = fs.lock();
        guard.rm(rel)
    }

    pub fn touch(&self, parent_path: &str, filename: &str) -> Result<(), ()> {
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
