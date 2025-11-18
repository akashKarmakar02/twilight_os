pub mod bitmap;
pub mod heap;
pub mod phys;

use crate::log;
use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::proc::mem::{PAGE, align_dn, align_up};
use conquer_once::spin::OnceCell;
use core::sync::atomic::Ordering::SeqCst;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use limine::memory_map::Entry;
use spin::Once;
use x86_64::structures::paging::mapper::CleanUp;
use x86_64::structures::paging::{
    FrameAllocator, FrameDeallocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags,
    PhysFrame, Size4KiB, Translate,
};
use x86_64::{PhysAddr, VirtAddr};

#[allow(static_mut_refs)]
static mut MAPPER: Once<OffsetPageTable<'static>> = Once::new();

pub(crate) static mut PHYSICAL_MEMORY_OFFSET: AtomicU64 = AtomicU64::new(0);

static mut KERNEL_PAGE_TABLE_FRAME: PhysFrame = PhysFrame::containing_address(PhysAddr::new(0));
static MEMORY_MAP: OnceCell<&'static [&Entry]> = OnceCell::uninit();
static MEMORY_SIZE: AtomicUsize = AtomicUsize::new(0);

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

fn make_flags(is_writable: bool, is_executable: bool) -> PageTableFlags {
    let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
    if is_writable {
        flags |= PageTableFlags::WRITABLE;
    }
    if !is_executable {
        flags |= PageTableFlags::NO_EXECUTE;
    }
    flags
}

pub fn alloc_pages(
    mapper: &mut OffsetPageTable,
    addr: u64,
    size: usize,
    is_writable: bool,
    is_executable: bool,
) -> Result<(), ()> {
    let size = size.saturating_sub(1) as u64;

    let pages = {
        let start_page: Page = Page::containing_address(VirtAddr::new(addr));
        let end_page: Page = Page::containing_address(VirtAddr::new(addr + size));
        Page::range_inclusive(start_page, end_page)
    };

    let flags = make_flags(is_writable, is_executable);

    with_frame_allocator(|frame_allocator| {
        for page in pages {
            if let Some(frame) = frame_allocator.allocate_frame() {
                let res = unsafe { mapper.map_to(page, frame, flags, frame_allocator) };
                if let Ok(mapping) = res {
                    mapping.flush();
                } else {
                    // log!("Could not map {:?} to {:?}", page, frame);
                    if let Ok(_old_frame) = mapper.translate_page(page) {
                        // log!("Already mapped to {:?}", old_frame);
                    }
                }
            } else {
                log!("Could not allocate frame for {:?}", page);
            }
        }
    });

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
    let flags = make_flags(writable, executable);

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

pub fn memory_size() -> usize {
    MEMORY_SIZE.load(Ordering::Relaxed)
}
