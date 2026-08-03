mod devfs;
pub mod fat16;
pub mod fat32;
mod gdt;
pub mod iso9660;
pub mod mbr;
pub mod memfd;
pub mod partition;
pub mod pipe;
mod procfs;
pub mod ram_fs;
pub mod twilight_fs;
pub mod vfs;

use crate::driver::disk::{FileBlockDevice, OpticalBlkHandle, USB_BLOCK_DEVICE, UsbBlkHandle};
use crate::println;
use crate::sys::fs::devfs::DevFs;
use crate::sys::fs::fat16::{Fat16Fs, detect_fat16_partition};
use crate::sys::fs::fat32::{Fat32Fs, detect_fat32_partition};
use crate::sys::fs::iso9660::{Iso9660Fs, boxed_device};
use crate::sys::fs::procfs::ProcFs;
use crate::sys::fs::ram_fs::InitramfsFs;
use crate::sys::fs::twilight_fs::{MountMode, TfsProxy, TwilightFs};
use crate::sys::fs::vfs::{FileSystem, VFS, VfsNode};
use crate::utils::sync::Mutex;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use conquer_once::TryInitError;
use conquer_once::spin::OnceCell;
use lazy_static::lazy_static;

pub static MFS: OnceCell<Mutex<TwilightFs>> = OnceCell::uninit();
lazy_static! {
    pub static ref LIVE_SYSTEM_FS: Mutex<Option<Arc<Mutex<TwilightFs>>>> = Mutex::new(None);
}

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
    let uptime = crate::driver::time::uptime_secs_f64();
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
                    VFS.get_mut().mount("/", Arc::new(Mutex::new(TfsProxy)));
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
        if let Err(e) = MFS.try_init_once(|| Mutex::new(tfs))
            && e != TryInitError::AlreadyInit
        {
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

    if try_mount_live_system(show_log) {
        mount_pseudo_filesystems(true);
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

    let mut mount_target: Option<TwilightFs> = None;

    let result = match TwilightFs::check_usb_blk() {
        Ok(fs) => {
            mount_target = Some(fs);
            true
        }
        Err(err) => {
            if show_log {
                println!(
                    "\x1b[93m[{:.6}]\x1b[0m USB media left unchanged: {}",
                    crate::driver::time::uptime_secs_f64(),
                    err
                );
            }
            false
        }
    };

    if let Some(fs) = mount_target.take() {
        #[allow(static_mut_refs)]
        let mounted = unsafe {
            VFS.get_mut()
                .mount_points
                .iter()
                .any(|mount| mount.prefix == "/mnt/usb")
        };

        if !mounted {
            #[allow(static_mut_refs)]
            unsafe {
                if VFS.get_mut().metadata("/mnt").is_err() {
                    let _ = VFS.get_mut().mkdir("/", "mnt", 0o755);
                }
                if VFS.get_mut().metadata("/mnt/usb").is_err() {
                    let _ = VFS.get_mut().mkdir("/mnt", "usb", 0o755);
                }
                VFS.get_mut().mount("/mnt/usb", Arc::new(Mutex::new(fs)));
            }
        }

        if show_log {
            println!(
                "\x1b[93m[{:.6}]\x1b[0m USB storage mounted at /mnt/usb",
                crate::driver::time::uptime_secs_f64()
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

fn try_mount_live_system(show_log: bool) -> bool {
    #[allow(static_mut_refs)]
    let boot_image = unsafe { VFS.get_mut().open("/boot/SYSTEM.TFS") };
    if let Ok(image) = boot_image {
        if try_mount_live_image(image, "/boot/SYSTEM.TFS", None, show_log) {
            return true;
        }
    }

    if try_mount_live_device(boxed_device(OpticalBlkHandle), "/dev/cdrom", show_log) {
        return true;
    }

    #[allow(static_mut_refs)]
    let usb_ready = unsafe { USB_BLOCK_DEVICE.is_some() };
    usb_ready && try_mount_live_device(boxed_device(UsbBlkHandle), "/dev/usb0", show_log)
}

fn try_mount_live_device(
    device: crate::sys::fs::vfs::BlockDev,
    source: &'static str,
    show_log: bool,
) -> bool {
    let mut iso = match Iso9660Fs::probe(device) {
        Ok(iso) => iso,
        Err(_) => return false,
    };
    let image = match iso.open("/SYSTEM.TFS") {
        Ok(image) => image,
        Err(_) => return false,
    };
    try_mount_live_image(image, source, Some(iso), show_log)
}

fn try_mount_live_image(
    image: VfsNode,
    source: &'static str,
    install_media: Option<Iso9660Fs>,
    show_log: bool,
) -> bool {
    let image_device = match FileBlockDevice::new(image, 512) {
        Ok(device) => device,
        Err(_) => return false,
    };
    let system = match TwilightFs::open(Box::new(image_device), MountMode::ReadOnly) {
        Ok(system) => system,
        Err(err) => {
            if show_log {
                println!("Live system image rejected: {}", err);
            }
            return false;
        }
    };

    let system = Arc::new(Mutex::new(system));
    *LIVE_SYSTEM_FS.lock() = Some(system.clone());
    #[allow(static_mut_refs)]
    unsafe {
        VFS.get_mut().mount("/", system);
        if let Some(iso) = install_media {
            VFS.get_mut()
                .mount("/media/install", Arc::new(Mutex::new(iso)));
        }
    }
    if show_log {
        println!(
            "\x1b[93m[{:.6}]\x1b[0m Live system mounted from {}",
            crate::driver::time::uptime_secs_f64(),
            source
        );
    }
    true
}

fn mount_pseudo_filesystems(live: bool) {
    #[allow(static_mut_refs)]
    unsafe {
        VFS.get_mut()
            .mount("/dev", Arc::new(Mutex::new(DevFs::new())));
        VFS.get_mut()
            .mount("/proc", Arc::new(Mutex::new(ProcFs::new())));
        if live {
            for path in ["/run", "/tmp", "/home", "/var/log"] {
                VFS.get_mut()
                    .mount(path, Arc::new(Mutex::new(InitramfsFs::empty())));
            }
        }
    }
}

fn try_mount_boot(bus: u8, dsk: u8, show_log: bool) -> bool {
    #[allow(static_mut_refs)]
    let already = unsafe {
        VFS.get_mut()
            .mount_points
            .iter()
            .any(|mount| mount.prefix == "/boot")
    };
    if already {
        return true;
    }

    if let Some(entry) = detect_fat32_partition(bus, dsk) {
        let entry_lba = entry.lba_start;
        if let Ok(fs) = Fat32Fs::from_partition(bus, dsk, entry) {
            #[allow(static_mut_refs)]
            unsafe {
                VFS.get_mut().mount("/boot", Arc::new(Mutex::new(fs)));
            }
            if show_log {
                println!(
                    "\x1b[93m[{:.6}]\x1b[0m FAT32 partition mounted at /boot from ATA {}:{} (LBA {})",
                    crate::driver::time::uptime_secs_f64(),
                    bus,
                    dsk,
                    entry_lba
                );
            }
            return true;
        }
    }

    if let Some(entry) = detect_fat16_partition(bus, dsk) {
        let entry_lba = entry.lba_start;
        if let Ok(fs) = Fat16Fs::from_partition(bus, dsk, entry) {
            #[allow(static_mut_refs)]
            unsafe {
                VFS.get_mut().mount("/boot", Arc::new(Mutex::new(fs)));
            }
            if show_log {
                println!(
                    "\x1b[93m[{:.6}]\x1b[0m FAT16 partition mounted at /boot from ATA {}:{} (LBA {})",
                    crate::driver::time::uptime_secs_f64(),
                    bus,
                    dsk,
                    entry_lba
                );
            }
            return true;
        }
    }

    false
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
