use crate::sys::memory::{phys_to_virt};
use core::ptr::NonNull;
use x86_64::structures::paging::FrameAllocator;
use x86_64::PhysAddr;
use crate::sys::memory::bitmap::with_frame_allocator;

#[derive(Clone, Debug)]
pub struct PhysBuf {
    phys: PhysAddr,
    virt: NonNull<u8>,
    len: usize,
}

unsafe impl Send for PhysBuf {}
unsafe impl Sync for PhysBuf {}

impl PhysBuf {
    pub fn new(len: usize) -> Self {
        // Round to nearest page size (for DMA-safe alloc)
        let aligned_len = (len + 0xFFF) & !0xFFF;

        let num_pages = aligned_len / 0x1000;
        let mut first_frame = None;

        with_frame_allocator(|frame_allocator| {
            for i in 0..num_pages {
                let frame = frame_allocator.allocate_frame().expect("Out of memory");
                if i == 0 {
                    first_frame = Some(frame);
                }
            }
        });

        let phys = first_frame.unwrap().start_address();
        let virt = phys_to_virt(phys).as_mut_ptr();

        Self {
            phys,
            virt: NonNull::new(virt).expect("Failed to map phys addr"),
            len,
        }
    }

    pub fn addr(&self) -> u64 {
        self.phys.as_u64()
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
