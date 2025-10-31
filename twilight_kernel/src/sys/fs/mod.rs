pub mod twilight_fs;
pub mod ram_fs;
pub mod vfs;
mod devfs;
mod gdt;

use crate::sys::fs::devfs::DevFs;
use crate::sys::fs::twilight_fs::TwilightFs;
use crate::sys::fs::vfs::VFS;
use crate::println;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use conquer_once::spin::OnceCell;
use spin::Mutex;

pub static MFS: OnceCell<Mutex<TwilightFs>> = OnceCell::uninit();

pub const KERNEL_PADDING: usize = 4 * 1024 * 1024;

pub const FS_PADDING: usize = 2097152;

#[derive(Debug)]
pub enum VfsError {
    NotFound,
    PermissionDenied,
    InvalidOperation,
    IoError,
}

pub fn init(show_log: bool) {
    let uptime = crate::driver::timer::pit::uptime();
    for bus in 0..2 {
        for dsk in 0..2 {
            if let Ok(mfs) = TwilightFs::check_ata(bus, dsk) {
                if let Err(_) = MFS.try_init_once(|| Mutex::new(mfs)) {
                    println!("MFS already initialized");
                    return;
                }
                #[allow(static_mut_refs)]
                unsafe {
                    VFS.get_mut().mount("/", Arc::new(Mutex::new(TwilightFs::check_ata(bus, dsk).unwrap())));
                }
                #[allow(static_mut_refs)]
                unsafe {
                    VFS.get_mut().mount("/dev", Arc::new(Mutex::new(DevFs::new())));
                }
                if show_log {
                    println!(
                        "\x1b[93m[{:.6}]\x1b[0m TwilightFS Superblock found in ATA {}:{}",
                        uptime, bus, dsk
                    );
                }
                return;
            }
        }
    }
    #[allow(static_mut_refs)]
    unsafe {
        VFS.get_mut().mount("/dev", Arc::new(Mutex::new(DevFs::new())));
    }
    println!("\x1b[93m[{:.6}]\x1b[0m No TwilightFS Superblock found", uptime);
    println!("\x1b[93mWarning\x1b[0m Trying running 'install' to install Twilight OS");
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
