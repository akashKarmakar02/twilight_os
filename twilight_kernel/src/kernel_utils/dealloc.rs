use crate::driver::disk::DISK_FS;
use crate::println;

pub fn main(args: &[&str]) {
    if args.is_empty() {
        println!("Usage: dealloc <zone>");
        return;
    }
    
    let mut zone_idx: u32 = 0;
    
    if let Ok(zone) = args[0].parse::<u32>() {
        zone_idx = zone;
        println!("Deallocating zone {}", zone);
    } else {
        println!("Invalid zone");
    }
    
    
    #[allow(static_mut_refs)]
    if let Some(disk) = unsafe { DISK_FS.get_mut() } {
        match disk.free_zone(zone_idx) {
            Ok(_) => println!("Zone {} deallocated", zone_idx),
            Err(e) => println!("Error: {}", e),
        }
    } else { 
        println!("filesystem not mounted");
    }
}