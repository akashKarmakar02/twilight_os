pub mod allocator;

use x86_64::structures::paging::{FrameAllocator, OffsetPageTable, PageTable, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};
use limine::memory_map::{Entry, EntryType};

pub struct BootInfoFrameAllocator {
    memory_map: &'static [&'static Entry],
    region_index: usize,
    current_addr: u64,
}

impl BootInfoFrameAllocator {
    pub unsafe fn init(memory_map: &'static [&Entry]) -> Self {
        let mut allocator = Self {
            memory_map,
            region_index: 0,
            current_addr: 0,
        };

        // Skip to first usable region
        allocator.skip_to_next_usable_region();
        allocator
    }

    fn skip_to_next_usable_region(&mut self) {
        while self.region_index < self.memory_map.len() {
            let region = self.memory_map[self.region_index];
            if region.entry_type == EntryType::USABLE {
                self.current_addr = region.base;
                return;
            }
            self.region_index += 1;
        }
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        while self.region_index < self.memory_map.len() {
            let region = self.memory_map[self.region_index];

            if region.entry_type != EntryType::USABLE {
                self.region_index += 1;
                continue;
            }

            let end = region.base + region.length;
            if self.current_addr < end {
                let frame = PhysFrame::containing_address(PhysAddr::new(self.current_addr));
                self.current_addr += 4096;
                return Some(frame);
            } else {
                self.region_index += 1;
                self.skip_to_next_usable_region();
            }
        }

        None
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}
