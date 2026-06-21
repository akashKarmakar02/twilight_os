use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::preempt::PreemptGuard;
use core::alloc::{GlobalAlloc, Layout};
use linked_list_allocator::LockedHeap;
use x86_64::VirtAddr;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB, mapper::MapToError,
};

#[global_allocator]
static ALLOCATOR: PreemptSafeHeap = PreemptSafeHeap::empty();

struct PreemptSafeHeap {
    inner: LockedHeap,
}

impl PreemptSafeHeap {
    const fn empty() -> Self {
        Self {
            inner: LockedHeap::empty(),
        }
    }
}

// SAFETY: this delegates allocation semantics to LockedHeap unchanged. The
// added guard only prevents task preemption while LockedHeap's internal spin
// lock is acquired or held.
unsafe impl GlobalAlloc for PreemptSafeHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _preempt_guard = PreemptGuard::new_no_resched();
        // SAFETY: forwarded with the exact layout supplied by GlobalAlloc.
        unsafe { GlobalAlloc::alloc(&self.inner, layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _preempt_guard = PreemptGuard::new_no_resched();
        // SAFETY: ptr/layout are forwarded unchanged to the allocator that
        // produced the allocation.
        unsafe { GlobalAlloc::dealloc(&self.inner, ptr, layout) };
    }
}

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

    let _preempt_guard = PreemptGuard::new_no_resched();
    unsafe {
        ALLOCATOR
            .inner
            .lock()
            .init(heap_start.as_u64() as usize, super::memory_size());
    }

    Ok(())
}

pub fn heap_size() -> usize {
    let _preempt_guard = PreemptGuard::new_no_resched();
    ALLOCATOR.inner.lock().size()
}

pub fn heap_used() -> usize {
    let _preempt_guard = PreemptGuard::new_no_resched();
    ALLOCATOR.inner.lock().used()
}

pub fn heap_free() -> usize {
    let _preempt_guard = PreemptGuard::new_no_resched();
    ALLOCATOR.inner.lock().free()
}
