use crate::println;

use crate::sys::memory::heap::{heap_size, heap_used};

pub fn main() {
    let total_mem = heap_size();
    let used_mem = heap_used();
    let free_mem = total_mem - used_mem;

    println!("Memory Information:");
    println!("-------------------");
    println!("Total: {} KB", total_mem / 1024);
    println!("Used:  {} KB", used_mem / 1024);
    println!("Free:  {} KB", free_mem / 1024);
}
