pub mod ram_fs;
pub mod minixfs;

use crate::fs::ram_fs::RamFS;
use alloc::string::String;
use alloc::vec::Vec;
use conquer_once::spin::OnceCell;
use spin::Mutex;

pub(crate) static FS: OnceCell<Mutex<RamFS>> = OnceCell::uninit();

#[derive(Debug)]
pub enum VfsError {
    NotFound,
    PermissionDenied,
    InvalidOperation,
    IoError,
}

pub fn init_fs() {
    FS.try_init_once(|| Mutex::new(RamFS::new())).unwrap()
}

pub trait VfsNode {
    fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError>;

    fn write(&mut self, offset: u64, buffer: &[u8]) -> Result<usize, VfsError>;

    fn size(&self) -> u64;

    fn is_directory(&self) -> bool;
}

pub trait Vfs: Send {
    fn read(&self, inode: u64, offset: u64) -> Result<&[u8], VfsError>;
    fn write(&mut self, inode: u64, offset: u64, buffer: &[u8]) -> Result<usize, VfsError>;
    fn open(&self, path: &str) -> Result<u64, VfsError>;
    fn close(&self, path: &str) -> Result<(), VfsError>;
    fn create(&mut self, path: &str) -> Result<u64, VfsError>;
    fn delete(&mut self, path: &str) -> Result<(), VfsError>;
    fn readdir(&self, inode: u64) -> Result<Vec<String>, VfsError>;
    fn mount(&mut self, device: &str) -> Result<(), VfsError>;
    fn unmount(&mut self, path: &str) -> Result<(), VfsError>;
}


pub fn read(path: &str, offset: u64) -> Option<Vec<u8>> {
    let fs = FS.try_get().unwrap().lock();

    if let Ok(inode) = fs.open(path) {
        if let Ok(bytes) = fs.read(inode, offset) {
            Some(Vec::from(bytes))
        } else {
            None
        }
    } else {
        None
    }
}


pub fn write(path: &str, offset: u64, buffer: &[u8]) -> Option<usize> {
    let mut fs = FS.try_get().unwrap().lock();

    if let Ok(inode) = fs.open(path) {
        if let Ok(bytes) = fs.write(inode, offset, buffer) {
            Some(bytes)
        } else {
            None
        }
    } else {
        None
    }
}

pub fn create(path: &str) -> Option<u64> {
    let mut fs = FS.try_get().unwrap().lock();

    if let Ok(inode) = fs.create(path) {
        Some(inode)
    } else {
        None
    }
}

pub fn delete(path: &str) -> Option<()> {
    let mut fs = FS.try_get().unwrap().lock();

    if let Ok(()) = fs.delete(path) {
        Some(())
    } else {
        None
    }
}

pub fn readdir(path: &str) -> Option<Vec<String>> {
    let fs = FS.try_get().unwrap().lock();

    if let Ok(inode) = fs.open(path) {
        if let Ok(files) = fs.readdir(inode) {
            Some(files)
        } else {
            None
        }
    } else {
        None
    }
}