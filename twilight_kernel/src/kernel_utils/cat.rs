use crate::println;
use crate::sys::console::DIR;
use crate::sys::fs;
use alloc::string::String;

pub fn main(args: &[&str]) {
    if args.is_empty() {
        return;
    }

    let mut fs = unsafe { fs::MFS.get_unchecked().lock() };
    #[allow(static_mut_refs)]
    let pwd = unsafe { DIR.as_str() };
    let inode = if pwd == "/" { 1 } else { fs.find_dir_entry(1, &pwd[1..]).unwrap().unwrap() };


    if args[0] == ">" {
        let inode = match fs.find_dir_entry(inode, args[1]).unwrap() {
            Some(inode) => inode,
            None => fs.create_file(1, args[1]).unwrap(),
        };

        if args.len() > 2 {
            fs.write_file(inode + 1, args[2..].join(" ").as_bytes()).unwrap();
        }

        return;
    }

    if let Some(inode) = fs.find_dir_entry(inode, args[0]).unwrap() {
        let content_buf = fs.read_file(inode + 1);

        println!(
            "{}",
            String::from_utf8_lossy(content_buf.unwrap().as_slice())
        );
    } else {
        println!("cat: {}: No such file or directory", args[0]);
    }
}
