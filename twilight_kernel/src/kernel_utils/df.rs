use crate::println;

pub fn main(_args: &[&str]) {
    #[allow(static_mut_refs)]
    let disk = unsafe { crate::driver::disk::BLOCK_DEVICE.as_mut() };

    if let Some(disk_ref) = disk {
        if crate::fs::twilight_fs::read_superblock(&mut **disk_ref).is_ok() {
            println!("{:<12}{}", "Filesystem", "Mounted on");
            println!("{:<12}{}", "twilightfs", "/dev/ata0");
        } else {
            println!("{:<12}{}", "Filesystem", "Mounted on");
            println!("{:<12}{}", "unknown", "/dev/ata0");
        }
    }
}