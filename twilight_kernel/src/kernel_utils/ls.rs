use crate::fs::Vfs;
use crate::{println, print};

pub fn main(_args: &[&str], ctx: &str) {
    let fs = crate::fs::FS.get().unwrap().lock();

    if let Ok(id) = fs.open(ctx) {
        let res = fs.readdir(id).ok().unwrap();

        for file in res {
            println!("{}", file);
        }
    }

}