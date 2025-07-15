use crate::serial_prtinln;
use limine::memory_map::EntryType;
use linked_list_allocator::LockedHeap;
use x86_64::VirtAddr;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB};
use crate::sys::memory::{mapper, MEMORY_MAP};

pub const HEAP_START: usize = 0x_4444_4444_0000;

#[global_allocator]
pub static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init_heap(
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let heap_size = get_total_usable_memory();
    
    let mapper = mapper();

    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let head_end = heap_start + heap_size / 2 - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(head_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };
    serial_prtinln!("Total usable memory: {} mb", heap_size / (1024 * 1024));

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe { mapper.map_to(page, frame, flags, frame_allocator)?.flush() };
    }

    unsafe {
        ALLOCATOR.lock().init(HEAP_START, heap_size as usize);
    };

    Ok(())
}

pub fn get_total_usable_memory() -> u64 {
    ((unsafe { MEMORY_MAP.get_unchecked() }
        .iter()
        .filter(|entry| entry.entry_type == EntryType::USABLE)
        .map(|entry| entry.length)
        .sum::<u64>()
        >> 20)
        - 1)
        * 1024
        * 1024
}

pub fn get_total_heap_size() -> usize {
    ALLOCATOR.lock().size()
}

pub fn get_used_heap_size() -> usize {
    ALLOCATOR.lock().used()
}
