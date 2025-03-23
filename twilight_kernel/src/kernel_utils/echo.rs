use crate::{print, println};

pub fn main(args: &[&str]) {
    if args.iter().count() == 0 {
        println!("Echo: no arguments supplied");
        return;
    }

    println!("{}", args.join(" "));
}