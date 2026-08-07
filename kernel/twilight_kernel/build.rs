use std::env;
use std::path::PathBuf;

fn main() {
    // Only link the kernel executable with the bare-metal linker script. When
    // `cargo test` builds a host test binary (target_os = "linux"), the kernel
    // linker script must NOT be applied: it lacks a PT_TLS program header, so
    // linking libstd-based test binaries fails with "STT_TLS symbol but doesn't
    // have a PT_TLS segment". Gating on target_os keeps host unit tests runnable.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "none" {
        return;
    }

    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();

    // Get the absolute path to the linker script
    let linker_script = PathBuf::from(format!("twilight_kernel/linker-{arch}.ld"));

    println!("cargo:rustc-link-arg=-T{}", linker_script.display());
    println!("cargo:rerun-if-changed={}", linker_script.display());
}
