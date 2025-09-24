mod addr;
mod frame;
mod mapper;
mod page;
mod page_table;

pub use self::addr::*;
pub use self::frame::*;
pub use self::mapper::*;
pub use self::page::*;
pub use self::page_table::*;
use core::sync::atomic::Ordering;
use x86_64::registers::control::{Cr3, Cr4, Cr4Flags};

use crate::memory::PHYSICAL_MEMORY_OFFSET;

pub static FRAME_ALLOCATOR: LockedFrameAllocator = LockedFrameAllocator::new_uninit();

bitflags::bitflags! {
    #[derive(Debug, Copy, Clone)]
    #[repr(transparent)]
    pub struct PageFaultErrorCode: u64 {
        const PROTECTION_VIOLATION = 1;
        const CAUSED_BY_WRITE = 1 << 1;

        const USER_MODE = 1 << 2;

        const MALFORMED_TABLE = 1 << 3;

        const INSTRUCTION_FETCH = 1 << 4;
    }
}

#[cfg(target_arch = "x86_64")]
pub fn level_5_paging_enabled() -> bool {
    Cr4::read().contains(Cr4Flags::L5_PAGING)
}

#[cfg(target_arch = "aarch64")]
pub const fn level_5_paging_enabled() -> bool {
    false
}

pub fn init(
    mmap_resp: &mut limine::response::MemoryMapResponse,
) -> Result<OffsetPageTable<'static>, MapToError<Size4KiB>> {
    let active_level_4 = active_level_4_table();
    #[allow(static_mut_refs)]
    let offset_table = unsafe {
        OffsetPageTable::new(
            active_level_4,
            VirtAddr::new(PHYSICAL_MEMORY_OFFSET.load(Ordering::SeqCst)),
        )
    };

    FRAME_ALLOCATOR.init(mmap_resp);
    Ok(offset_table)
}

/// Get a mutable reference to the active level 4 page table.
#[cfg(target_arch = "x86_64")]
pub fn active_level_4_table() -> &'static mut PageTable {
    let (level_4_table_frame, _) = Cr3::read();
    let physical_addr = level_4_table_frame.start_address();
    #[allow(static_mut_refs)]
    let virtual_address = unsafe {
        VirtAddr::new(physical_addr.as_u64() + PHYSICAL_MEMORY_OFFSET.load(Ordering::SeqCst))
    };

    let page_table_ptr: *mut PageTable = virtual_address.as_mut_ptr();

    unsafe { &mut *page_table_ptr }
}

#[cfg(target_arch = "aarch64")]
pub unsafe fn active_level_4_table() -> &'static mut PageTable {
    unimplemented!()
}
