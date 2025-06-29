use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs;

pub fn main(args: &[&str]) {
    if args.len() < 1 {
        println!("Usage: readelf <file>");
        return;
    }

    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    let inode = if pwd == "/" { 1 } else { fs.resolve_path(pwd).unwrap() };

    if let Some(inode) = fs.find_dir_entry(inode, args[0]).unwrap() {
        let content_buf = fs.read_file(inode + 1).unwrap();
        
        let elf = goblin::elf::Elf::parse(content_buf.as_slice()).unwrap();
        println!("{:#?}", elf.header);
    } else {
        println!("readelf: {}: No such file or directory", args[0]);
    }
}
