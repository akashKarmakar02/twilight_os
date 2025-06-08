use crate::println;

pub fn main(_args: &[&str]) {
    #[allow(static_mut_refs)]
    let disk = unsafe { crate::driver::disk::DISK.get_mut() };
    
    if let Some(disk) = disk {
        if crate::fs::minixfs::read_superblock(*disk).is_ok() {
            println!("{:<12}{}", "Filesystem", "Mounted on");
            println!("{:<12}{}", "minixfs", "/dev/ata0");
        } else {
            println!("{:<12}{}", "Filesystem", "Mounted on");
            println!("{:<12}{}", "unknown", "/dev/ata0");
        }
    }
}