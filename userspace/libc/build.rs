use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=c/printf.c");
    println!("cargo:rerun-if-changed=c/scanf.c");
    println!("cargo:rerun-if-changed=include/unistd.h");

    cc::Build::new()
        .file("c/printf.c")
        .file("c/scanf.c")
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .compile("mini_printf");

    let profile = env::var("PROFILE").unwrap();
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("target").join(&profile);
    println!("cargo:STATICLIB_DIR={}", dir.display());
    println!("cargo:STATICLIB_NAME=twilight_c");
}
