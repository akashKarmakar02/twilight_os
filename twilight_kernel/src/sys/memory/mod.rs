pub mod allocator;
pub mod paging;
pub mod phys;
pub mod slab;
pub mod vmalloc;

use crate::sys::memory::paging::{FrameAllocator, MapToError, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysAddr, PhysFrame, Translate, VirtAddr, FRAME_ALLOCATOR};
use crate::{log, sys};
use conquer_once::spin::OnceCell;
use core::arch::asm;
use core::sync::atomic::Ordering::SeqCst;
use core::sync::atomic::{AtomicU64, AtomicUsize};
use limine::memory_map::{Entry, EntryType};
use spin::Once;
use x86_64::structures::paging::Size4KiB;

#[allow(static_mut_refs)]
static mut MAPPER: Once<OffsetPageTable<'static>> = Once::new();

pub(crate) static mut PHYSICAL_MEMORY_OFFSET: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_FRAMES: AtomicUsize = AtomicUsize::new(0);
static mut KERNEL_PAGE_TABLE_FRAME: Once<PhysFrame> = Once::new();
static MEMORY_MAP: OnceCell<&'static [&Entry]> = OnceCell::uninit();

pub fn init(physical_memory_offset: VirtAddr, memory_map: &'static [&Entry]) {
    let level_4_table = unsafe { active_level_4_table() };
    let (frame, _) = x86_64::registers::control::Cr3::read();
    let frame = PhysFrame::containing_address(PhysAddr::new(frame.start_address().as_u64()));
    #[allow(static_mut_refs)]
    unsafe {
        KERNEL_PAGE_TABLE_FRAME.call_once(|| {
            frame
        });
    }
    #[allow(static_mut_refs)]
    unsafe {
        MAPPER.call_once(|| OffsetPageTable::new(level_4_table, physical_memory_offset));
    }

    MEMORY_MAP.try_init_once(|| memory_map).unwrap();

    allocator::init_heap();
}

pub struct AddressSpace {
    cr3: sys::memory::paging::PhysFrame,
}

impl AddressSpace {
    /// Allocates a new *virtual* address space.
    pub fn new() -> Result<Self, MapToError<Size4KiB>> {
        let cr3 = unsafe {
            let frame = FRAME_ALLOCATOR
                .allocate_frame()
                .ok_or(MapToError::FrameAllocationFailed)?;
            let phys_addr = frame.start_address();
            let virt_addr = phys_addr.as_hhdm_virt();
            phys_addr.as_vm_frame().unwrap().inc_ref_count();
            let page_table: *mut PageTable = virt_addr.as_mut_ptr();
            let page_table = &mut *page_table;
            let current_table = active_level_4_table();
            for i in 0..256 {
                page_table[i].set_unused();
            }
            for i in 256..512 {
                page_table[i] = current_table[i].clone();
            }
            frame
        };
        Ok(Self { cr3 })
    }
    pub fn this() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let cr3 = {
                // Get the value of the Cr3 register.
                let value: u64;
                unsafe {
                    asm!("mov {}, cr3", out(reg) value, options(nomem));
                }

                let addr = paging::PhysAddr::new(value & 0x_000f_ffff_ffff_f000);
                paging::PhysFrame::containing_address(addr)
            };
            Self { cr3 }
        }
        #[cfg(target_arch = "aarch64")]
        unimplemented!()
    }
    pub fn switch(&self) {
        #[cfg(target_arch = "x86_64")]
        {
            let cr3 = self.cr3().start_address().as_u64();
            unsafe {
                asm!("mov cr3, {}", in(reg) cr3, options(nostack)); // Load the new address space
            }
        }
        #[cfg(target_arch = "aarch64")]
        unimplemented!()
    }

    pub fn cr3(&self) -> paging::PhysFrame {
        self.cr3
    }

    pub fn page_table(&mut self) -> &'static mut PageTable {
        unsafe { &mut *(self.cr3.start_address().as_hhdm_virt().as_mut_ptr()) }
    }

    pub fn offset_page_table(&mut self) -> OffsetPageTable {
        #[allow(static_mut_refs)]
        unsafe { OffsetPageTable::new(self.page_table(), VirtAddr::new(PHYSICAL_MEMORY_OFFSET.load(SeqCst))) }
    }
}

pub struct BootInfoFrameAllocator {
    memory_map: &'static [&'static Entry],
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_map: &'static [&Entry]) -> Self {
        let allocator = Self { memory_map };

        allocator
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> {
        let regions = self.memory_map.iter();
        let usable_regions = regions.filter(|r| r.entry_type == EntryType::USABLE);
        let addr_ranges = usable_regions.map(|r| r.base..(r.base + r.length));
        let frame_addresses = addr_ranges.flat_map(|r| r.step_by(4096));
        frame_addresses.map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

pub(crate) fn kernel_page_table() -> &'static mut PageTable {
    #[allow(static_mut_refs)]
    let frame = unsafe { KERNEL_PAGE_TABLE_FRAME.get_mut().unwrap() };

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
    let frame = PhysFrame::containing_address(PhysAddr::new(frame.start_address().as_u64()));
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

pub fn phys_addr(ptr: *const u8) -> u64 {
    let virt_addr = VirtAddr::new(ptr as u64);
    let phys_addr = virt_to_phys(virt_addr).unwrap();
    phys_addr.as_u64()
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
        Page::range(start_page, end_page)
    };

    let flags = make_flags(is_writable, is_executable);

    for page in pages {
        if let Some(frame) = FRAME_ALLOCATOR.allocate_frame() {
            // serial_prtinln!("{:?} to {:?} with flags {:?}", page, frame, flags);
            let res = unsafe { mapper.map_to(page, frame, flags) };
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
    let pages = Page::range(start_page, end_page);

    for page in pages {
        if let Ok((mapping)) = mapper.unmap(page) {
            mapping.flush();
            // serial_prtinln!("unmapped page {:?} to frame {:?}", page, frame);
        }
    }

    Ok(())
}