use crate::{println, print};

pub fn main() {
    use crate::memory::allocator::ALLOCATOR;

    let heap = ALLOCATOR.lock();

    let total_mem = heap.size();
    let used_mem = heap.used();
    let free_mem = total_mem - used_mem;

    println!("Memory Information:");
    println!("-------------------");
    println!("Total: {} KB", total_mem / 1024);
    println!("Used:  {} KB", used_mem / 1024);
    println!("Free:  {} KB", free_mem / 1024);
}