pub mod bitmap;
pub mod heap;
pub mod phys;

use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::proc::mem::{align_dn, align_up, PAGE};
use crate::{log, serial_println};
use conquer_once::spin::OnceCell;
use core::ptr;
use core::sync::atomic::Ordering::SeqCst;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use limine::memory_map::Entry;
use spin::Once;
use x86_64::structures::paging::mapper::CleanUp;
use x86_64::structures::paging::{
    FrameAllocator, FrameDeallocator, Mapper, OffsetPageTable, Page, PageSize, PageTable,
    PageTableFlags, PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

#[allow(static_mut_refs)]
static mut MAPPER: Once<OffsetPageTable<'static>> = Once::new();

pub(crate) static mut PHYSICAL_MEMORY_OFFSET: AtomicU64 = AtomicU64::new(0);

static mut KERNEL_PAGE_TABLE_FRAME: PhysFrame = PhysFrame::containing_address(PhysAddr::new(0));
static MEMORY_MAP: OnceCell<&'static [&Entry]> = OnceCell::uninit();
static MEMORY_SIZE: AtomicUsize = AtomicUsize::new(0);
pub const PAGE_SIZE: usize = 4096;

pub fn mem_stats_bytes() -> (usize, usize) {
    // Total = total usable frames managed by allocator.
    // Free = frames not allocated in the bitmap.
    let (total_frames, free_frames) = with_frame_allocator(|a| (a.total_frames(), a.free_frames()));
    let total = total_frames.saturating_mul(4096);
    let free = free_frames.saturating_mul(4096);
    (total, free)
}

pub fn init(physical_memory_offset: VirtAddr, memory_map: &'static [&Entry]) {
    let level_4_table = unsafe { active_level_4_table() };
    let (frame, _) = x86_64::registers::control::Cr3::read();
    #[allow(static_mut_refs)]
    unsafe {
        KERNEL_PAGE_TABLE_FRAME = frame;
    }
    #[allow(static_mut_refs)]
    unsafe {
        MAPPER.call_once(|| OffsetPageTable::new(level_4_table, physical_memory_offset));
    }

    let mut memory_size = 0;
    let mut last_end_addr = 0;
    for region in memory_map {
        let start_addr = region.base;
        let end_addr = region.base + region.length;
        let size = end_addr - start_addr;
        let hole = start_addr - last_end_addr;
        if hole > 0 {
            log!(
                "MEM [{:#016X}-{:#016X}] {}", // "({} KB)"
                last_end_addr,
                start_addr - 1,
                "Unmapped" //, hole >> 10
            );
            if start_addr < (1 << 20) {
                memory_size += hole as usize; // BIOS memory
            }
        }
        memory_size += size as usize;
        last_end_addr = end_addr;
    }

    MEMORY_SIZE.store(memory_size, SeqCst);

    bitmap::init_frame_allocator(memory_map);
    MEMORY_MAP.try_init_once(|| memory_map).unwrap();

    heap::init_heap().expect("Failed to initialize heap");
}

pub(crate) fn kernel_page_table() -> &'static mut PageTable {
    #[allow(static_mut_refs)]
    let frame = unsafe { KERNEL_PAGE_TABLE_FRAME };

    let phys = frame.start_address();
    let virt = VirtAddr::new(phys.as_u64() + phys_mem_offset());
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    unsafe { &mut *page_table_ptr }
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn active_level_4_table() -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = VirtAddr::new(phys.as_u64() + phys_mem_offset());
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}

pub fn mapper() -> &'static mut OffsetPageTable<'static> {
    #[allow(static_mut_refs)]
    unsafe {
        MAPPER.get_mut_unchecked()
    }
}

pub fn phys_mem_offset() -> u64 {
    #[allow(static_mut_refs)]
    unsafe {
        PHYSICAL_MEMORY_OFFSET.load(SeqCst)
    }
}

pub fn phys_to_virt(addr: PhysAddr) -> VirtAddr {
    VirtAddr::new(addr.as_u64() + phys_mem_offset())
}

pub fn virt_to_phys(addr: VirtAddr) -> Option<PhysAddr> {
    mapper().translate_addr(addr)
}

pub fn create_page_table(frame: PhysFrame) -> &'static mut PageTable {
    let phys_addr = frame.start_address();
    let virt_addr = phys_to_virt(phys_addr);
    let page_table_ptr = virt_addr.as_mut_ptr();
    unsafe { &mut *page_table_ptr }
}

pub fn get_page_table_frame() -> PhysFrame {
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    frame
}

pub fn user_page_flags(is_writable: bool, is_executable: bool) -> PageTableFlags {
    user_page_flags_with_access(true, is_writable, is_executable)
}

pub fn user_page_flags_with_access(
    user_accessible: bool,
    is_writable: bool,
    is_executable: bool,
) -> PageTableFlags {
    let mut flags = PageTableFlags::PRESENT;
    if user_accessible {
        flags |= PageTableFlags::USER_ACCESSIBLE;
    }
    if is_writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !is_executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }
    flags
}

pub fn allocate_zeroed_frame() -> Option<PhysFrame<Size4KiB>> {
    with_frame_allocator(|frame_allocator| {
        let frame = frame_allocator.allocate_frame()?;
        let frame_ptr = phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
        // SAFETY: the frame was just allocated exclusively to the caller and
        // the physical-memory mapping covers the full 4 KiB frame.
        unsafe {
            ptr::write_bytes(frame_ptr, 0, Size4KiB::SIZE as usize);
        }
        Some(frame)
    })
}

pub fn deallocate_frame(frame: PhysFrame<Size4KiB>) {
    // SAFETY: callers only pass frames they own and have not mapped elsewhere.
    unsafe {
        with_frame_allocator(|frame_allocator| frame_allocator.deallocate_frame(frame));
    }
}

pub fn map_user_frame(
    mapper: &mut OffsetPageTable,
    addr: u64,
    frame: PhysFrame<Size4KiB>,
    flags: PageTableFlags,
) -> Result<(), ()> {
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
    with_frame_allocator(|frame_allocator| {
        // SAFETY: `frame` is exclusively owned by the caller, `page` is a
        // userspace page, and the supplied flags describe that mapping.
        unsafe {
            mapper
                .map_to(page, frame, flags, frame_allocator)
                .map_err(|_| ())?
                .flush();
        }
        Ok(())
    })
}

pub fn update_user_page_flags(
    mapper: &mut OffsetPageTable,
    addr: u64,
    flags: PageTableFlags,
) -> Result<(), ()> {
    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
    if mapper.translate_page(page).is_err() {
        return Ok(());
    }

    // SAFETY: the page is present in this process's page table and `flags`
    // retain the required PRESENT bit.
    unsafe {
        mapper.update_flags(page, flags).map_err(|_| ())?.flush();
    }
    Ok(())
}

pub fn alloc_pages(
    mapper: &mut OffsetPageTable,
    addr: u64,
    size: usize,
    is_writable: bool,
    is_executable: bool,
) -> Result<(), ()> {
    alloc_pages_inner(mapper, addr, size, is_writable, is_executable, true)
}

/// Maps pages without invalidating the TLB.
///
/// Use only while constructing a fresh address space that has no cached
/// translations and has not started executing userspace code.
pub fn alloc_pages_unflushed(
    mapper: &mut OffsetPageTable,
    addr: u64,
    size: usize,
    is_writable: bool,
    is_executable: bool,
) -> Result<(), ()> {
    alloc_pages_inner(mapper, addr, size, is_writable, is_executable, false)
}

fn alloc_pages_inner(
    mapper: &mut OffsetPageTable,
    addr: u64,
    size: usize,
    is_writable: bool,
    is_executable: bool,
    flush: bool,
) -> Result<(), ()> {
    let size = size.saturating_sub(1) as u64;

    let pages = {
        let start_page: Page = Page::containing_address(VirtAddr::new(addr));
        let end_page: Page = Page::containing_address(VirtAddr::new(addr + size));
        Page::range_inclusive(start_page, end_page)
    };

    let flags = user_page_flags(is_writable, is_executable);

    with_frame_allocator(|frame_allocator| -> Result<(), ()> {
        for page in pages {
            let Some(frame) = frame_allocator.allocate_frame() else {
                log!("Could not allocate frame for {:?}", page);
                return Err(());
            };

            let frame_ptr = phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
            unsafe {
                ptr::write_bytes(frame_ptr, 0, Size4KiB::SIZE as usize);
            }

            let res = unsafe { mapper.map_to(page, frame, flags, frame_allocator) };
            if let Ok(mapping) = res {
                if flush {
                    mapping.flush();
                } else {
                    mapping.ignore();
                }
            } else if mapper.translate_page(page).is_ok() {
                unsafe {
                    frame_allocator.deallocate_frame(frame);
                }
                if let Ok(mapping) = unsafe { mapper.update_flags(page, flags) } {
                    if flush {
                        mapping.flush();
                    } else {
                        mapping.ignore();
                    }
                } else {
                    serial_println!("Failed to update page flag for {:?}", page);
                    return Err(());
                }
            } else {
                serial_println!("Failed to map user page {:?}", page);
                unsafe {
                    frame_allocator.deallocate_frame(frame);
                }
                return Err(());
            }
        }
        Ok(())
    })?;

    Ok(())
}

pub fn dealloc_pages(mapper: &mut OffsetPageTable, addr: u64, size: usize) -> Result<(), ()> {
    let size = size.saturating_sub(1) as u64;
    let start_page: Page = Page::containing_address(VirtAddr::new(addr));
    let end_page: Page = Page::containing_address(VirtAddr::new(addr + size));
    let pages = Page::range_inclusive(start_page, end_page);

    for page in pages {
        if let Ok((frame, mapping)) = mapper.unmap(page) {
            mapping.flush();
            unsafe {
                with_frame_allocator(|frame_allocator| {
                    mapper.clean_up(frame_allocator);
                    frame_allocator.deallocate_frame(frame);
                });
            }
        }
    }

    Ok(())
}

pub fn unmap_user_pages(mapper: &mut OffsetPageTable, addr: u64, size: usize) -> Result<(), ()> {
    let size = size.saturating_sub(1) as u64;
    let start_page: Page = Page::containing_address(VirtAddr::new(addr));
    let end_page: Page = Page::containing_address(VirtAddr::new(addr + size));
    let pages = Page::range_inclusive(start_page, end_page);

    for page in pages {
        if let Ok((_frame, mapping)) = mapper.unmap(page) {
            mapping.flush();
        }
    }

    Ok(())
}

pub fn map_kernel_buffer(
    mapper: &mut OffsetPageTable,
    kernel_ptr: usize,
    len: usize,
    user_va: usize,
    writable: bool,
    executable: bool,
) -> Result<(), ()> {
    if len == 0 {
        return Err(());
    }

    let start = align_dn(kernel_ptr, PAGE);
    let end = align_up(kernel_ptr.saturating_add(len), PAGE);
    let flags = user_page_flags(writable, executable);

    with_frame_allocator(|frame_allocator| -> Result<(), ()> {
        let mut src = start;
        let mut dst = user_va;
        while src < end {
            let phys = virt_to_phys(VirtAddr::new(src as u64)).ok_or(())?;
            let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(phys);
            let page = Page::containing_address(VirtAddr::new(dst as u64));
            unsafe {
                mapper
                    .map_to(page, frame, flags, frame_allocator)
                    .map_err(|_| ())?
                    .flush();
            }
            src += PAGE;
            dst += PAGE;
        }
        Ok(())
    })
}

pub fn phys_addr(ptr: *const u8) -> u64 {
    let virt_addr = VirtAddr::new(ptr as u64);
    let phys_addr = virt_to_phys(virt_addr).unwrap();
    phys_addr.as_u64()
}

pub fn map_mmio(phys_addr: u64, size: usize) -> Result<(), ()> {
    use x86_64::structures::paging::PageTableFlags;

    let size = size.saturating_sub(1) as u64;
    let start_frame: PhysFrame = PhysFrame::containing_address(PhysAddr::new(phys_addr));
    let end_frame: PhysFrame = PhysFrame::containing_address(PhysAddr::new(phys_addr + size));
    let frames = PhysFrame::range_inclusive(start_frame, end_frame);

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::NO_EXECUTE;

    with_frame_allocator(|frame_allocator| {
        for frame in frames {
            let phys = frame.start_address();
            let virt = VirtAddr::new(phys.as_u64() + phys_mem_offset());
            let page = Page::containing_address(virt);

            unsafe {
                if let Ok(mapping) = mapper().map_to(page, frame, flags, frame_allocator) {
                    mapping.flush();
                } else {
                    // If it failed, it might be already mapped.
                    // We try to update flags just in case, or ignore.
                }
            }
        }
    });
    Ok(())
}

pub fn memory_size() -> usize {
    MEMORY_SIZE.load(Ordering::Relaxed)
}
