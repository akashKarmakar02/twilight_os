
pub mod ram_fs;
use alloc::vec::Vec;
use alloc::string::String;
use conquer_once::spin::OnceCell;
use spin::Mutex;
use crate::fs::ram_fs::RamFS;

pub static FS: OnceCell<Mutex<RamFS>>= OnceCell::uninit();

#[derive(Debug)]
pub enum VfsError {
    NotFound,
    PermissionDenied,
    InvalidOperation,
    IoError,
}

pub fn init_fs() {
    FS.try_init_once(|| {
        Mutex::new(RamFS::new())
    }).unwrap()
}

pub trait VfsNode {
    fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError>;

    fn write(&mut self, offset: u64, buffer: &[u8]) -> Result<usize, VfsError>;

    fn size(&self) -> u64;

    fn is_directory(&self) -> bool;
}


pub trait Vfs {
    fn read(&self, inode: u64, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError>;
    fn write(&mut self, inode: u64, offset: u64, buffer: &[u8]) -> Result<usize, VfsError>;
    fn open(&self, path: &str) -> Result<u64, VfsError>;
    fn close(&self, path: &str) -> Result<(), VfsError>;
    fn create(&mut self, path: &str) -> Result<u64, VfsError>;
    fn delete(&mut self, path: &str) -> Result<(), VfsError>;
    fn readdir(&self, inode: u64) -> Result<Vec<String>, VfsError>;
    fn mount(&mut self, device: &str) -> Result<(), VfsError>;
    fn unmount(&mut self, path: &str) -> Result<(), VfsError>;
}
