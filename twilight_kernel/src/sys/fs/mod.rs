mod devfs;
mod gdt;
pub mod partition;
pub mod ram_fs;
pub mod twilight_fs;
pub mod vfs;
pub mod fat32;
pub mod fat16;

use crate::println;
use crate::sys::fs::devfs::DevFs;
use crate::sys::fs::fat16::{detect_fat16_partition, Fat16Fs};
use crate::sys::fs::twilight_fs::{TfsProxy, TwilightFs};
use crate::sys::fs::vfs::VFS;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use conquer_once::spin::OnceCell;
use spin::Mutex;
use crate::sys::fs::ram_fs::InitramfsFs;

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
            try_mount_boot(bus, dsk, show_log);

            if let Ok(mfs) = TwilightFs::check_ata(bus, dsk) {
                if let Err(_) = MFS.try_init_once(|| Mutex::new(mfs)) {
                    println!("MFS already initialized");
                    return;
                }
                #[allow(static_mut_refs)]
                unsafe {
                    VFS.get_mut().mount(
                        "/",
                        Arc::new(Mutex::new(TwilightFs::check_ata(bus, dsk).unwrap())),
                    );
                }
                #[allow(static_mut_refs)]
                unsafe {
                    VFS.get_mut()
                        .mount("/dev", Arc::new(Mutex::new(DevFs::new())));
                }
                try_mount_boot(bus, dsk, show_log);
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

    if let Ok(tfs) = TwilightFs::check_virtio_blk() {
        if let Err(_) = MFS.try_init_once(|| Mutex::new(tfs)) {
            println!("MFS already initialized");
            return;
        }
        #[allow(static_mut_refs)]
        unsafe {
            VFS.get_mut().unmount("/");
        }
        #[allow(static_mut_refs)]
        unsafe {
            VFS.get_mut().mount(
                "/",
                Arc::new(Mutex::new(TfsProxy)),
            )
        }
        #[allow(static_mut_refs)]
        unsafe {
            VFS.get_mut()
                .mount("/dev", Arc::new(Mutex::new(DevFs::new())));
        }
        if show_log {
            println!(
                "\x1b[93m[{:.6}]\x1b[0m TwilightFS Superblock found in Virtio Block Device",
                uptime
            );
        }
        return;
    }

    #[allow(static_mut_refs)]
    unsafe {
        VFS.get_mut()
            .mount("/dev", Arc::new(Mutex::new(DevFs::new())));
    }
    println!(
        "\x1b[93m[{:.6}]\x1b[0m No TwilightFS Superblock found",
        uptime
    );
    println!("\x1b[93mWarning\x1b[0m Trying running 'install' to install Twilight OS");
    // because harddisk does not have a file system use rootfs
    // TODO: this is messy fix it later
    try_mount_rootfs();
}

fn try_mount_rootfs() {
    #[allow(static_mut_refs)]
    unsafe { VFS.get_mut().mount("/", Arc::new(Mutex::new(InitramfsFs::new()))) };
    // InitramfsFs::new();
}

fn try_mount_boot(bus: u8, dsk: u8, show_log: bool) -> bool {
    #[allow(static_mut_refs)]
    let already = unsafe {
        VFS.get_mut()
            .mount_points
            .iter()
            .any(|(prefix, _)| *prefix == "/boot")
    };
    if already {
        return true;
    }

    let Some(entry) = detect_fat16_partition(bus, dsk) else {
        return false;
    };
    let entry_lba = entry.lba_start;

    match Fat16Fs::from_partition(bus, dsk, entry) {
        Ok(fs) => {
            #[allow(static_mut_refs)]
            unsafe {
                VFS.get_mut().mount("/boot", Arc::new(Mutex::new(fs)));
            }
            if show_log {
                println!(
                    "\x1b[93m[{:.6}]\x1b[0m FAT16 partition mounted at /boot from ATA {}:{} (LBA {})",
                    crate::driver::timer::pit::uptime(),
                    bus,
                    dsk,
                    entry_lba
                );
            }
            true
        }
        Err(_) => false,
    }
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
