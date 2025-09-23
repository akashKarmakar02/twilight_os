use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=.");
    let nasm_dir = Path::new("nasm");
    let obj_out_dir = Path::new("target/nasm");
    let bin_out_dir = Path::new("../rootfs/bin");

    fs::create_dir_all(&obj_out_dir).expect("Failed to create target/nasm/");
    fs::create_dir_all(&bin_out_dir).expect("Failed to create rootfs/bin/");

    for entry in fs::read_dir(nasm_dir).expect("Failed to read nasm folder") {
        let entry = entry.expect("Invalid dir entry");
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("asm") {
            let filename = path.file_stem().unwrap().to_str().unwrap();

            let obj_path = obj_out_dir.join(format!("{filename}.o"));
            let bin_path = bin_out_dir.join(filename);

            println!("cargo:rerun-if-changed={}", path.display());

            // Assemble with NASM
            let nasm_status = Command::new("nasm")
                .args([
                    "-f", "elf64",
                    path.to_str().unwrap(),
                    "-o", obj_path.to_str().unwrap()
                ])
                .status()
                .expect("Failed to run nasm");
            if !nasm_status.success() {
                panic!("NASM failed for file: {}", path.display());
            }

            // Link with ld
            let ld_status = Command::new("ld")
                .args([
                    "-nostdlib",
                    "-static",
                    "-o", bin_path.to_str().unwrap(),
                    obj_path.to_str().unwrap(),
                ])
                .status()
                .expect("Failed to run ld");
            if !ld_status.success() {
                panic!("ld failed to link object: {}", obj_path.display());
            }
        }
    }

    let c_dir = Path::new("apps");
    let obj_out_dir = Path::new("target/apps");

    fs::create_dir_all(&obj_out_dir).expect("Failed to create target/apps/");

    for entry in fs::read_dir(c_dir).expect("Failed to read nasm folder") {
        let entry = entry.expect("Invalid dir entry");
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("c") {
            let filename = path.file_stem().unwrap().to_str().unwrap();

            let bin_path = bin_out_dir.join(filename);
            println!("cargo:rerun-if-changed={}", path.display());

            let c_status = Command::new("musl-gcc")
                .args([
                    "-static",
                    path.to_str().unwrap(),
                    "-o",
                    bin_path.to_str().unwrap(),
                ])
                .status()
                .unwrap();

            if !c_status.success() {
                panic!("GCC failed for file: {}", path.display());
            }
        }
    }
}
