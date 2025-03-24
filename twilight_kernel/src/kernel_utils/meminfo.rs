use crate::{println, print};

pub fn main() {
    let total_mem = crate::memory::allocator::get_total_heap_size();
    let used_mem = crate::memory::allocator::get_used_heap_size();
    let free_mem = total_mem - used_mem;

    println!("Memory Information:");
    println!("-------------------");
    println!("Total: {} KB", total_mem / 1024);
    println!("Used:  {} KB", used_mem / 1024);
    println!("Free:  {} KB", free_mem / 1024);
}