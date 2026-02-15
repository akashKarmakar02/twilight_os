mod devfs;
pub mod fat16;
pub mod fat32;
mod gdt;
pub mod partition;
pub mod pipe;
mod procfs;
pub mod ram_fs;
pub mod twilight_fs;
pub mod vfs;
pub mod mbr;

use crate::driver::disk::USB_BLOCK_DEVICE;
use crate::println;
use crate::sys::fs::devfs::DevFs;
use crate::sys::fs::fat16::{Fat16Fs, detect_fat16_partition};
use crate::sys::fs::procfs::ProcFs;
use crate::sys::fs::ram_fs::InitramfsFs;
use crate::sys::fs::twilight_fs::{
    TfsProxy, TwilightFs, fs_block_offset_bytes, set_fs_block_offset_bytes,
};
use crate::sys::fs::vfs::VFS;
use crate::sys::fs::vfs::{FileSystem, Metadata, VfsNode};
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

struct OffsetScopedTwilightFs {
    fs: TwilightFs,
    offset_bytes: usize,
}

impl OffsetScopedTwilightFs {
    fn run_with_offset<T>(
        &mut self,
        f: impl FnOnce(&mut TwilightFs) -> Result<T, ()>,
    ) -> Result<T, ()> {
        let old = fs_block_offset_bytes();
        set_fs_block_offset_bytes(self.offset_bytes);
        let out = f(&mut self.fs);
        set_fs_block_offset_bytes(old);
        out
    }
}

impl FileSystem for OffsetScopedTwilightFs {
    fn open(&mut self, path: &str) -> Result<VfsNode, ()> {
        self.run_with_offset(|fs| fs.open(path))
    }

    fn mkdir(&mut self, parent_dir: &str, path: &str) -> Result<(), ()> {
        self.run_with_offset(|fs| fs.mkdir(parent_dir, path))
    }

    fn rmdir(&mut self, path: &str) -> Result<(), ()> {
        self.run_with_offset(|fs| fs.rmdir(path))
    }

    fn ls(&mut self, path: &str) -> Result<Vec<Metadata>, ()> {
        self.run_with_offset(|fs| fs.ls(path))
    }

    fn rm(&mut self, path: &str) -> Result<(), ()> {
        self.run_with_offset(|fs| fs.rm(path))
    }

    fn touch(&mut self, parent_path: &str, filename: &str) -> Result<(), ()> {
        self.run_with_offset(|fs| fs.touch(parent_path, filename))
    }

    fn metadata(&mut self, path: &str) -> Result<Metadata, ()> {
        self.run_with_offset(|fs| fs.metadata(path))
    }
}

pub fn init(show_log: bool) {
    let uptime = crate::driver::timer::pit::uptime();
    for _ in 0..16 {
        #[allow(static_mut_refs)]
        let usb_ready = unsafe { USB_BLOCK_DEVICE.is_some() };
        if usb_ready {
            break;
        }
        crate::driver::usb::poll_all_drivers();
    }

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
                #[allow(static_mut_refs)]
                unsafe {
                    VFS.get_mut()
                        .mount("/proc", Arc::new(Mutex::new(ProcFs::new())));
                }
                try_init_usb_storage(show_log);
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
            VFS.get_mut().mount("/", Arc::new(Mutex::new(TfsProxy)))
        }
        #[allow(static_mut_refs)]
        unsafe {
            VFS.get_mut()
                .mount("/dev", Arc::new(Mutex::new(DevFs::new())));
        }
        #[allow(static_mut_refs)]
        unsafe {
            VFS.get_mut()
                .mount("/proc", Arc::new(Mutex::new(ProcFs::new())));
        }
        try_init_usb_storage(show_log);
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
    #[allow(static_mut_refs)]
    unsafe {
        VFS.get_mut()
            .mount("/proc", Arc::new(Mutex::new(ProcFs::new())));
    }
    try_init_usb_storage(show_log);
    println!(
        "\x1b[93m[{:.6}]\x1b[0m No TwilightFS Superblock found",
        uptime
    );
    println!("\x1b[93mWarning\x1b[0m Trying running 'install' to install Twilight OS");
    // because harddisk does not have a file system use rootfs
    // TODO: this is messy fix it later
    try_mount_rootfs();
}

fn try_init_usb_storage(show_log: bool) -> bool {
    #[allow(static_mut_refs)]
    let usb_ready = unsafe { USB_BLOCK_DEVICE.is_some() };
    if !usb_ready {
        return false;
    }

    let old_offset = fs_block_offset_bytes();
    let mut mount_target: Option<(TwilightFs, usize)> = None;

    let result = match TwilightFs::check_usb_blk() {
        Ok(fs) => {
            mount_target = Some((fs, fs_block_offset_bytes()));
            true
        }
        Err(_) => match TwilightFs::format_usb_blk() {
            Ok(fs) => {
                mount_target = Some((fs, fs_block_offset_bytes()));
                if show_log {
                    println!(
                        "\x1b[93m[{:.6}]\x1b[0m USB storage initialized with TwilightFS at /dev/disk1",
                        crate::driver::timer::pit::uptime()
                    );
                }
                true
            }
            Err(err) => {
                if show_log {
                    println!(
                        "\x1b[93m[{:.6}]\x1b[0m USB storage init skipped: {}",
                        crate::driver::timer::pit::uptime(),
                        err
                    );
                }
                false
            }
        },
    };

    set_fs_block_offset_bytes(old_offset);

    if let Some((fs, offset_bytes)) = mount_target.take() {
        #[allow(static_mut_refs)]
        let mounted = unsafe {
            VFS.get_mut()
                .mount_points
                .iter()
                .any(|(prefix, _)| *prefix == "/mnt/usb")
        };

        if !mounted {
            #[allow(static_mut_refs)]
            unsafe {
                if VFS.get_mut().metadata("/mnt").is_err() {
                    let _ = VFS.get_mut().mkdir("/", "mnt");
                }
                if VFS.get_mut().metadata("/mnt/usb").is_err() {
                    let _ = VFS.get_mut().mkdir("/mnt", "usb");
                }
                VFS.get_mut().mount(
                    "/mnt/usb",
                    Arc::new(Mutex::new(OffsetScopedTwilightFs { fs, offset_bytes })),
                );
            }
        }

        if show_log {
            println!(
                "\x1b[93m[{:.6}]\x1b[0m USB storage mounted at /mnt/usb",
                crate::driver::timer::pit::uptime()
            );
        }
    };

    result
}

fn try_mount_rootfs() {
    #[allow(static_mut_refs)]
    unsafe {
        VFS.get_mut()
            .mount("/", Arc::new(Mutex::new(InitramfsFs::new())))
    };
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
