use alloc::vec::Vec;
use crate::driver::disk::BlockDeviceIO;
use crate::println;
use crate::sys::fs::init;
use crate::sys::fs::minixfs::{Inode};

macro_rules! copy_file {
    ($path:expr, $verbose:expr) => {{
        copy_file($path, include_bytes!(concat!("../../../rootfs", $path)), $verbose);
    }};
}

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
    let mut need_copy = false;

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
                    indirect_zones: 0,
                };
                root_inode.zones[0] = root_zone as u16;
                fs.write_inode(root_inode_num + 1, &root_inode).expect("TODO: panic message");

                // Add '.' and '..'
                fs.create_dir_entry(root_inode_num + 1, ".", root_inode_num + 1).expect("TODO: panic message");
                fs.create_dir_entry(root_inode_num + 1, "..", root_inode_num + 1).expect("TODO: panic message");
                
                init(false);
                
                fs.create_dir(root_inode_num + 1, "bin").unwrap();
                fs.create_dir(root_inode_num + 1, "dev").unwrap();
                fs.create_dir(root_inode_num + 1, "init").unwrap();
                fs.create_dir(root_inode_num + 1, "home").unwrap();
                need_copy = true;
            }
        } else {
            println!("disk not found");
            return;
        }
    }
    
    if need_copy {
        copy_file!("/init/logo", true);
        copy_file!("/bin/exit42", true);
    }
}

fn copy_file(path: &str, data: &[u8], verbose: bool) {
    use crate::sys::fs::minixfs::FsError;
    
    let mut fs = unsafe {
        crate::fs::MFS.get_unchecked().lock()
    };

    let components: Vec<&str> = path
        .split('/')
        .filter(|c| !c.is_empty())
        .collect();

    if components.is_empty() {
        println!("Invalid file path: {}", path);
        return;
    }

    // Traverse or create parent directories
    let mut cur_inode = 1; // root
    for &part in &components[..components.len() - 1] {
        match fs.find_dir_entry(cur_inode, part) {
            Ok(Some(inode)) => cur_inode = inode,
            Ok(None) => {
                match fs.create_dir(cur_inode, part) {
                    Ok(new_inode) => cur_inode = new_inode,
                    Err(e) => {
                        println!("Failed to create dir '{}': {:?}", part, e);
                        return;
                    }
                }
            }
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
            if let Err(e) = fs.write_file(file_inode + 1, data) {
                println!("Failed to write to '{}': {:?}", path, e);
            } else if verbose {
                println!("\x1b[93m[DEBUG] \x1b[0mcopied: {}", path);
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
