use crate::driver::disk::BlockDeviceIO;
use crate::println;
use crate::sys::fs::minixfs::{Inode};

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

    let disk_size;
    let inode;
    let block_size;

    #[allow(static_mut_refs)]
    {
        let device = unsafe { crate::driver::disk::BLOCK_DEVICE.as_mut() };
        if let Some(disk) = device {
            disk_size = disk.block_size() * disk.block_count();
            inode = disk_size / (disk.block_size() * 16);
            block_size = disk.block_size();
            
            if let Ok(_sb) = crate::fs::minixfs::read_superblock(disk) {
                println!("disk already formatted");
                return;
            }

            if let Ok(mut fs) = crate::fs::minixfs::format_superblock(disk, disk_size, inode as u16, block_size as u16) {
                let root_inode_num = fs.allocate_inode().unwrap();
                let root_zone = fs.allocate_zone().unwrap();

                let now = 0; // set timestamp if available

                let mut root_inode = Inode {
                    mode: 0o040755, // directory
                    nlinks: 2,
                    uid: 0,
                    gid: 0,
                    size: 0,
                    time: now,
                    zones: [0; 9],
                };
                root_inode.zones[0] = root_zone as u16;
                fs.write_inode(root_inode_num + 1, &root_inode).expect("TODO: panic message");
                
                // Add '.' and '..'
                fs.create_dir_entry(root_inode_num + 1, ".", root_inode_num).expect("TODO: panic message");
                fs.create_dir_entry(root_inode_num + 1, "..", root_inode_num).expect("TODO: panic message");
            }
        } else {
            println!("disk not found");
            return;
        }
    }
}
