use crate::println;
use crate::sys::fs::minixfs::FsError::FileAlreadyExists;

pub fn main(args: &[&str]) {
    let root_inode = 1;

    let mut fs = unsafe { crate::sys::fs::MFS.get_unchecked().lock() };
    if let Err(e) = fs.create_file(root_inode, args[0]) {
        match e {
            FileAlreadyExists => {
                println!("{}: already exists", args[0]);
                return;
            }
            _ => {
                println!("Failed to create file");
            }
        }
    }
}
