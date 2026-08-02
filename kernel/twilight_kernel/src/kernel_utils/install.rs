#![allow(unused_assignments)]
use crate::driver::disk::BlockDeviceIO;
use crate::driver::timer::cmos::CMOS;
use crate::println;
use crate::sys::fs::init;
use crate::sys::fs::mbr::Mbr;
use crate::sys::fs::partition::{self};
use crate::sys::fs::ram_fs::initramfs::CpioIterator;
use crate::sys::fs::twilight_fs::inode::Inode;
use crate::sys::fs::vfs::{FileSystem, FileType, VFS};
use crate::utils::sync::Mutex;
use crate::{print, serial_println};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp;
use lazy_static::lazy_static;

lazy_static! {
    pub static ref INITRAMFS: Mutex<CpioIterator> = Mutex::new(CpioIterator::default());
}

const PARTITION_ALIGNMENT_SECTORS: u32 = 2048; // 1 MiB
const RESERVED_BOOT_MB: u32 = 50;
const MIN_TWILIGHT_SECTORS: u32 = PARTITION_ALIGNMENT_SECTORS * 8;

#[derive(Debug)]
struct TwilightPartitionLayout {
    twilight_start_lba: u32,
    twilight_sectors: u32,
    boot_partition: Option<crate::fs::mbr::PartitionEntry>,
}

#[derive(Clone)]
enum SourceEntryKind {
    File { mode: u16, size: usize },
    Symlink { target: String },
}

#[derive(Clone)]
struct SourceEntry {
    path: String,
    kind: SourceEntryKind,
}

impl SourceEntry {
    fn size(&self) -> usize {
        match &self.kind {
            SourceEntryKind::File { size, .. } => *size,
            SourceEntryKind::Symlink { target } => target.len(),
        }
    }
}

pub fn main() {
    let source = match crate::fs::LIVE_SYSTEM_FS.lock().clone() {
        Some(source) => source,
        None => {
            println!("install: no live system image is mounted");
            return;
        }
    };
    let mut source_entries = Vec::new();
    if collect_source_entries(&source, "/", &mut source_entries).is_err() {
        println!("install: failed to enumerate live system image");
        return;
    }
    let total_files = source_entries.len() as u64;
    let total_bytes = source_entries
        .iter()
        .map(|entry| entry.size() as u64)
        .sum::<u64>();
    if total_files == 0 {
        println!("install: live system image contains no files");
        return;
    }

    let need_copy = {
        #[allow(static_mut_refs)]
        {
            let device = unsafe { crate::driver::disk::BLOCK_DEVICE.as_mut() };
            let Some(disk) = device else {
                println!("disk not found");
                return;
            };

            let layout = match ensure_partition_table(&mut **disk) {
                Ok(layout) => layout,
                Err(err) => {
                    println!("install: {}", err);
                    return;
                }
            };

            if let Some(boot) = layout.boot_partition {
                let boot_lba = boot.lba_start;
                let boot_size_mib = sectors_to_mebibytes(boot.sectors);
                println!(
                    "Reserved boot partition at LBA {} ({} MiB)",
                    boot_lba, boot_size_mib
                );
            }

            println!(
                "TwilightFS partition at LBA {} ({} MiB)",
                layout.twilight_start_lba,
                sectors_to_mebibytes(layout.twilight_sectors)
            );

            let mut fs = match crate::fs::twilight_fs::format_superblock(
                alloc::boxed::Box::new(crate::driver::disk::GlobalBlkHandle),
                layout.twilight_start_lba,
                layout.twilight_sectors,
            ) {
                Ok(fs) => fs,
                Err(err) => {
                    println!("install: {}", err);
                    return;
                }
            };

            let root_inode_num = fs.allocate_inode().unwrap();
            let root_zone = fs.allocate_zone().unwrap();

            let time = CMOS::new().unix_time();

            let mut root_inode = Inode::new_dir(time, 0o755);
            root_inode.direct_slot_set(0, root_zone);

            fs.write_inode(root_inode_num + 1, &root_inode)
                .expect("TODO: panic message");

            // Add '.' and '..'
            fs.create_dir_entry(root_inode_num + 1, ".", root_inode_num + 1)
                .expect("TODO: panic message");
            fs.create_dir_entry(root_inode_num + 1, "..", root_inode_num + 1)
                .expect("TODO: panic message");

            init(false);

            fs.create_dir(root_inode_num + 1, "bin").unwrap();
            fs.create_dir(root_inode_num + 1, "dev").unwrap();
            fs.create_dir(root_inode_num + 1, "init").unwrap();
            fs.create_dir(root_inode_num + 1, "home").unwrap();
            fs.create_dir(root_inode_num + 1, "usr").unwrap();
            true
        }
    };

    if need_copy {
        print!("\x1b[?25l"); // hide cursor

        let mut done_files: u64 = 0;
        let mut done_bytes: u64 = 0;
        let mut dir_cache: Option<(String, u32)> = None;

        render_progress(done_files, total_files, done_bytes, total_bytes, None);

        for entry in source_entries.drain(..) {
            let entry_size = entry.size();
            match entry.kind {
                SourceEntryKind::File { mode, size } => {
                    let mut node = match source.lock().open(&entry.path) {
                        Ok(node) => node,
                        Err(_) => {
                            println!("install: failed to open {}", entry.path);
                            continue;
                        }
                    };
                    let mut data = alloc::vec![0u8; size];
                    if node.read_exact(0, &mut data).is_err() {
                        println!("install: failed to read {}", entry.path);
                        continue;
                    }
                    copy_file(&entry.path, &data, mode, false, &mut dir_cache);
                }
                SourceEntryKind::Symlink { target } => {
                    copy_symlink(&entry.path, &target, false, &mut dir_cache);
                }
            }

            done_files += 1;
            done_bytes += entry_size as u64;
            render_progress(
                done_files,
                total_files,
                done_bytes,
                total_bytes,
                Some(&entry.path),
            );
        }

        render_progress(
            total_files,
            total_files,
            done_bytes,
            total_bytes,
            Some("done"),
        );
        print!("\n\x1b[?25h"); // newline + show cursor

        #[allow(static_mut_refs)]
        unsafe {
            for mount in [
                "/run",
                "/tmp",
                "/home",
                "/var/log",
                "/media/install",
                "/dev",
                "/proc",
            ] {
                VFS.get_mut().unmount(mount);
            }
            VFS.get_mut().unmount("/");
        }
        *crate::fs::LIVE_SYSTEM_FS.lock() = None;
    }
}

fn collect_source_entries(
    source: &alloc::sync::Arc<Mutex<crate::fs::twilight_fs::TwilightFs>>,
    path: &str,
    files: &mut Vec<SourceEntry>,
) -> Result<(), ()> {
    let entries = source.lock().ls(path)?;
    for entry in entries {
        if entry.name == "." || entry.name == ".." {
            continue;
        }
        let child = if path == "/" {
            format!("/{}", entry.name)
        } else {
            format!("{}/{}", path.trim_end_matches('/'), entry.name)
        };
        match entry.file_type {
            FileType::Dir => collect_source_entries(source, &child, files)?,
            FileType::File => files.push(SourceEntry {
                path: child,
                kind: SourceEntryKind::File {
                    mode: entry.mode,
                    size: entry.size,
                },
            }),
            FileType::Symlink => {
                let target = source.lock().read_symlink(entry.ino).map_err(|_| ())?;
                files.push(SourceEntry {
                    path: child,
                    kind: SourceEntryKind::Symlink { target },
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn render_progress(
    done_files: u64,
    total_files: u64,
    done_bytes: u64,
    total_bytes: u64,
    current: Option<&str>,
) {
    let pct = if total_files == 0 {
        100
    } else {
        ((done_files * 100) / total_files).min(100)
    };

    let width: usize = 32;
    let filled = ((pct as usize) * width) / 100;
    let mut bar = String::with_capacity(width);
    for i in 0..width {
        bar.push(if i < filled { '#' } else { '.' });
    }

    let cur = current.unwrap_or("");
    let cur = tail_str(cur, 40);

    print!(
        "\r\x1b[2Kinstall: [{}] {:3}% {}/{} {} / {}  {}",
        bar,
        pct,
        done_files,
        total_files,
        fmt_bytes(done_bytes),
        fmt_bytes(total_bytes),
        cur
    );
}

fn tail_str(s: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }

    let mut count = 0usize;
    for _ in s.chars() {
        count += 1;
        if count > max_chars {
            break;
        }
    }
    if count <= max_chars {
        return s;
    }

    let mut seen = 0usize;
    let mut start = 0usize;
    for (idx, _) in s.char_indices().rev() {
        seen += 1;
        if seen == max_chars {
            start = idx;
            break;
        }
    }
    &s[start..]
}

fn fmt_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * 1024 * 1024;

    if n < KIB {
        format!("{} B", n)
    } else if n < MIB {
        format!("{} KiB", n / KIB)
    } else if n < GIB {
        format!("{} MiB", n / MIB)
    } else {
        format!("{} GiB", n / GIB)
    }
}

fn ensure_partition_table(
    device: &mut dyn BlockDeviceIO,
) -> Result<TwilightPartitionLayout, &'static str> {
    let total_sectors = cmp::min(device.block_count() as u64, u32::MAX as u64);
    if total_sectors <= (PARTITION_ALIGNMENT_SECTORS as u64) * 2 {
        serial_println!("{} {}", total_sectors, PARTITION_ALIGNMENT_SECTORS);
        return Err("disk is too small to partition");
    }

    let mut mbr = [0u8; 512];
    let mut boot_slot = None;
    let mut twilight_slot = None;

    if device.read(0, &mut mbr).is_err() {
        return Err("failed to read partition table");
    }

    let mut mbr_manager = match Mbr::new(mbr, device) {
        Some(m) => m,
        None => Mbr::create_new(mbr, device),
    };

    let mut entries = mbr_manager.get_entries();

    boot_slot = entries.iter().position(is_boot_partition);
    twilight_slot = entries.iter().position(|entry| {
        entry.partition_type == partition::TWILIGHT_PARTITION_TYPE && entry.is_present()
    });

    let mut boot_entry = boot_slot.map(|idx| entries[idx]);
    let mut twilight_entry = twilight_slot.map(|idx| entries[idx]);

    let min_twilight = MIN_TWILIGHT_SECTORS as u64;

    let boot_start = if let Some(entry) = boot_entry {
        entry.lba_start as u64
    } else {
        PARTITION_ALIGNMENT_SECTORS as u64
    };
    let mut boot_sectors = if let Some(entry) = boot_entry {
        entry.sectors as u64
    } else {
        0
    };

    // v0.1 boot sector needs to be atleast 50MB (14/02/26) so we will shrink this to 50mb so save space (TODO: we need to fix this in future)
    if boot_sectors > (50 * 1024 * 2) {
        entries[boot_slot.unwrap()].sectors = 50 * 1024 * 2;
        if mbr_manager.write_entries(&entries).is_err() {
            return Err("failed to write partition table");
        } else {
            boot_sectors = (RESERVED_BOOT_MB as u64) * 1024 * 2;
            let boot_entry_clone = entries[boot_slot.unwrap()];
            boot_entry = Some(crate::fs::mbr::PartitionEntry::new(
                boot_entry_clone.status,
                boot_entry_clone.partition_type,
                boot_entry_clone.lba_start,
                boot_sectors as u32,
            ));
            println!("MBR: resized boot partition to 50mb");
        }
    }

    if let Some(_entry) = twilight_entry {
    } else {
        let mut start = align_up_u64(
            boot_start + boot_sectors,
            PARTITION_ALIGNMENT_SECTORS as u64,
        );

        if boot_entry.is_some() && total_sectors <= start + min_twilight {
            boot_sectors = 0;
            start = boot_start;
        }

        if total_sectors <= start + min_twilight {
            serial_println!("LOG: (1) {} {}", total_sectors, start + min_twilight);
            return Err("disk is too small to host TwilightFS");
        }
        let sectors = total_sectors - start;
        if sectors < min_twilight {
            serial_println!("LOG: (2) {} {}", sectors, min_twilight);
            return Err("disk is too small to host TwilightFS");
        }

        twilight_entry = Some(crate::fs::mbr::PartitionEntry::new(
            0x00,
            partition::TWILIGHT_PARTITION_TYPE,
            start as u32,
            sectors as u32,
        ));

        let twilight_entry_val = crate::fs::mbr::PartitionEntry::new(
            0x00,
            partition::TWILIGHT_PARTITION_TYPE,
            start as u32,
            sectors as u32,
        );

        for (idx, entry) in entries.iter_mut().enumerate() {
            if entry.partition_type == 0x00 {
                twilight_slot = Some(idx);
                entry.sectors = twilight_entry_val.sectors;
                entry.partition_type = partition::TWILIGHT_PARTITION_TYPE;
                entry.lba_start = twilight_entry_val.lba_start;
                entry.status = twilight_entry_val.status;
                break;
            }
        }

        if mbr_manager.write_entries(&entries).is_err() {
            return Err("failed to write partition table");
        }
    }

    Ok(TwilightPartitionLayout {
        twilight_start_lba: twilight_entry.unwrap().lba_start,
        twilight_sectors: twilight_entry.unwrap().sectors,
        boot_partition: boot_entry,
    })
}

fn is_boot_partition(entry: &crate::fs::mbr::PartitionEntry) -> bool {
    entry.is_present()
        && matches!(
            entry.partition_type,
            partition::FAT32_CHS_PARTITION_TYPE
                | partition::FAT32_LBA_PARTITION_TYPE
                | partition::EFI_SYSTEM_PARTITION_TYPE
                | partition::FAT16_CHS_PARTITION_TYPE
                | partition::FAT16_LBA_PARTITION_TYPE
        )
}

fn sectors_to_mebibytes(sectors: u32) -> u64 {
    (sectors as u64 * partition::SECTOR_SIZE as u64) / (1024 * 1024)
}

fn align_up_u64(value: u64, align: u64) -> u64 {
    if align == 0 {
        return value;
    }
    ((value + align - 1) / align) * align
}

fn destination_parent(
    fs: &mut crate::fs::twilight_fs::TwilightFs,
    path: &str,
    cache: &mut Option<(String, u32)>,
) -> Result<(u32, String), crate::sys::fs::twilight_fs::FsError> {
    use crate::sys::fs::twilight_fs::FsError;

    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    if components.is_empty() {
        return Err(FsError::InvalidPath);
    }

    let mut parent_inode = 1;
    let mut start_idx = 0;
    if components.len() > 1 {
        let parent_path = components[..components.len() - 1].join("/");
        if let Some((cached_path, cached_inode)) = cache {
            if *cached_path == parent_path {
                parent_inode = *cached_inode;
                start_idx = components.len() - 1;
            }
        }
    }

    for (index, part) in components[..components.len() - 1].iter().enumerate() {
        if index < start_idx {
            continue;
        }
        parent_inode = match fs.find_dir_entry(parent_inode, part) {
            Ok(Some(inode)) => inode,
            Ok(None) => fs.create_dir(parent_inode, part)?,
            Err(_) => return Err(FsError::InvalidPath),
        };
    }

    if components.len() > 1 {
        let parent_path = components[..components.len() - 1].join("/");
        *cache = Some((parent_path, parent_inode));
    }

    Ok((parent_inode, String::from(*components.last().unwrap())))
}

fn copy_file(path: &str, data: &[u8], mode: u16, verbose: bool, cache: &mut Option<(String, u32)>) {
    use crate::sys::fs::twilight_fs::FsError;

    let mut fs = unsafe { crate::fs::MFS.get_unchecked().lock() };
    let (parent_inode, file_name) = match destination_parent(&mut fs, path, cache) {
        Ok(destination) => destination,
        Err(error) => {
            println!("Failed to prepare '{}': {:?}", path, error);
            return;
        }
    };

    // Create and write file
    match fs.create_file(parent_inode, &file_name) {
        Ok(file_inode) => {
            set_file_mode(&mut fs, file_inode, mode);
            if let Err(e) = fs.write_file(file_inode, data) {
                println!("Failed to write to '{}': {:?}", path, e);
            } else if verbose {
                println!(
                    "\x1b[93m[DEBUG] \x1b[0mcopied: {} inode: {}",
                    path, file_inode
                );
            }
        }
        Err(FsError::FileAlreadyExists) => {
            if let Ok(Some(file_inode)) = fs.find_dir_entry(parent_inode, &file_name) {
                set_file_mode(&mut fs, file_inode, mode);
            }
            if verbose {
                println!("Skipped (exists) {}", path);
            }
        }
        Err(e) => {
            println!("Failed to create file '{}': {:?}", path, e);
        }
    }
}

fn copy_symlink(path: &str, target: &str, verbose: bool, cache: &mut Option<(String, u32)>) {
    use crate::sys::fs::twilight_fs::FsError;

    let mut fs = unsafe { crate::fs::MFS.get_unchecked().lock() };
    let (parent_inode, file_name) = match destination_parent(&mut fs, path, cache) {
        Ok(destination) => destination,
        Err(error) => {
            println!("Failed to prepare '{}': {:?}", path, error);
            return;
        }
    };

    match fs.create_symlink(parent_inode, &file_name, target) {
        Ok(_) => {
            if verbose {
                println!("copied symlink: {} -> {}", path, target);
            }
        }
        Err(FsError::FileAlreadyExists) => {
            if verbose {
                println!("Skipped (exists) {}", path);
            }
        }
        Err(error) => {
            println!("Failed to create symlink '{}': {:?}", path, error);
        }
    }
}

fn set_file_mode(fs: &mut crate::fs::twilight_fs::TwilightFs, inode_num: u32, mode: u16) {
    let Ok(mut inode) = fs.read_inode(inode_num) else {
        return;
    };
    let new_mode = (inode.mode & crate::fs::twilight_fs::inode::MODE_TYPE_MASK) | (mode & 0o7777);
    if new_mode != inode.mode {
        inode.mode = new_mode;
        let _ = fs.write_inode(inode_num, &inode);
    }
}
