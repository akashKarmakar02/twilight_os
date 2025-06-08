use crate::driver::disk::DISK_FS;
use crate::println;

pub fn main() {
    #[allow(static_mut_refs)]
    let block = unsafe { DISK_FS.get_mut() };
    if let Some(block) = block {
        let alloc = block.allocate_zone();
        match alloc {
            Ok(alloc) => println!("Allocated zone: {:?}", alloc),
            Err(err) => println!("Error: {:?}", err),
        }
    } else {
        println!("No filesystem found.")
    }
}