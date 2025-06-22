pub mod ram_fs;
pub mod minixfs;

use crate::sys::fs::ram_fs::RamFS;
use alloc::string::String;
use alloc::vec::Vec;
use conquer_once::spin::OnceCell;
use spin::Mutex;
use crate::println;
use crate::sys::fs::minixfs::{MinixFs};

pub static FS: OnceCell<Mutex<RamFS>> = OnceCell::uninit();
pub static MFS: OnceCell<Mutex<MinixFs>> = OnceCell::uninit();

#[derive(Debug)]
pub enum VfsError {
    NotFound,
    PermissionDenied,
    InvalidOperation,
    IoError,
}

pub fn init_fs() {
    FS.try_init_once(|| Mutex::new(RamFS::new())).unwrap();
}

pub fn init(show_log: bool) {
    let uptime = crate::driver::timer::pit::uptime();
    for bus in 0..2 {
        for dsk in 0..2 {
            if let Ok(mfs) =  MinixFs::check_ata(bus, dsk) {
                MFS.try_init_once(|| Mutex::new(mfs)).unwrap();
                if show_log {
                    println!("\x1b[93m[{:.6}]\x1b[0m MinixFS Superblock found in ATA {}:{}", uptime, bus, dsk);
                }
                return;
            }
        }
    }
    println!("\x1b[93m[{:.6}]\x1b[0m No MinixFS Superblock found", uptime);
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
        fs.write(inode, offset, buffer).ok()
    } else {
        None
    }
}

pub fn create(path: &str) -> Option<u64> {
    let mut fs = FS.try_get().unwrap().lock();

    fs.create(path).ok()
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
        fs.readdir(inode).ok()
    } else {
        None
    }
}