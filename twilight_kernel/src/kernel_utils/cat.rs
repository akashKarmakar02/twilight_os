use crate::{fs, print, println};

pub fn main(args: &[&str]) {
    if args.len() == 0 {
        return;
    }

    if args[0] == ">" && args.len() == 2 {
        if let Some(mut file) = fs::File::create(args[1]) {
            file.write("Hello World");
            println!("Created file {}", args[1]);
        } else {
            println!("Failed to create file {}", args[1]);
        }
        return;
    }

    if let Some(mut file) = fs::File::open(args[0]) {
        println!("{}", file.read());
    } else {
        println!("failed to read file {}", args[0]);
    }
}
