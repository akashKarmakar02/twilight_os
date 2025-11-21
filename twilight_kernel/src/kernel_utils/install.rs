#![allow(unused_assignments)]
use crate::driver::disk::BlockDeviceIO;
use crate::driver::timer::cmos::CMOS;
use crate::println;
use crate::sys::fs::init;
use crate::sys::fs::partition::{self, PartitionEntry};
use crate::sys::fs::ram_fs::initramfs::CpioIterator;
use crate::sys::fs::twilight_fs::inode::Inode;
use alloc::format;
use alloc::vec::Vec;
use core::cmp;
use lazy_static::lazy_static;
use spin::mutex::Mutex;

lazy_static! {
    pub static ref INITRAMFS: Mutex<CpioIterator> = Mutex::new(CpioIterator::default());
}

const PARTITION_ALIGNMENT_SECTORS: u32 = 2048; // 1 MiB
const RESERVED_BOOT_MB: u32 = 64;
const MIN_TWILIGHT_SECTORS: u32 = PARTITION_ALIGNMENT_SECTORS * 8;

struct TwilightPartitionLayout {
    twilight_start_lba: u32,
    twilight_sectors: u32,
    boot_partition: Option<PartitionEntry>,
}

pub fn main() {
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
                &mut **disk,
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

            let mut root_inode = Inode {
                mode: 0o040755, // directory
                nlinks: 2,
                uid: 0,
                gid: 0,
                size: 0,
                access_time: time as u32,
                created_time: time as u32,
                modified_time: time as u32,
                zones: [0; 7],
                indirect_zones: 0,
                double_indirect_zones: 0,
                triple_indirect_zones: 0,
            };
            root_inode.zones[0] = root_zone;

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

    let mut initramfs = { INITRAMFS.lock() };
    if need_copy {
        while let Some(cpio_res) = initramfs.next() {
            match cpio_res {
                Ok(entry) => {
                    if entry.header.is_regular_file() {
                        copy_file(
                            format!("/{}", entry.filename().unwrap()).as_str(),
                            entry.data,
                            true,
                        );
                    }
                }
                Err(_e) => {}
            }
        }
    }
}

fn ensure_partition_table(
    device: &mut dyn BlockDeviceIO,
) -> Result<TwilightPartitionLayout, &'static str> {
    let total_sectors = cmp::min(device.block_count() as u64, u32::MAX as u64);
    if total_sectors <= (PARTITION_ALIGNMENT_SECTORS as u64) * 2 {
        return Err("disk is too small to partition");
    }

    let mut mbr = [0u8; 512];
    let mut entries = [PartitionEntry::empty(); 4];
    let mut boot_slot = None;
    let mut twilight_slot = None;

    if device.read(0, &mut mbr).is_ok() && partition::has_signature(&mbr) {
        entries = partition::decode_entries(&mbr);
        boot_slot = entries.iter().position(is_boot_partition);
        twilight_slot = entries.iter().position(|entry| {
            entry.partition_type == partition::TWILIGHT_PARTITION_TYPE && entry.is_present()
        });
    } else {
        mbr.fill(0);
    }

    let mut boot_entry = boot_slot.map(|idx| entries[idx]);
    let mut twilight_entry = twilight_slot.map(|idx| entries[idx]);

    let min_twilight = MIN_TWILIGHT_SECTORS as u64;

    let mut boot_start = if let Some(entry) = boot_entry {
        entry.lba_start as u64
    } else {
        PARTITION_ALIGNMENT_SECTORS as u64
    };

    let mut boot_sectors = if let Some(entry) = boot_entry {
        entry.sectors as u64
    } else {
        align_up_u64(
            (RESERVED_BOOT_MB as u64 * 1024 * 1024) / partition::SECTOR_SIZE as u64,
            PARTITION_ALIGNMENT_SECTORS as u64,
        )
    };

    if let Some(entry) = twilight_entry {
        if (entry.sectors as u64) < min_twilight {
            return Err("existing Twilight partition is too small");
        }
    } else {
        let mut start = align_up_u64(
            boot_start + boot_sectors,
            PARTITION_ALIGNMENT_SECTORS as u64,
        );
        if boot_entry.is_none() && total_sectors <= start + min_twilight {
            boot_sectors = 0;
            start = boot_start;
        }
        if total_sectors <= start + min_twilight {
            return Err("disk is too small to host TwilightFS");
        }
        let sectors = total_sectors - start;
        if sectors < min_twilight {
            return Err("disk is too small to host TwilightFS");
        }
        let entry = PartitionEntry::new(
            0x00,
            partition::TWILIGHT_PARTITION_TYPE,
            start as u32,
            sectors as u32,
        );
        let slot = insert_partition_entry(&mut entries, twilight_slot, entry)
            .ok_or("no free partition table entry for TwilightFS")?;
        twilight_slot = Some(slot);
        twilight_entry = Some(entry);
    }

    if boot_entry.is_none() && boot_sectors > 0 {
        let entry = PartitionEntry::new(
            0x00,
            partition::FAT32_LBA_PARTITION_TYPE,
            boot_start as u32,
            boot_sectors as u32,
        );
        let slot = insert_partition_entry(&mut entries, boot_slot, entry)
            .ok_or("no free partition table entry for boot partition")?;
        boot_slot = Some(slot);
        boot_entry = Some(entry);
    } else if let Some(entry) = boot_entry {
        boot_start = entry.lba_start as u64;
        boot_sectors = entry.sectors as u64;
    }

    partition::encode_entries(&mut mbr, &entries);
    partition::write_signature(&mut mbr);
    device
        .write(0, &mbr)
        .map_err(|_| "failed to write partition table")?;

    Ok(TwilightPartitionLayout {
        twilight_start_lba: twilight_entry.unwrap().lba_start,
        twilight_sectors: twilight_entry.unwrap().sectors,
        boot_partition: boot_entry,
    })
}

fn insert_partition_entry(
    entries: &mut [PartitionEntry; 4],
    slot: Option<usize>,
    entry: PartitionEntry,
) -> Option<usize> {
    if let Some(idx) = slot {
        entries[idx] = entry;
        return Some(idx);
    }
    for (idx, existing) in entries.iter_mut().enumerate() {
        if !existing.is_present() {
            *existing = entry;
            return Some(idx);
        }
    }
    None
}

fn is_boot_partition(entry: &PartitionEntry) -> bool {
    entry.is_present()
        && matches!(
            entry.partition_type,
            partition::FAT32_LBA_PARTITION_TYPE
                | partition::FAT16_CHS_PARTITION_TYPE
                | partition::FAT16_LBA_PARTITION_TYPE
                | 0x0B
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

fn copy_file(path: &str, data: &[u8], verbose: bool) {
    use crate::sys::fs::twilight_fs::FsError;

    let mut fs = unsafe { crate::fs::MFS.get_unchecked().lock() };

    let components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();

    if components.is_empty() {
        println!("Invalid file path: {}", path);
        return;
    }

    let mut cur_inode = 1;
    for &part in &components[..components.len() - 1] {
        match fs.find_dir_entry(cur_inode, part) {
            Ok(Some(inode)) => cur_inode = inode,
            Ok(None) => match fs.create_dir(cur_inode, part) {
                Ok(new_inode) => cur_inode = new_inode,
                Err(e) => {
                    println!("Failed to create dir '{}': {:?}", part, e);
                    return;
                }
            },
            Err(e) => {
                println!("Failed to lookup '{}': {:?}", part, e);
                return;
            }
        }
    }

    let file_name = components.last().unwrap();

    // Create and write file
    match fs.create_file(cur_inode, file_name) {
        Ok(file_inode) => {
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
            if verbose {
                println!("Skipped (exists) {}", path);
            }
        }
        Err(e) => {
            println!("Failed to create file '{}': {:?}", path, e);
        }
    }
}
