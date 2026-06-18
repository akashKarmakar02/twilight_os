use core::ptr;

use super::driver::{EINVAL, ENOMEM};
use crate::log;
use crate::sys::memory::bitmap::with_frame_allocator;
use crate::sys::memory::{PAGE_SIZE, phys_to_virt, virt_to_phys};
use x86_64::structures::paging::{FrameDeallocator, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

pub const BUS_SPACE_MAXADDR: u64 = u64::MAX;
pub const BUS_SPACE_MAXADDR_32BIT: u64 = u32::MAX as u64;

pub const BUS_DMA_NOWAIT: u32 = 0x0001;
pub const BUS_DMA_WAITOK: u32 = 0x0002;
pub const BUS_DMA_ZERO: u32 = 0x0004;
pub const BUS_DMA_COHERENT: u32 = 0x0008;

pub const BUS_DMASYNC_PREREAD: u32 = 0x0001;
pub const BUS_DMASYNC_POSTREAD: u32 = 0x0002;
pub const BUS_DMASYNC_PREWRITE: u32 = 0x0004;
pub const BUS_DMASYNC_POSTWRITE: u32 = 0x0008;

#[derive(Clone, Copy, Debug)]
pub struct BusDmaTag {
    pub alignment: usize,
    pub boundary: u64,
    pub lowaddr: u64,
    pub highaddr: u64,
    pub maxsize: usize,
    pub nsegments: usize,
    pub maxsegsz: usize,
    pub flags: u32,
}

#[derive(Debug)]
pub struct BusDmaMap {
    pub vaddr: usize,
    pub paddr: u64,
    pub size: usize,
    pub loaded: bool,
    owned_pages: usize,
    owns_memory: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct BusDmaSegment {
    pub ds_addr: u64,
    pub ds_len: usize,
}

pub type BusDmaCallback = fn(callback_arg: usize, segs: &[BusDmaSegment], error: i32);

pub fn bus_dma_tag_create(
    alignment: usize,
    boundary: u64,
    lowaddr: u64,
    highaddr: u64,
    maxsize: usize,
    nsegments: usize,
    maxsegsz: usize,
    flags: u32,
) -> Result<BusDmaTag, i32> {
    if alignment == 0 || maxsize == 0 || nsegments == 0 || maxsegsz == 0 {
        return Err(EINVAL);
    }
    if nsegments > 1 || maxsegsz > maxsize || alignment > PAGE_SIZE {
        return Err(EINVAL);
    }

    let tag = BusDmaTag {
        alignment,
        boundary,
        lowaddr,
        highaddr,
        maxsize,
        nsegments,
        maxsegsz,
        flags,
    };

    log!(
        "freebsd_kpi: dma tag created alignment={} lowaddr={:#x} maxsize={} nsegments={}",
        alignment,
        lowaddr,
        maxsize,
        nsegments
    );
    Ok(tag)
}

pub fn bus_dma_tag_destroy(tag: BusDmaTag) -> i32 {
    log!(
        "freebsd_kpi: dma tag destroyed maxsize={} nsegments={}",
        tag.maxsize,
        tag.nsegments
    );
    0
}

pub fn bus_dmamap_create(tag: &BusDmaTag) -> Result<BusDmaMap, i32> {
    validate_tag(tag)?;
    Ok(BusDmaMap::empty())
}

pub fn bus_dmamap_destroy(_tag: &BusDmaTag, map: BusDmaMap) -> i32 {
    if map.owns_memory {
        return EINVAL;
    }
    log!("freebsd_kpi: dma map destroyed");
    0
}

pub fn bus_dmamem_alloc(tag: &BusDmaTag, flags: u32) -> Result<(usize, BusDmaMap), i32> {
    validate_tag(tag)?;

    let pages = pages_for(tag.maxsize).ok_or(EINVAL)?;
    let alloc_size = pages * PAGE_SIZE;
    let frame = with_frame_allocator(|frame_allocator| {
        if tag.lowaddr == BUS_SPACE_MAXADDR {
            frame_allocator.allocate_contiguous(pages)
        } else {
            frame_allocator.allocate_contiguous_below(pages, tag.lowaddr)
        }
    })
    .ok_or(ENOMEM)?;

    let paddr = frame.start_address().as_u64();
    if tag.alignment != 0 && paddr % tag.alignment as u64 != 0 {
        deallocate_phys_range(paddr, pages);
        return Err(EINVAL);
    }

    let vaddr = phys_to_virt(frame.start_address()).as_u64() as usize;
    if (flags & BUS_DMA_ZERO) != 0 {
        // SAFETY: the frames were just allocated exclusively to this DMA map
        // and the physical-memory mapping covers the whole contiguous range.
        unsafe {
            ptr::write_bytes(vaddr as *mut u8, 0, alloc_size);
        }
    }

    let map = BusDmaMap {
        vaddr,
        paddr,
        size: tag.maxsize,
        loaded: false,
        owned_pages: pages,
        owns_memory: true,
    };

    log!(
        "freebsd_kpi: dma memory allocated vaddr={:#x} paddr={:#x} size={}",
        vaddr,
        paddr,
        tag.maxsize
    );
    Ok((vaddr, map))
}

pub fn bus_dmamem_free(_tag: &BusDmaTag, vaddr: usize, map: BusDmaMap) -> i32 {
    if !map.owns_memory || map.vaddr != vaddr || map.owned_pages == 0 {
        return EINVAL;
    }

    deallocate_phys_range(map.paddr, map.owned_pages);
    log!(
        "freebsd_kpi: dma memory freed vaddr={:#x} paddr={:#x} size={}",
        map.vaddr,
        map.paddr,
        map.size
    );
    0
}

pub fn bus_dmamap_load(
    tag: &BusDmaTag,
    map: &mut BusDmaMap,
    vaddr: usize,
    buflen: usize,
    callback: BusDmaCallback,
    callback_arg: usize,
) -> i32 {
    if validate_tag(tag).is_err() || buflen == 0 || buflen > tag.maxsize || buflen > tag.maxsegsz {
        return EINVAL;
    }

    let Some(paddr) = map_paddr(map, vaddr, buflen) else {
        return EINVAL;
    };

    map.vaddr = vaddr;
    map.paddr = paddr;
    map.size = buflen;
    map.loaded = true;

    let segment = [BusDmaSegment {
        ds_addr: paddr,
        ds_len: buflen,
    }];
    callback(callback_arg, &segment, 0);
    log!(
        "freebsd_kpi: dma map loaded vaddr={:#x} paddr={:#x} size={}",
        vaddr,
        paddr,
        buflen
    );
    0
}

pub fn bus_dmamap_unload(_tag: &BusDmaTag, map: &mut BusDmaMap) {
    log!(
        "freebsd_kpi: dma map unloaded paddr={:#x} size={}",
        map.paddr,
        map.size
    );
    map.loaded = false;
}

/// No-op for now: Twilight's first FreeBSD KPI DMA layer assumes coherent
/// x86_64 DMA and does not implement cache maintenance, IOMMU, or bounce pages.
pub fn bus_dmamap_sync(_tag: &BusDmaTag, map: &BusDmaMap, op: u32) {
    log!(
        "freebsd_kpi: dma sync no-op paddr={:#x} size={} op={:#x}",
        map.paddr,
        map.size,
        op
    );
}

impl BusDmaMap {
    fn empty() -> Self {
        Self {
            vaddr: 0,
            paddr: 0,
            size: 0,
            loaded: false,
            owned_pages: 0,
            owns_memory: false,
        }
    }
}

fn validate_tag(tag: &BusDmaTag) -> Result<(), i32> {
    if tag.alignment == 0
        || tag.maxsize == 0
        || tag.nsegments != 1
        || tag.maxsegsz == 0
        || tag.maxsegsz > tag.maxsize
    {
        return Err(EINVAL);
    }
    Ok(())
}

fn pages_for(size: usize) -> Option<usize> {
    size.checked_add(PAGE_SIZE - 1).map(|size| size / PAGE_SIZE)
}

fn map_paddr(map: &BusDmaMap, vaddr: usize, buflen: usize) -> Option<u64> {
    if map.owns_memory && map.vaddr == vaddr && buflen <= map.size {
        return Some(map.paddr);
    }

    let page_offset = vaddr & (PAGE_SIZE - 1);
    if page_offset.checked_add(buflen)? > PAGE_SIZE {
        return None;
    }

    virt_to_phys(VirtAddr::new(vaddr as u64)).map(|phys| phys.as_u64())
}

fn deallocate_phys_range(paddr: u64, pages: usize) {
    with_frame_allocator(|frame_allocator| {
        for page in 0..pages {
            let addr = PhysAddr::new(paddr + (page as u64) * PAGE_SIZE as u64);
            let frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(addr);
            // SAFETY: callers pass the exact frames previously allocated for
            // this DMA map, and this helper consumes that ownership on free.
            unsafe {
                frame_allocator.deallocate_frame(frame);
            }
        }
    });
}
