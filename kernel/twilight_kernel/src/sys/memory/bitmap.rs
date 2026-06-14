use crate::log;
use bit_field::BitField;
use core::{cmp, slice};
use limine::memory_map::EntryType;
use spin::{Mutex, Once};
use x86_64::structures::paging::{FrameAllocator, FrameDeallocator, PhysFrame, Size4KiB};
use x86_64::PhysAddr;

#[derive(Debug, Clone, Copy, PartialEq)]
struct UsableRegion {
    first_frame: PhysFrame,
    frame_count: usize,
}

impl UsableRegion {
    // NOTE: end_addr is exclusive
    pub fn new(start_addr: u64, end_addr: u64) -> Self {
        let first_frame = frame_at(start_addr);
        let last_frame = frame_at(end_addr - 1);
        let a = first_frame.start_address();
        let b = last_frame.start_address();
        let frame_count = ((b - a) / 4096) as usize + 1;

        Self {
            first_frame,
            frame_count,
        }
    }

    pub fn first_frame(&self) -> PhysFrame {
        self.first_frame
    }

    pub fn last_frame(&self) -> PhysFrame {
        self.first_frame + (self.frame_count - 1) as u64
    }

    pub fn len(&self) -> usize {
        self.frame_count
    }

    pub fn contains(&self, frame: PhysFrame) -> bool {
        self.first_frame() <= frame && frame <= self.last_frame()
    }

    pub fn offset(&self, frame: PhysFrame) -> usize {
        let addr = frame.start_address() - self.first_frame.start_address();
        (addr / 4096) as usize
    }
}

fn frame_at(addr: u64) -> PhysFrame<Size4KiB> {
    PhysFrame::containing_address(PhysAddr::new(addr))
}

static FRAME_ALLOCATOR: Once<Mutex<BitmapFrameAllocator>> = Once::new();

pub fn init_frame_allocator(memory_map: &[&limine::memory_map::Entry]) {
    FRAME_ALLOCATOR.call_once(|| Mutex::new(BitmapFrameAllocator::init(memory_map)));
}

const MAX_REGIONS: usize = 32;

pub struct BitmapFrameAllocator {
    bitmap: &'static mut [u64],
    next_free_index: usize,
    usable_regions: [Option<UsableRegion>; MAX_REGIONS],
    regions_count: usize,
    frames_count: usize,
}

impl BitmapFrameAllocator {
    pub fn init(memory_map: &[&limine::memory_map::Entry]) -> Self {
        let mut bitmap_addr = None;

        let frames_count: usize = memory_map
            .iter()
            .map(|region| {
                if region.entry_type == EntryType::USABLE {
                    let size = region.length;
                    debug_assert_eq!(size % 4096, 0);
                    (size / 4096) as usize
                } else {
                    0
                }
            })
            .sum();
        let bitmap_size = ((frames_count + 63) / 64) * 8;
        let bitmap_storage_size = (bitmap_size + 4095) & !4095;

        let mut allocator = Self {
            bitmap: &mut [],
            next_free_index: 0,
            usable_regions: [None; MAX_REGIONS],
            regions_count: 0,
            frames_count: 0,
        };

        for region in memory_map.iter() {
            if region.entry_type != EntryType::USABLE {
                continue;
            }

            let region_start = region.base;
            let region_end = region.base + region.length;
            let region_size = (region_end - region_start) as usize;

            // Try to place the bitmap in the region
            if bitmap_addr.is_none() && region_size >= bitmap_storage_size {
                bitmap_addr = Some(region_start);

                let addr = super::phys_to_virt(PhysAddr::new(region_start));
                let ptr = addr.as_mut_ptr();
                let len = bitmap_size / 8;
                unsafe {
                    allocator.bitmap = slice::from_raw_parts_mut(ptr, len);
                    allocator.bitmap.fill(0);
                }
            }

            // Calculate usable portion
            let (usable_start, usable_end) = match bitmap_addr {
                Some(addr) if region_start == addr => {
                    // Reserve every frame touched by the bitmap. Otherwise a
                    // non-page-aligned bitmap tail can be allocated and zeroed.
                    let bitmap_end = region_start + bitmap_storage_size as u64;
                    if bitmap_end >= region_end {
                        continue; // Entire region consumed by the bitmap
                    }
                    (bitmap_end, region_end)
                }
                _ => (region_start, region_end),
            };

            if usable_end - usable_start >= 4096 {
                if allocator.regions_count >= MAX_REGIONS {
                    log!("MEM: Could not add usable region");
                    break;
                }
                let r = UsableRegion::new(usable_start, usable_end);
                allocator.usable_regions[allocator.regions_count] = Some(r);
                allocator.regions_count += 1;
                allocator.frames_count += r.len();
            }
        }

        if bitmap_addr.is_none() {
            panic!("MEM: No usable region large enough to host bitmap");
        }

        allocator
    }

    fn index_to_frame(&self, index: usize) -> Option<PhysFrame> {
        if index >= self.frames_count {
            return None;
        }

        let mut base = 0;
        for i in 0..self.regions_count {
            if let Some(region) = self.usable_regions[i] {
                if index < base + region.len() {
                    let frame_offset = index - base;
                    return Some(region.first_frame() + frame_offset as u64);
                }
                base += region.len();
            }
        }
        None
    }

    fn frame_to_index(&self, frame: PhysFrame) -> Option<usize> {
        let mut base = 0;
        for i in 0..self.regions_count {
            if let Some(region) = self.usable_regions[i] {
                if region.contains(frame) {
                    let frame_offset = region.offset(frame);
                    return Some(base + frame_offset);
                }
                base += region.len();
            }
        }
        None
    }

    fn is_frame_allocated(&self, index: usize) -> bool {
        let word_index = index / 64;
        let bit_index = index % 64;
        self.bitmap[word_index].get_bit(bit_index)
    }

    fn set_frame_allocated(&mut self, index: usize, allocated: bool) {
        let word_index = index / 64;
        let bit_index = index % 64;
        self.bitmap[word_index].set_bit(bit_index, allocated);
    }

    pub fn total_frames(&self) -> usize {
        self.frames_count
    }

    pub fn allocated_frames(&self) -> usize {
        if self.frames_count == 0 {
            return 0;
        }
        let full_words = self.frames_count / 64;
        let rem_bits = self.frames_count % 64;
        let mut used: usize = 0;
        for i in 0..full_words {
            used += self.bitmap[i].count_ones() as usize;
        }
        if rem_bits != 0 {
            let mask = (1u64 << rem_bits) - 1;
            used += (self.bitmap[full_words] & mask).count_ones() as usize;
        }
        used
    }

    pub fn free_frames(&self) -> usize {
        self.total_frames().saturating_sub(self.allocated_frames())
    }

    /// Allocate `num_pages` physically-contiguous 4KiB frames.
    pub fn allocate_contiguous(&mut self, num_pages: usize) -> Option<PhysFrame<Size4KiB>> {
        if num_pages == 0 {
            return None;
        }

        let mut base = 0usize;
        for region_idx in 0..self.regions_count {
            let Some(region) = self.usable_regions[region_idx] else {
                continue;
            };

            let region_len = region.len();
            if region_len < num_pages {
                base += region_len;
                continue;
            }

            let max_start = region_len - num_pages;
            let start0 = if self.next_free_index >= base && self.next_free_index < base + region_len
            {
                (self.next_free_index - base).min(max_start)
            } else {
                0
            };

            for pass in 0..2 {
                let (start, end_excl) = if pass == 0 {
                    (start0, max_start + 1)
                } else {
                    (0, start0.min(max_start + 1))
                };

                for start_off in start..end_excl {
                    let start_index = base + start_off;

                    let mut ok = true;
                    for j in 0..num_pages {
                        if self.is_frame_allocated(start_index + j) {
                            ok = false;
                            break;
                        }
                    }
                    if !ok {
                        continue;
                    }

                    for j in 0..num_pages {
                        self.set_frame_allocated(start_index + j, true);
                    }
                    self.next_free_index = start_index + num_pages;
                    return Some(region.first_frame() + start_off as u64);
                }
            }

            base += region_len;
        }

        None
    }

    /// Allocate `num_pages` physically-contiguous 4KiB frames whose last byte is
    /// at or below `max_phys_addr_inclusive`.
    pub fn allocate_contiguous_below(
        &mut self,
        num_pages: usize,
        max_phys_addr_inclusive: u64,
    ) -> Option<PhysFrame<Size4KiB>> {
        if num_pages == 0 {
            return None;
        }

        let mut base = 0usize;
        for region_idx in 0..self.regions_count {
            let Some(region) = self.usable_regions[region_idx] else {
                continue;
            };

            let region_len = region.len();
            if region_len < num_pages {
                base += region_len;
                continue;
            }

            let region_start = region.first_frame().start_address().as_u64();
            if region_start > max_phys_addr_inclusive {
                base += region_len;
                continue;
            }

            // Limit how much of this region we can use before exceeding max_phys.
            let max_pages_by_addr =
                ((max_phys_addr_inclusive + 1).saturating_sub(region_start) / 0x1000) as usize;
            let region_len_limited = region_len.min(max_pages_by_addr);
            if region_len_limited < num_pages {
                base += region_len;
                continue;
            }

            let max_start = region_len_limited - num_pages;
            let start0 = if self.next_free_index >= base
                && self.next_free_index < base + region_len_limited
            {
                (self.next_free_index - base).min(max_start)
            } else {
                0
            };

            for pass in 0..2 {
                let (start, end_excl) = if pass == 0 {
                    (start0, max_start + 1)
                } else {
                    (0, start0.min(max_start + 1))
                };

                for start_off in start..end_excl {
                    let start_index = base + start_off;

                    let mut ok = true;
                    for j in 0..num_pages {
                        if self.is_frame_allocated(start_index + j) {
                            ok = false;
                            break;
                        }
                    }
                    if !ok {
                        continue;
                    }

                    for j in 0..num_pages {
                        self.set_frame_allocated(start_index + j, true);
                    }
                    self.next_free_index = start_index + num_pages;
                    return Some(region.first_frame() + start_off as u64);
                }
            }

            base += region_len;
        }

        None
    }
}

unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        for i in 0..self.frames_count {
            let index = (self.next_free_index + i) % self.frames_count;
            if !self.is_frame_allocated(index) {
                self.set_frame_allocated(index, true);
                self.next_free_index = index + 1;
                return self.index_to_frame(index);
            }
        }
        None // No free frames
    }
}

impl FrameDeallocator<Size4KiB> for BitmapFrameAllocator {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        if let Some(index) = self.frame_to_index(frame) {
            if self.is_frame_allocated(index) {
                self.set_frame_allocated(index, false);
                self.next_free_index = cmp::min(self.next_free_index, index);
            } else {
                //panic!("Double free detected");
            }
        } else {
            //panic!("Deallocating a frame not managed by the allocator");
        }
    }
}

pub fn frame_allocator() -> &'static Mutex<BitmapFrameAllocator> {
    FRAME_ALLOCATOR
        .get()
        .expect("frame allocator not initialized")
}

pub fn with_frame_allocator<F, R>(f: F) -> R
where
    F: FnOnce(&mut BitmapFrameAllocator) -> R,
{
    let mut allocator = frame_allocator().lock();
    f(&mut allocator)
}
