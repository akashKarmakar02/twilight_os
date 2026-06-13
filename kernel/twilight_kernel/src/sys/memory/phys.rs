use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::memory::phys_to_virt;
use core::ptr::NonNull;
use x86_64::structures::paging::{FrameDeallocator, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

#[derive(Debug)]
pub struct PhysBuf {
    phys: PhysAddr,
    virt: NonNull<u8>,
    len: usize,
    pages: usize,
}

unsafe impl Send for PhysBuf {}
unsafe impl Sync for PhysBuf {}

impl PhysBuf {
    pub fn new(len: usize) -> Self {
        // Round to nearest page size (for DMA-safe alloc)
        let aligned_len = (len + 0xFFF) & !0xFFF;

        let num_pages = aligned_len / 0x1000;
        let first_frame = with_frame_allocator(|frame_allocator| {
            frame_allocator
                .allocate_contiguous(num_pages)
                .expect("Out of contiguous DMA memory")
        });

        let phys = first_frame.start_address();
        let virt = phys_to_virt(phys).as_mut_ptr();
        // let page = Page::containing_address(virt);
        // unsafe {
        //     mapper().map_to(page, virt);
        // }

        Self {
            phys,
            virt: NonNull::new(virt).expect("Failed to map phys addr"),
            len,
            pages: num_pages,
        }
    }

    pub fn new_dma32(len: usize) -> Self {
        // Bus-master IDE uses 32-bit physical addresses.
        let aligned_len = (len + 0xFFF) & !0xFFF;
        let num_pages = aligned_len / 0x1000;
        let first_frame = with_frame_allocator(|frame_allocator| {
            frame_allocator
                .allocate_contiguous_below(num_pages, u32::MAX as u64)
                .expect("Out of 32-bit DMA memory")
        });

        let phys = first_frame.start_address();
        let virt = phys_to_virt(phys).as_mut_ptr();
        Self {
            phys,
            virt: NonNull::new(virt).expect("Failed to map phys addr"),
            len,
            pages: num_pages,
        }
    }

    pub fn virt_addr(&self) -> VirtAddr {
        phys_to_virt(self.phys)
    }

    pub fn addr(&self) -> u64 {
        self.phys.as_u64()
    }
}

impl Drop for PhysBuf {
    fn drop(&mut self) {
        let start = self.phys.as_u64();
        let pages = self.pages;
        with_frame_allocator(|frame_allocator| {
            for page_idx in 0..pages {
                let addr = PhysAddr::new(start + (page_idx as u64) * 0x1000);
                let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(addr);
                unsafe { frame_allocator.deallocate_frame(frame) };
            }
        });
    }
}

impl core::ops::Deref for PhysBuf {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.virt.as_ptr(), self.len) }
    }
}

impl core::ops::DerefMut for PhysBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.virt.as_ptr(), self.len) }
    }
}
