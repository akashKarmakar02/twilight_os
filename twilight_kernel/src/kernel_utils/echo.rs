use crate::println;

pub fn main(args: &[&str]) {
    if args.len() == 1 {
        println!("Echo: no arguments supplied");
        return;
    }

    println!("{}", args[1..].join(" "));
}