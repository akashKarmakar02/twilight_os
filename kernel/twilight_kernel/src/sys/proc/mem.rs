use crate::sys::fs::vfs::VfsNode;
use crate::sys::fs::memfd::MemFd;
use crate::sys::memory::{alloc_pages, dealloc_pages};
use crate::utils::sync::Mutex;
use alloc::sync::Arc;
use alloc::vec::Vec;
use x86_64::structures::paging::OffsetPageTable;

pub const PAGE: usize = 4096;
pub const PROT_READ: usize = 1;
pub const PROT_WRITE: usize = 2;
pub const PROT_EXEC: usize = 4;

#[inline]
pub fn align_up(x: usize, a: usize) -> usize {
    (x + a - 1) & !(a - 1)
}

#[inline]
pub fn align_dn(x: usize, a: usize) -> usize {
    x & !(a - 1)
}

const USER_MMAP_BASE: usize = 0x0000_4000_0000_0000;
const USER_UPPER: usize = 0x0000_7FFF_F000_0000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VmPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl VmPermissions {
    pub fn from_prot(prot: usize) -> Self {
        Self {
            read: prot & PROT_READ != 0,
            write: prot & PROT_WRITE != 0,
            execute: prot & PROT_EXEC != 0,
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            read: self.read || other.read,
            write: self.write || other.write,
            execute: self.execute || other.execute,
        }
    }

    pub fn allows(self, write: bool, execute: bool) -> bool {
        if execute {
            self.execute
        } else if write {
            self.write
        } else {
            self.read
        }
    }
}

#[derive(Clone)]
pub struct ProcMM {
    pub heap_start: usize,
    pub brk_cur: usize,
    pub mapped_heap_end: usize,
    pub mmap_base_hint: usize,
    pub mmap_regions: Vec<MmapRegion>,
    pub elf_regions: Vec<ElfRegion>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MmapKind {
    Anonymous,
    Owned,
    Shared,
}

#[derive(Clone)]
pub struct MmapRegion {
    pub base: usize,
    pub len: usize,
    pub kind: MmapKind,
    pub permissions: VmPermissions,
    pub memfd: Option<Arc<Mutex<MemFd>>>,
}

#[derive(Clone)]
pub struct ElfRegion {
    pub base: usize,
    pub len: usize,
    pub file_base: usize,
    pub file_offset: usize,
    pub file_end: usize,
    pub permissions: VmPermissions,
    pub file: VfsNode,
}

pub struct ElfPageFragment {
    pub file: VfsNode,
    pub file_offset: usize,
    pub page_offset: usize,
    pub len: usize,
}

pub struct PageFaultPlan {
    pub permissions: VmPermissions,
    pub fragments: Vec<ElfPageFragment>,
}

impl ProcMM {
    pub fn new(heap_start: usize) -> Self {
        let heap_start = align_up(heap_start, PAGE);
        Self {
            heap_start,
            brk_cur: heap_start,
            mapped_heap_end: heap_start,
            mmap_base_hint: 0,
            mmap_regions: Vec::new(),
            elf_regions: Vec::new(),
        }
    }

    pub fn set_brk(&mut self, mapper: &mut OffsetPageTable, new_end: usize) -> Result<usize, ()> {
        let new_end = align_up(new_end, PAGE);
        if new_end < self.heap_start {
            return Ok(self.brk_cur);
        }

        if new_end > self.mapped_heap_end {
            let grow_from = self.mapped_heap_end;
            let grow_len = new_end - grow_from;
            alloc_pages(mapper, grow_from as u64, grow_len, true, true)?;
            self.mapped_heap_end = new_end;
        } else if new_end < self.mapped_heap_end {
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

    #[inline]
    pub fn ensure_mmap_base(&mut self) {
        if self.mmap_base_hint == 0 {
            let start = core::cmp::max(self.mapped_heap_end, USER_MMAP_BASE);
            self.mmap_base_hint = align_up(start, PAGE);
        }
    }

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

    pub fn track_mmap(
        &mut self,
        base: usize,
        len: usize,
        kind: MmapKind,
        permissions: VmPermissions,
    ) {
        self.mmap_regions.push(MmapRegion {
            base,
            len,
            kind,
            permissions,
            memfd: None,
        });
    }

    pub fn track_memfd_mmap(
        &mut self,
        base: usize,
        len: usize,
        permissions: VmPermissions,
        memfd: Arc<Mutex<MemFd>>,
    ) {
        self.mmap_regions.push(MmapRegion {
            base,
            len,
            kind: MmapKind::Shared,
            permissions,
            memfd: Some(memfd),
        });
    }

    pub fn page_fault_plan(&self, page_base: usize) -> Option<PageFaultPlan> {
        if let Some(region) = self.mmap_regions.iter().find(|region| {
            region.kind == MmapKind::Anonymous
                && page_base >= region.base
                && page_base < region.base.saturating_add(region.len)
        }) {
            return Some(PageFaultPlan {
                permissions: region.permissions,
                fragments: Vec::new(),
            });
        }

        let page_end = page_base.checked_add(PAGE)?;
        let mut permissions = VmPermissions::default();
        let mut fragments = Vec::new();
        let mut found = false;

        for region in &self.elf_regions {
            let region_end = region.base.saturating_add(region.len);
            if page_base >= region_end || page_end <= region.base {
                continue;
            }
            found = true;
            permissions = permissions.union(region.permissions);

            let copy_start =
                core::cmp::max(page_base, core::cmp::max(region.base, region.file_base));
            let copy_end = core::cmp::min(page_end, core::cmp::min(region_end, region.file_end));
            if copy_start >= copy_end {
                continue;
            }

            fragments.push(ElfPageFragment {
                file: region.file.clone(),
                file_offset: region
                    .file_offset
                    .checked_add(copy_start.checked_sub(region.file_base)?)?,
                page_offset: copy_start.checked_sub(page_base)?,
                len: copy_end - copy_start,
            });
        }

        found.then_some(PageFaultPlan {
            permissions,
            fragments,
        })
    }

    pub fn permissions_for_page(&self, page_base: usize) -> Option<VmPermissions> {
        if let Some(region) = self.mmap_regions.iter().find(|region| {
            page_base >= region.base && page_base < region.base.saturating_add(region.len)
        }) {
            return Some(region.permissions);
        }

        let page_end = page_base.checked_add(PAGE)?;
        let mut permissions = VmPermissions::default();
        let mut found = false;
        for region in &self.elf_regions {
            let region_end = region.base.saturating_add(region.len);
            if page_base < region_end && page_end > region.base {
                found = true;
                permissions = permissions.union(region.permissions);
            }
        }
        found.then_some(permissions)
    }

    pub fn is_shared_page(&self, page_base: usize) -> bool {
        self.mmap_regions.iter().any(|region| {
            region.kind == MmapKind::Shared
                && page_base >= region.base
                && page_base < region.base.saturating_add(region.len)
        })
    }

    pub fn protect_range(&mut self, base: usize, len: usize, permissions: VmPermissions) {
        self.mmap_regions = protect_mmap_regions(&self.mmap_regions, base, len, permissions);
        self.elf_regions = protect_elf_regions(&self.elf_regions, base, len, permissions);
    }

    pub fn remove_mmap_range(&mut self, base: usize, len: usize) -> Vec<MmapRegion> {
        let Some(end) = base.checked_add(len) else {
            return Vec::new();
        };

        let mut kept = Vec::with_capacity(self.mmap_regions.len());
        let mut removed = Vec::new();
        for region in self.mmap_regions.drain(..) {
            let region_end = region.base.saturating_add(region.len);
            let overlap_start = core::cmp::max(base, region.base);
            let overlap_end = core::cmp::min(end, region_end);

            if overlap_start >= overlap_end {
                kept.push(region);
                continue;
            }

            let mut overlap = region.clone();
            overlap.base = overlap_start;
            overlap.len = overlap_end - overlap_start;
            removed.push(overlap);
            if region.base < overlap_start {
                let mut left = region.clone();
                left.len = overlap_start - region.base;
                kept.push(left);
            }
            if overlap_end < region_end {
                let mut right = region;
                right.base = overlap_end;
                right.len = region_end - overlap_end;
                kept.push(right);
            }
        }
        self.mmap_regions = kept;
        removed
    }

    pub fn remove_elf_range(&mut self, base: usize, len: usize) -> Vec<ElfRegion> {
        let Some(end) = base.checked_add(len) else {
            return Vec::new();
        };

        let mut kept = Vec::with_capacity(self.elf_regions.len());
        let mut removed = Vec::new();
        for region in self.elf_regions.drain(..) {
            let region_end = region.base.saturating_add(region.len);
            let overlap_start = core::cmp::max(base, region.base);
            let overlap_end = core::cmp::min(end, region_end);

            if overlap_start >= overlap_end {
                kept.push(region);
                continue;
            }

            let delta_removed = overlap_start.wrapping_sub(region.base);
            let mut overlap = region.clone();
            overlap.base = overlap_start;
            overlap.len = overlap_end - overlap_start;
            overlap.file_base = overlap.file_base.wrapping_add(delta_removed);
            overlap.file_offset = overlap.file_offset.wrapping_add(delta_removed);
            overlap.file_end = overlap.file_end.wrapping_add(delta_removed);
            removed.push(overlap);

            if region.base < overlap_start {
                let mut left = region.clone();
                left.len = overlap_start - region.base;
                kept.push(left);
            }
            if overlap_end < region_end {
                let delta_right = overlap_end.wrapping_sub(region.base);
                let mut right = region;
                right.base = overlap_end;
                right.len = region_end - overlap_end;
                right.file_base = right.file_base.wrapping_add(delta_right);
                right.file_offset = right.file_offset.wrapping_add(delta_right);
                right.file_end = right.file_end.wrapping_add(delta_right);
                kept.push(right);
            }
        }
        self.elf_regions = kept;
        removed
    }

    #[inline]
    pub fn curr_brk(&self) -> usize {
        self.brk_cur
    }
}

fn protect_mmap_regions(
    regions: &[MmapRegion],
    base: usize,
    len: usize,
    permissions: VmPermissions,
) -> Vec<MmapRegion> {
    let Some(end) = base.checked_add(len) else {
        return regions.to_vec();
    };
    let mut result = Vec::with_capacity(regions.len() + 2);
    for region in regions.iter().cloned() {
        let region_end = region.base.saturating_add(region.len);
        let overlap_start = core::cmp::max(base, region.base);
        let overlap_end = core::cmp::min(end, region_end);
        if overlap_start >= overlap_end {
            result.push(region);
            continue;
        }
        if region.base < overlap_start {
            let mut left = region.clone();
            left.len = overlap_start - region.base;
            result.push(left);
        }
        let mut middle = region.clone();
        middle.base = overlap_start;
        middle.len = overlap_end - overlap_start;
        middle.permissions = permissions;
        result.push(middle);
        if overlap_end < region_end {
            let mut right = region;
            right.base = overlap_end;
            right.len = region_end - overlap_end;
            result.push(right);
        }
    }
    result
}

fn protect_elf_regions(
    regions: &[ElfRegion],
    base: usize,
    len: usize,
    permissions: VmPermissions,
) -> Vec<ElfRegion> {
    let Some(end) = base.checked_add(len) else {
        return regions.to_vec();
    };
    let mut result = Vec::with_capacity(regions.len() + 2);
    for region in regions {
        let region_end = region.base.saturating_add(region.len);
        let overlap_start = core::cmp::max(base, region.base);
        let overlap_end = core::cmp::min(end, region_end);
        if overlap_start >= overlap_end {
            result.push(region.clone());
            continue;
        }
        if region.base < overlap_start {
            let mut left = region.clone();
            left.len = overlap_start - region.base;
            result.push(left);
        }
        let mut overlap = region.clone();
        overlap.base = overlap_start;
        overlap.len = overlap_end - overlap_start;
        overlap.permissions = permissions;
        result.push(overlap);
        if overlap_end < region_end {
            let mut right = region.clone();
            right.base = overlap_end;
            right.len = region_end - overlap_end;
            result.push(right);
        }
    }
    result
}
