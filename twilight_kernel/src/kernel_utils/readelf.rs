use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs;

pub fn main(args: &[&str]) {
    if args.len() < 2 {
        println!("Usage: readelf <file>");
        return;
    }

    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    let inode = if pwd == "/" {
        1
    } else {
        fs.resolve_path(pwd).unwrap()
    };

    if let Some(inode) = fs.find_dir_entry(inode, args[1]).unwrap() {
        let content_buf = fs.read_file(inode + 1).unwrap();

        if let Ok(elf) = object::File::parse(content_buf.as_slice()) {
            println!("{:?}", elf);
        } else {
            println!("readelf: filed to parse file {}", args[1]);
        }
    } else {
        println!("readelf: {}: No such file or directory", args[1]);
    }
}
