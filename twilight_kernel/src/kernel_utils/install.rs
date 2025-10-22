use crate::driver::timer::cmos::CMOS;
use crate::sys::fs::init;
use crate::sys::fs::ram_fs::initramfs::CpioIterator;
use crate::println;
use alloc::format;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::mutex::Mutex;
use crate::sys::fs::twilight_fs::inode::Inode;

lazy_static! {
    pub static ref INITRAMFS: Mutex<CpioIterator> = Mutex::new(CpioIterator::default());
}

pub fn main() {
    let mut need_copy = false;

    #[allow(static_mut_refs)]
    {
        let device = unsafe { crate::driver::disk::BLOCK_DEVICE.as_mut() };
        if let Some(disk) = device {
            if let Ok(mut fs) = crate::fs::twilight_fs::format_superblock(
                &mut **disk,
            ) {
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
                need_copy = true;
            }
        } else {
            println!("disk not found");
            return;
        }
    }

    let mut initramfs = { INITRAMFS.lock() };
    if need_copy {
        while let Some(cpio_res) = initramfs.next() {
            match cpio_res {
                Ok(entry) => {
                    if entry.header.is_regular_file() {
                        copy_file(format!("/{}", entry.filename().unwrap()).as_str(), entry.data, true);
                    }
                }
                Err(_e) => {}
            }
        }
    }
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
