pub mod allocator;
pub mod phys;
pub mod paging;

use crate::{log};
use conquer_once::spin::OnceCell;
use core::sync::atomic::Ordering::SeqCst;
use core::sync::atomic::{AtomicU64, AtomicUsize};
use limine::memory_map::{Entry, EntryType};
use spin::Once;
use x86_64::structures::paging::{FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB, Translate};
use x86_64::{PhysAddr, VirtAddr};

#[allow(static_mut_refs)]
static mut MAPPER: Once<OffsetPageTable<'static>> = Once::new();

static mut PHYSICAL_MEMORY_OFFSET: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_FRAMES: AtomicUsize = AtomicUsize::new(0);
static MEMORY_MAP: OnceCell<&'static[&Entry]> = OnceCell::uninit();


pub fn init(physical_memory_offset: VirtAddr, memory_map: &'static [&Entry]) {
    #[allow(static_mut_refs)]
    unsafe {
        PHYSICAL_MEMORY_OFFSET.store(physical_memory_offset.as_u64(), SeqCst);
    }
    let level_4_table = unsafe { active_level_4_table() };
    #[allow(static_mut_refs)]
    unsafe {
        MAPPER.call_once(|| {
            OffsetPageTable::new(level_4_table, physical_memory_offset)
        });
    }

    let mut frame_allocator = unsafe {
        BootInfoFrameAllocator::init(memory_map)
    };
    MEMORY_MAP.try_init_once(|| memory_map).unwrap();

    allocator::init_heap(&mut frame_allocator).expect("Failed to initialize heap");

}


pub struct BootInfoFrameAllocator {
    memory_map: &'static [&'static Entry],
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_map: &'static [&Entry]) -> Self {
        let allocator = Self {
            memory_map,
        };

        allocator
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        let regions = self.memory_map.iter();
        let usable_regions = regions.filter(|r|
            r.entry_type == EntryType::USABLE
        );
        let addr_ranges = usable_regions.map(|r|
            r.base..(r.base + r.length)
        );
        let frame_addresses = addr_ranges.flat_map(|r|
            r.step_by(4096)
        );
        frame_addresses.map(|addr|
            PhysFrame::containing_address(PhysAddr::new(addr))
        )
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let next = ALLOCATED_FRAMES.fetch_add(1, SeqCst);
        // FIXME: When the heap is larger than a few megabytes,
        // creating an iterator for each allocation become very slow.
        self.usable_frames().nth(next)
    }
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
    unsafe { MAPPER.get_mut_unchecked() }
}

pub fn phys_mem_offset() -> u64 {
    #[allow(static_mut_refs)]
    unsafe { PHYSICAL_MEMORY_OFFSET.load(SeqCst) }
}


pub fn phys_to_virt(addr: PhysAddr) -> VirtAddr {
    VirtAddr::new(addr.as_u64() + phys_mem_offset())
}

pub fn virt_to_phys(addr: VirtAddr) -> Option<PhysAddr> {
    mapper().translate_addr(addr)
}

pub fn frame_allocator() -> BootInfoFrameAllocator {
    unsafe { BootInfoFrameAllocator::init(MEMORY_MAP.get_unchecked()) }
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
    let mut frame_allocator = frame_allocator();

    let pages = {
        let start_page: Page = Page::containing_address(VirtAddr::new(addr));
        let end_page: Page = Page::containing_address(VirtAddr::new(addr + size));
        Page::range_inclusive(start_page, end_page)
    };

    let flags = make_flags(is_writable, is_executable);

    for page in pages {
        if let Some(frame) = frame_allocator.allocate_frame() {
            // serial_prtinln!("{:?} to {:?} with flags {:?}", page, frame, flags);
            let res = unsafe { mapper.map_to(page, frame, flags, &mut frame_allocator) };
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
            return Err(());
        }
    }

    Ok(())
}

pub fn dealloc_pages(
    mapper: &mut OffsetPageTable,
    addr: u64,
    size: usize,
) -> Result<(), ()> {
    let size = size.saturating_sub(1) as u64;
    let start_page: Page = Page::containing_address(VirtAddr::new(addr));
    let end_page: Page = Page::containing_address(VirtAddr::new(addr + size));
    let pages = Page::range_inclusive(start_page, end_page);
    
    for page in pages {
        if let Ok((_frame, mapping)) = mapper.unmap(page) {
            mapping.flush();
            // serial_prtinln!("unmapped page {:?} to frame {:?}", page, frame);
        }
    }
    
    Ok(())
}

pub fn phys_addr(ptr: *const u8) -> u64 {
    let virt_addr = VirtAddr::new(ptr as u64);
    let phys_addr = virt_to_phys(virt_addr).unwrap();
    phys_addr.as_u64()
}