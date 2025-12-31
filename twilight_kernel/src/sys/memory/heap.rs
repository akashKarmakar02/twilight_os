use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::{
    mapper::MapToError, FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB
};
use x86_64::VirtAddr;
use crate::sys::memory::bitmap::with_frame_allocator;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub const HEAP_START: u64 = 0x4444_4444_0000;

pub fn init_heap() -> Result<(), MapToError<Size4KiB>> {
    let mapper = super::mapper();

    let heap_size = 100 * 1024 * 1024u64;
    let heap_start = VirtAddr::new(HEAP_START);

    let pages = {
        let heap_end = heap_start + heap_size - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

    with_frame_allocator(|frame_allocator| -> Result<(), MapToError<Size4KiB>> {
        for page in pages {
            let err = MapToError::FrameAllocationFailed;
            let frame = frame_allocator.allocate_frame().ok_or(err)?;
            unsafe {
                mapper.map_to(page, frame, flags, frame_allocator)?.flush();
            }
        }
        Ok(())
    })?;

    unsafe {
        ALLOCATOR.lock().init(heap_start.as_u64() as usize, super::memory_size());
    }

    Ok(())
}

pub fn heap_size() -> usize {
    ALLOCATOR.lock().size()
}

pub fn heap_used() -> usize {
    ALLOCATOR.lock().used()
}

pub fn heap_free() -> usize {
    ALLOCATOR.lock().free()
}
