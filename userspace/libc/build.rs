use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=c/printf.c");
    println!("cargo:rerun-if-changed=c/scanf.c");
    println!("cargo:rerun-if-changed=c/stdio.c");
    println!("cargo:rerun-if-changed=c/string.c");
    println!("cargo:rerun-if-changed=c/stdlib.c");
    println!("cargo:rerun-if-changed=c/ctype.c");
    println!("cargo:rerun-if-changed=include/unistd.h");

    cc::Build::new()
        .files([
            "c/printf.c",
            "c/scanf.c",
            "c/stdio.c",
            "c/string.c",
            "c/ctype.c",
            "c/stdlib.c",
        ])
        .flag("-ffreestanding")
        .flag("-fno-stack-protector")
        .compile("mini_c");

    let profile = env::var("PROFILE").unwrap();
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("target").join(&profile);
    println!("cargo:STATICLIB_DIR={}", dir.display());
    println!("cargo:STATICLIB_NAME=twilight_c");
}
