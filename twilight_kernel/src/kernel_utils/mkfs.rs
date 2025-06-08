use crate::println;

pub fn main(args: &[&str]) {
    if args.len() < 2 {
        println!("Usage: mkfs -t <fs> <disk>");
        return;
    }

    if args[1] != "minixfs" {
        println!("Unsupported filesystem type {}", args[2]);
        return;
    }

    if args[2] != "/dev/ata0" {
        println!("disk not found");
        return;
    }

    #[allow(static_mut_refs)]
    let disk = unsafe { crate::driver::disk::DISK.get_mut() };

    if let Some(disk) = disk {
        let disk_size = disk.sector_size() * disk.sector_count();
        let inode = disk_size / (disk.sector_size() * 4);
        let block_size = disk.sector_size();

        crate::fs::minixfs::format_superblock(*disk, disk_size as usize, inode as u16, block_size as u16).expect("Unable to initialize minixfs");
    } else {
        println!("disk not found");
    }
}