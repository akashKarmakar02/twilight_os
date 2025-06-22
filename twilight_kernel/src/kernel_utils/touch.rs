use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs::minixfs::FsError::FileAlreadyExists;

pub fn main(args: &[&str]) {
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    
    let mut inode = 1;

    let mut fs = unsafe { crate::sys::fs::MFS.get_unchecked().lock() };

    if pwd != "/" {
        inode = fs.find_dir_entry(inode, pwd).unwrap().unwrap();
    }
    if let Err(e) = fs.create_file(inode, args[0]) {
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
