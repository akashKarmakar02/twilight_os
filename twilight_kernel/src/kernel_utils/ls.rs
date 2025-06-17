use alloc::string::String;
use alloc::vec::Vec;

use crate::println;
use crate::sys::fs;

pub fn main(_args: &[&str], _ctx: &str) {
    let files = get_files("/");

    for file in files {
        println!("{}", file);
    }
}

fn get_files(ctx: &str) -> Vec<String> {
    fs::readdir(ctx).unwrap_or_default()

}
