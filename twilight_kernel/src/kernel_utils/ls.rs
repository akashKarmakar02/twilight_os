use alloc::string::String;
use alloc::vec::Vec;

use crate::{fs, println};

pub fn main(_args: &[&str], _ctx: &str) {
    let files = get_files("/");

    for file in files {
        println!("{}", file);
    }
}

fn get_files(ctx: &str) -> Vec<String> {
    if let Some(files) = fs::readdir(ctx) {
        files
    } else {
        Vec::new()
    }

}
