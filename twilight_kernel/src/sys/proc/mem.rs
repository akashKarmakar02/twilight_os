use crate::sys::memory::{alloc_pages, dealloc_pages};
use alloc::vec::Vec;
use x86_64::structures::paging::OffsetPageTable;
pub const PAGE: usize = 4096;
#[inline]
pub fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}
#[inline]
pub fn align_dn(x: usize, a: usize) -> usize {
    x & !(a - 1)
}

const USER_LOWER: usize = 0x0000_0000_4000_0000; // pick what fits your layout
const USER_UPPER: usize = 0x0000_7FFF_F000_0000; // below USER_STACK_TOP

pub struct ProcMM {
    /// Start of the heap (page-aligned), typically align_up(max_end_of_loaded_segments).
    pub heap_start: usize,
    /// Current program break (heap end, page-aligned).
    pub brk_cur: usize,
    /// How far we have actually mapped for the heap (>= heap_start, page-aligned).
    pub mapped_heap_end: usize,
    /// Cursor for picking addresses for mmap(NULL,...).
    pub mmap_base_hint: usize,
    pub mmap_regions: Vec<MmapRegion>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MmapKind {
    Owned,
    Shared,
}

#[derive(Clone, Copy, Debug)]
pub struct MmapRegion {
    pub base: usize,
    pub len: usize,
    pub kind: MmapKind,
}

impl ProcMM {
    pub fn new(heap_start: usize) -> Self {
        let heap_start = align_up(heap_start, PAGE);
        ProcMM {
            heap_start,
            brk_cur: heap_start,
            mapped_heap_end: heap_start,
            mmap_base_hint: 0,
            mmap_regions: Vec::new(),
        }
    }

    pub fn set_brk(&mut self, mapper: &mut OffsetPageTable, new_end: usize) -> Result<usize, ()> {
        let new_end = align_up(new_end, PAGE);
        if new_end < self.heap_start {
            // fail but report current break (Linux does this)
            return Ok(self.brk_cur);
        }

        if new_end > self.mapped_heap_end {
            let grow_from = self.mapped_heap_end;
            let grow_len = new_end - grow_from;
            // Map as user, writable. (You can thread executable/NX if needed.)
            alloc_pages(mapper, grow_from as u64, grow_len, true, true)?;
            self.mapped_heap_end = new_end;
        } else if new_end < self.mapped_heap_end {
            // Optional: actually release pages. Safe to ignore errors here.
            let shrink_from = new_end;
            let shrink_len = self.mapped_heap_end - new_end;
            let _ = dealloc_pages(mapper, shrink_from as u64, shrink_len);
            self.mapped_heap_end = new_end;
        }

        self.brk_cur = new_end;
        Ok(self.brk_cur)
    }

    pub fn brk_grow_by(&mut self, mapper: &mut OffsetPageTable, size: usize) -> Result<usize, ()> {
        if size == 0 {
            return Ok(self.brk_cur);
        }
        self.set_brk(mapper, self.brk_cur.saturating_add(size))
    }

    /// Initialize the mmap cursor the first time we need it.
    #[inline]
    pub fn ensure_mmap_base(&mut self) {
        if self.mmap_base_hint == 0 {
            // Start somewhere sane in the user range (below stack, above heap).
            let start = core::cmp::max(self.mapped_heap_end, USER_LOWER);
            self.mmap_base_hint = align_up(start, PAGE);
        }
    }

    /// Reserve a VA range for anonymous mmap with a simple bump cursor.
    /// (You can replace with a proper VMA first-fit later.)
    pub fn reserve_mmap_range(&mut self, length: usize) -> Option<usize> {
        self.ensure_mmap_base();
        let len = align_up(length, PAGE);
        let base = align_up(self.mmap_base_hint, PAGE);
        let end = base.checked_add(len)?;
        if end >= USER_UPPER {
            return None;
        }
        self.mmap_base_hint = end;
        Some(base)
    }

    pub fn track_mmap(&mut self, base: usize, len: usize, kind: MmapKind) {
        let region = MmapRegion { base, len, kind };
        self.mmap_regions.push(region);
    }

    pub fn remove_mmap(&mut self, base: usize, len: usize) -> Option<MmapKind> {
        if let Some(pos) = self
            .mmap_regions
            .iter()
            .position(|r| r.base == base && r.len == len)
        {
            Some(self.mmap_regions.remove(pos).kind)
        } else {
            None
        }
    }

    #[inline]
    pub fn curr_brk(&self) -> usize {
        self.brk_cur
    }
}
