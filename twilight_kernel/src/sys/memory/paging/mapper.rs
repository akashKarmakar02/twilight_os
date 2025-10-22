use core::arch::asm;
use core::ops::RangeInclusive;
use core::sync::atomic::AtomicU64;
use super::FRAME_ALLOCATOR;
use super::addr::{PhysAddr, VirtAddr};
use super::page::{AddressNotAligned, Page, PageSize, PhysFrame, Size1GiB, Size2MiB, Size4KiB};
use super::page_table::{FrameError, PageTable, PageTableEntry, PageTableFlags};

pub unsafe trait FrameAllocator<S: PageSize> {
    fn allocate_frame(&self) -> Option<PhysFrame<S>>;
    fn deallocate_frame(&self, frame: PhysFrame<S>);
}

pub trait MapperAllSize: Mapper<Size4KiB> + Mapper<Size2MiB> + Mapper<Size1GiB> {}

impl<T> MapperAllSize for T where T: Mapper<Size4KiB> + Mapper<Size2MiB> + Mapper<Size1GiB> {}

pub trait Translate {
    fn translate(&self, addr: VirtAddr) -> TranslateResult;

    #[inline]
    fn translate_addr(&self, addr: VirtAddr) -> Option<PhysAddr> {
        match self.translate(addr) {
            TranslateResult::NotMapped | TranslateResult::InvalidFrameAddress(_) => None,
            TranslateResult::Mapped { frame, offset, .. } => Some(frame.start_address() + offset),
        }
    }
}

#[derive(Debug)]
pub enum TranslateResult {
    Mapped {
        frame: MappedFrame,
        offset: u64,
        flags: PageTableFlags,
    },
    NotMapped,
    InvalidFrameAddress(PhysAddr),
}

#[derive(Debug)]
pub enum MappedFrame {
    Size4KiB(PhysFrame<Size4KiB>),
    Size2MiB(PhysFrame<Size2MiB>),
    Size1GiB(PhysFrame<Size1GiB>),
}

impl MappedFrame {
    pub const fn start_address(&self) -> PhysAddr {
        match self {
            MappedFrame::Size4KiB(frame) => frame.start_address,
            MappedFrame::Size2MiB(frame) => frame.start_address,
            MappedFrame::Size1GiB(frame) => frame.start_address,
        }
    }

    #[allow(unused)]
    pub const fn size(&self) -> u64 {
        match self {
            MappedFrame::Size4KiB(_) => Size4KiB::SIZE,
            MappedFrame::Size2MiB(_) => Size2MiB::SIZE,
            MappedFrame::Size1GiB(_) => Size1GiB::SIZE,
        }
    }
}

pub trait Mapper<S: PageSize> {
    #[inline]
    unsafe fn map_to(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
    ) -> Result<MapperFlush<S>, MapToError<S>>
    where
        Self: Sized,
    {
        let parent_table_flags = flags
            & (PageTableFlags::PRESENT
                | PageTableFlags::WRITABLE
                | PageTableFlags::USER_ACCESSIBLE);

        unsafe { self.map_to_with_table_flags(page, frame, flags, parent_table_flags) }
    }

    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        parent_table_flags: PageTableFlags,
    ) -> Result<MapperFlush<S>, MapToError<S>>
    where
        Self: Sized;

    fn unmap(&mut self, page: Page<S>) -> Result<MapperFlush<S>, UnmapError>;

    unsafe fn update_flags(
        &mut self,
        page: Page<S>,
        flags: PageTableFlags,
    ) -> Result<MapperFlush<S>, FlagUpdateError>;

    fn translate_page(&self, page: Page<S>) -> Result<PhysFrame<S>, TranslateError>;

    #[inline]
    unsafe fn identity_map(
        &mut self,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
    ) -> Result<MapperFlush<S>, MapToError<S>>
    where
        Self: Sized,
        S: PageSize,
        Self: Mapper<S>,
    {
        let page = Page::containing_address(VirtAddr::new(frame.start_address().as_u64()));
        unsafe { self.map_to(page, frame, flags) }
    }
}

#[derive(Debug)]
#[must_use = "Page Table changes must be flushed or ignored."]
pub struct MapperFlush<S: PageSize>(Page<S>);

impl<S: PageSize> MapperFlush<S> {
    #[inline]
    fn new(page: Page<S>) -> Self {
        MapperFlush(page)
    }

    pub fn ignore(self) {}

    #[inline]
    pub fn flush(self) {
        let raw = self.0.start_address().as_u64();

        unsafe {
            asm!("invlpg [{}]", in(reg) raw, options(nostack));
        }
    }
}

#[derive(Debug)]
pub enum MapToError<S: PageSize> {
    FrameAllocationFailed,
    ParentEntryHugePage,
    PageAlreadyMapped(PhysFrame<S>),
}

#[derive(Debug)]
pub enum UnmapError {
    ParentEntryHugePage,
    PageNotMapped,
    InvalidFrameAddress(PhysAddr),
}

#[derive(Debug)]
pub enum FlagUpdateError {
    PageNotMapped,
    ParentEntryHugePage,
}

#[derive(Debug)]
pub enum TranslateError {
    PageNotMapped,
    ParentEntryHugePage,
    InvalidFrameAddress(PhysAddr),
}

pub trait FrameDeallocator<S: PageSize> {
    unsafe fn deallocate_frame(&mut self, frame: PhysFrame<S>);
}

#[derive(Debug)]
pub struct MappedPageTable<'a, P: PageTableFrameMapping> {
    page_table_walker: PageTableWalker<P>,
    level_5_paging_enabled: bool,
    page_table: &'a mut PageTable,
}

impl<'a, P: PageTableFrameMapping> MappedPageTable<'a, P> {
    #[inline]
    pub fn new(page_table: &'a mut PageTable, page_table_frame_mapping: P) -> Self {
        Self {
            page_table,
            level_5_paging_enabled: super::level_5_paging_enabled(),
            page_table_walker: unsafe { PageTableWalker::new(page_table_frame_mapping) },
        }
    }

    fn map_to_2mib(
        &mut self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        flags: PageTableFlags,
        parent_table_flags: PageTableFlags,
    ) -> Result<MapperFlush<Size2MiB>, MapToError<Size2MiB>> {
        let mut is_alloc_4 = false;

        let p4;

        if self.level_5_paging_enabled {
            let p5 = &mut self.page_table;
            let (alloc, yes) = self
                .page_table_walker
                .create_next_table(&mut p5[page.p5_index()], parent_table_flags)?;

            p4 = yes;
            is_alloc_4 = alloc;
        } else {
            p4 = &mut self.page_table;
        }

        let (is_alloc_3, p3) = self
            .page_table_walker
            .create_next_table(&mut p4[page.p4_index()], parent_table_flags)?;

        let (is_alloc_2, p2) = self
            .page_table_walker
            .create_next_table(&mut p3[page.p3_index()], parent_table_flags)?;

        if !p2[page.p2_index()].is_unused() {
            return Err(MapToError::PageAlreadyMapped(frame));
        }

        p2[page.p2_index()].set_addr(frame.start_address(), flags | PageTableFlags::HUGE_PAGE);

        if is_alloc_2 {
            p3[page.p3_index()].inc_entry_count();
        }

        if is_alloc_3 {
            p4[page.p4_index()].inc_entry_count();
        }

        if is_alloc_4 {
            let p5 = &mut self.page_table;
            p5[page.p5_index()].inc_entry_count();
        }

        Ok(MapperFlush::new(page))
    }

    fn map_to_4kib(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
        parent_table_flags: PageTableFlags,
    ) -> Result<MapperFlush<Size4KiB>, MapToError<Size4KiB>> {
        let p4;

        let mut is_alloc_4 = false;

        if self.level_5_paging_enabled {
            let p5 = &mut self.page_table;
            let (alloc, yes) = self
                .page_table_walker
                .create_next_table(&mut p5[page.p5_index()], parent_table_flags)?;

            p4 = yes;
            is_alloc_4 = alloc;
        } else {
            p4 = &mut self.page_table;
        }

        let (is_alloc_3, p3) = self
            .page_table_walker
            .create_next_table(&mut p4[page.p4_index()], parent_table_flags)?;

        let (is_alloc_2, p2) = self
            .page_table_walker
            .create_next_table(&mut p3[page.p3_index()], parent_table_flags)?;

        let (is_alloc_1, p1) = self
            .page_table_walker
            .create_next_table(&mut p2[page.p2_index()], parent_table_flags)?;

        if !p1[page.p1_index()].is_unused() {
            return Err(MapToError::PageAlreadyMapped(frame));
        }

        p1[page.p1_index()].set_frame(frame, flags);

        if is_alloc_1 {
            p2[page.p2_index()].inc_entry_count();
        }

        if is_alloc_2 {
            p3[page.p3_index()].inc_entry_count();
        }

        if is_alloc_3 {
            p4[page.p4_index()].inc_entry_count();
        }

        if is_alloc_4 {
            let p5 = &mut self.page_table;
            p5[page.p5_index()].inc_entry_count();
        }

        Ok(MapperFlush::new(page))
    }
}

impl<'a, P: PageTableFrameMapping> Mapper<Size2MiB> for MappedPageTable<'a, P> {
    #[inline]
    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        flags: PageTableFlags,
        parent_table_flags: PageTableFlags,
    ) -> Result<MapperFlush<Size2MiB>, MapToError<Size2MiB>> {
        self.map_to_2mib(page, frame, flags, parent_table_flags)
    }

    fn unmap(&mut self, page: Page<Size2MiB>) -> Result<MapperFlush<Size2MiB>, UnmapError> {
        let p4 = if self.level_5_paging_enabled {
            let p5 = &mut self.page_table;

            self.page_table_walker
                .next_table_mut(&mut p5[page.p5_index()])?
        } else {
            &mut self.page_table
        };

        let p3 = self
            .page_table_walker
            .next_table_mut(&mut p4[page.p4_index()])?;
        let p2 = self
            .page_table_walker
            .next_table_mut(&mut p3[page.p3_index()])?;

        let p2_entry = &mut p2[page.p2_index()];
        let flags = p2_entry.flags();

        if !flags.contains(PageTableFlags::PRESENT) {
            return Err(UnmapError::PageNotMapped);
        }
        if !flags.contains(PageTableFlags::HUGE_PAGE) {
            return Err(UnmapError::ParentEntryHugePage);
        }

        let _frame: PhysFrame<Size4KiB> = PhysFrame::from_start_address(p2_entry.addr())
            .map_err(|AddressNotAligned| UnmapError::InvalidFrameAddress(p2_entry.addr()))?;

        p2_entry.unref_vm_frame();
        p2_entry.set_unused();

        Ok(MapperFlush::new(page))
    }

    unsafe fn update_flags(
        &mut self,
        page: Page<Size2MiB>,
        flags: PageTableFlags,
    ) -> Result<MapperFlush<Size2MiB>, FlagUpdateError> {
        let p4 = if self.level_5_paging_enabled {
            let p5 = &mut self.page_table;

            self.page_table_walker
                .next_table_mut(&mut p5[page.p5_index()])?
        } else {
            &mut self.page_table
        };

        let p3 = self
            .page_table_walker
            .next_table_mut(&mut p4[page.p4_index()])?;
        let p2 = self
            .page_table_walker
            .next_table_mut(&mut p3[page.p3_index()])?;

        if p2[page.p2_index()].is_unused() {
            return Err(FlagUpdateError::PageNotMapped);
        }

        p2[page.p2_index()].set_flags(flags | PageTableFlags::HUGE_PAGE);

        Ok(MapperFlush::new(page))
    }

    fn translate_page(&self, page: Page<Size2MiB>) -> Result<PhysFrame<Size2MiB>, TranslateError> {
        let p4;

        if self.level_5_paging_enabled {
            let p5 = &self.page_table;

            p4 = self.page_table_walker.next_table(&p5[page.p5_index()])?;
        } else {
            p4 = self.page_table;
        }

        let p3 = self.page_table_walker.next_table(&p4[page.p4_index()])?;
        let p2 = self.page_table_walker.next_table(&p3[page.p3_index()])?;

        let p2_entry = &p2[page.p2_index()];

        if p2_entry.is_unused() {
            return Err(TranslateError::PageNotMapped);
        }

        PhysFrame::from_start_address(p2_entry.addr())
            .map_err(|_address_not_aligned| TranslateError::InvalidFrameAddress(p2_entry.addr()))
    }
}

impl<'a, P: PageTableFrameMapping> Mapper<Size4KiB> for MappedPageTable<'a, P> {
    #[inline]
    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
        parent_table_flags: PageTableFlags,
    ) -> Result<MapperFlush<Size4KiB>, MapToError<Size4KiB>> {
        self.map_to_4kib(page, frame, flags, parent_table_flags)
    }

    fn unmap(&mut self, page: Page<Size4KiB>) -> Result<MapperFlush<Size4KiB>, UnmapError> {
        let p4;

        if self.level_5_paging_enabled {
            let p5 = &mut self.page_table;

            p4 = self
                .page_table_walker
                .next_table_mut(&mut p5[page.p5_index()])?;
        } else {
            p4 = &mut self.page_table;
        }

        let p3 = self
            .page_table_walker
            .next_table_mut(&mut p4[page.p4_index()])?;
        let p2 = self
            .page_table_walker
            .next_table_mut(&mut p3[page.p3_index()])?;
        let p1 = self
            .page_table_walker
            .next_table_mut(&mut p2[page.p2_index()])?;

        let p1_entry = &mut p1[page.p1_index()];

        let _frame = p1_entry.frame().map_err(|err| match err {
            FrameError::FrameNotPresent => UnmapError::PageNotMapped,
            FrameError::HugeFrame => UnmapError::ParentEntryHugePage,
        })?;

        p1_entry.unref_vm_frame();
        p1_entry.set_unused();

        Ok(MapperFlush::new(page))
    }

    unsafe fn update_flags(
        &mut self,
        page: Page<Size4KiB>,
        flags: PageTableFlags,
    ) -> Result<MapperFlush<Size4KiB>, FlagUpdateError> {
        let p4 = if self.level_5_paging_enabled {
            let p5 = &mut self.page_table;

            self.page_table_walker
                .next_table_mut(&mut p5[page.p5_index()])?
        } else {
            &mut self.page_table
        };

        let p3 = self
            .page_table_walker
            .next_table_mut(&mut p4[page.p4_index()])?;
        let p2 = self
            .page_table_walker
            .next_table_mut(&mut p3[page.p3_index()])?;
        let p1 = self
            .page_table_walker
            .next_table_mut(&mut p2[page.p2_index()])?;

        if p1[page.p1_index()].is_unused() {
            return Err(FlagUpdateError::PageNotMapped);
        }

        p1[page.p1_index()].set_flags(flags);

        Ok(MapperFlush::new(page))
    }

    fn translate_page(&self, page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, TranslateError> {
        let p4;

        if self.level_5_paging_enabled {
            let p5 = &self.page_table;

            p4 = self.page_table_walker.next_table(&p5[page.p5_index()])?;
        } else {
            p4 = self.page_table;
        }

        let p3 = self.page_table_walker.next_table(&p4[page.p4_index()])?;
        let p2 = self.page_table_walker.next_table(&p3[page.p3_index()])?;
        let p1 = self.page_table_walker.next_table(&p2[page.p2_index()])?;

        let p1_entry = &p1[page.p1_index()];

        if p1_entry.is_unused() {
            return Err(TranslateError::PageNotMapped);
        }

        PhysFrame::from_start_address(p1_entry.addr())
            .map_err(|AddressNotAligned| TranslateError::InvalidFrameAddress(p1_entry.addr()))
    }
}

impl<'a, P: PageTableFrameMapping> Translate for MappedPageTable<'a, P> {
    #[allow(clippy::inconsistent_digit_grouping)]
    fn translate(&self, addr: VirtAddr) -> TranslateResult {
        let p4;

        if self.level_5_paging_enabled {
            let p5 = &self.page_table;

            p4 = match self.page_table_walker.next_table(&p5[addr.p5_index()]) {
                Ok(page_table) => page_table,
                Err(PageTableWalkError::NotMapped) => return TranslateResult::NotMapped,
                Err(PageTableWalkError::MappedToHugePage) => {
                    panic!("level 4 entry has huge page bit set")
                }
            };
        } else {
            p4 = self.page_table;
        }

        let p3 = match self.page_table_walker.next_table(&p4[addr.p4_index()]) {
            Ok(page_table) => page_table,
            Err(PageTableWalkError::NotMapped) => return TranslateResult::NotMapped,
            Err(PageTableWalkError::MappedToHugePage) => {
                panic!("level 4 entry has huge page bit set")
            }
        };
        let p2 = match self.page_table_walker.next_table(&p3[addr.p3_index()]) {
            Ok(page_table) => page_table,
            Err(PageTableWalkError::NotMapped) => return TranslateResult::NotMapped,
            Err(PageTableWalkError::MappedToHugePage) => {
                let entry = &p3[addr.p3_index()];
                let frame = PhysFrame::containing_address(entry.addr());
                let offset = addr.as_u64() & 0o7_777_777_777;
                let flags = entry.flags();
                return TranslateResult::Mapped {
                    frame: MappedFrame::Size1GiB(frame),
                    offset,
                    flags,
                };
            }
        };
        let p1 = match self.page_table_walker.next_table(&p2[addr.p2_index()]) {
            Ok(page_table) => page_table,
            Err(PageTableWalkError::NotMapped) => return TranslateResult::NotMapped,
            Err(PageTableWalkError::MappedToHugePage) => {
                let entry = &p2[addr.p2_index()];
                let frame = PhysFrame::containing_address(entry.addr());
                let offset = addr.as_u64() & 0o7_777_777;
                let flags = entry.flags();
                return TranslateResult::Mapped {
                    frame: MappedFrame::Size2MiB(frame),
                    offset,
                    flags,
                };
            }
        };

        let p1_entry = &p1[addr.p1_index()];

        if p1_entry.is_unused() {
            return TranslateResult::NotMapped;
        }

        let frame = match PhysFrame::from_start_address(p1_entry.addr()) {
            Ok(frame) => frame,
            Err(AddressNotAligned) => return TranslateResult::InvalidFrameAddress(p1_entry.addr()),
        };
        let offset = u64::from(addr.page_offset());
        let flags = p1_entry.flags();
        TranslateResult::Mapped {
            frame: MappedFrame::Size4KiB(frame),
            offset,
            flags,
        }
    }
}

#[derive(Debug)]
struct PageTableWalker<P: PageTableFrameMapping> {
    page_table_frame_mapper: P,
}

impl<P: PageTableFrameMapping> PageTableWalker<P> {
    #[inline]
    pub unsafe fn new(page_table_frame_mapping: P) -> Self {
        Self {
            page_table_frame_mapper: page_table_frame_mapping,
        }
    }

    #[inline]
    fn next_table<'b>(
        &self,
        entry: &'b PageTableEntry,
    ) -> Result<&'b PageTable, PageTableWalkError> {
        let page_table_ptr = self
            .page_table_frame_mapper
            .frame_to_pointer(entry.frame()?);
        let page_table: &PageTable = unsafe { &*page_table_ptr };

        Ok(page_table)
    }

    #[inline]
    fn next_table_mut<'b>(
        &self,
        entry: &'b mut PageTableEntry,
    ) -> Result<&'b mut PageTable, PageTableWalkError> {
        let page_table_ptr = self
            .page_table_frame_mapper
            .frame_to_pointer(entry.frame()?);
        let page_table: &mut PageTable = unsafe { &mut *page_table_ptr };

        Ok(page_table)
    }

    fn create_next_table<'b>(
        &self,
        entry: &'b mut PageTableEntry,
        insert_flags: PageTableFlags,
    ) -> Result<(bool, &'b mut PageTable), PageTableCreateError> {
        let created;

        if entry.is_unused() {
            if let Some(frame) = FRAME_ALLOCATOR.allocate_frame() {
                entry.set_frame(frame, insert_flags);
                created = true;
            } else {
                return Err(PageTableCreateError::FrameAllocationFailed);
            }
        } else {
            if !insert_flags.is_empty() && !entry.flags().contains(insert_flags) {
                entry.set_flags(entry.flags() | insert_flags);
            }
            created = false;
        }

        let page_table = match self.next_table_mut(entry) {
            Err(PageTableWalkError::MappedToHugePage) => {
                return Err(PageTableCreateError::MappedToHugePage);
            }
            Err(PageTableWalkError::NotMapped) => {
                unreachable!("entry should be mapped at this point")
            }
            Ok(page_table) => page_table,
        };

        if created {
            page_table.zero();
        }

        Ok((created, page_table))
    }
}

#[derive(Debug)]
enum PageTableWalkError {
    NotMapped,
    MappedToHugePage,
}

#[derive(Debug)]
enum PageTableCreateError {
    MappedToHugePage,
    FrameAllocationFailed,
}

impl From<PageTableCreateError> for MapToError<Size4KiB> {
    #[inline]
    fn from(err: PageTableCreateError) -> Self {
        match err {
            PageTableCreateError::MappedToHugePage => MapToError::ParentEntryHugePage,
            PageTableCreateError::FrameAllocationFailed => MapToError::FrameAllocationFailed,
        }
    }
}

impl From<PageTableCreateError> for MapToError<Size2MiB> {
    #[inline]
    fn from(err: PageTableCreateError) -> Self {
        match err {
            PageTableCreateError::MappedToHugePage => MapToError::ParentEntryHugePage,
            PageTableCreateError::FrameAllocationFailed => MapToError::FrameAllocationFailed,
        }
    }
}

impl From<PageTableCreateError> for MapToError<Size1GiB> {
    #[inline]
    fn from(err: PageTableCreateError) -> Self {
        match err {
            PageTableCreateError::MappedToHugePage => MapToError::ParentEntryHugePage,
            PageTableCreateError::FrameAllocationFailed => MapToError::FrameAllocationFailed,
        }
    }
}

impl From<FrameError> for PageTableWalkError {
    #[inline]
    fn from(err: FrameError) -> Self {
        match err {
            FrameError::HugeFrame => PageTableWalkError::MappedToHugePage,
            FrameError::FrameNotPresent => PageTableWalkError::NotMapped,
        }
    }
}

impl From<PageTableWalkError> for UnmapError {
    #[inline]
    fn from(err: PageTableWalkError) -> Self {
        match err {
            PageTableWalkError::MappedToHugePage => UnmapError::ParentEntryHugePage,
            PageTableWalkError::NotMapped => UnmapError::PageNotMapped,
        }
    }
}

impl From<PageTableWalkError> for FlagUpdateError {
    #[inline]
    fn from(err: PageTableWalkError) -> Self {
        match err {
            PageTableWalkError::MappedToHugePage => FlagUpdateError::ParentEntryHugePage,
            PageTableWalkError::NotMapped => FlagUpdateError::PageNotMapped,
        }
    }
}

impl From<PageTableWalkError> for TranslateError {
    #[inline]
    fn from(err: PageTableWalkError) -> Self {
        match err {
            PageTableWalkError::MappedToHugePage => TranslateError::ParentEntryHugePage,
            PageTableWalkError::NotMapped => TranslateError::PageNotMapped,
        }
    }
}

pub unsafe trait PageTableFrameMapping {
    fn frame_to_pointer(&self, frame: PhysFrame) -> *mut PageTable;
}

#[derive(Debug)]
pub struct OffsetPageTable<'a> {
    inner: MappedPageTable<'a, PhysOffset>,
}

impl<'a> OffsetPageTable<'a> {
    #[inline]
    pub unsafe fn new(page_table: &'a mut PageTable, phys_offset: VirtAddr) -> Self {
        let phys_offset = PhysOffset {
            offset: phys_offset,
        };
        Self {
            inner: MappedPageTable::new(page_table, phys_offset),
        }
    }

    pub fn page_table(&mut self) -> &mut PageTable {
        self.inner.page_table
    }
}

#[derive(Debug)]
struct PhysOffset {
    offset: VirtAddr,
}

unsafe impl PageTableFrameMapping for PhysOffset {
    fn frame_to_pointer(&self, frame: PhysFrame) -> *mut PageTable {
        let virt = self.offset + frame.start_address().as_u64();
        virt.as_mut_ptr()
    }
}

impl<'a> Mapper<Size2MiB> for OffsetPageTable<'a> {
    #[inline]
    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        flags: PageTableFlags,
        parent_table_flags: PageTableFlags,
    ) -> Result<MapperFlush<Size2MiB>, MapToError<Size2MiB>> {
        unsafe {
            self.inner
                .map_to_with_table_flags(page, frame, flags, parent_table_flags)
        }
    }

    #[inline]
    fn unmap(
        &mut self,
        page: Page<Size2MiB>,
    ) -> Result<
        MapperFlush<x86_64::structures::paging::Size2MiB>,
        crate::memory::paging::mapper::UnmapError,
    > {
        self.inner.unmap(page)
    }

    #[inline]
    unsafe fn update_flags(
        &mut self,
        page: Page<Size2MiB>,
        flags: PageTableFlags,
    ) -> Result<MapperFlush<Size2MiB>, FlagUpdateError> {
        unsafe { self.inner.update_flags(page, flags) }
    }

    #[inline]
    fn translate_page(&self, page: Page<Size2MiB>) -> Result<PhysFrame<Size2MiB>, TranslateError> {
        self.inner.translate_page(page)
    }
}

impl<'a> Mapper<Size4KiB> for OffsetPageTable<'a> {
    #[inline]
    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
        parent_table_flags: PageTableFlags,
    ) -> Result<MapperFlush<Size4KiB>, MapToError<Size4KiB>> {
        unsafe {
            self.inner
                .map_to_with_table_flags(page, frame, flags, parent_table_flags)
        }
    }

    #[inline]
    fn unmap(&mut self, page: Page<Size4KiB>) -> Result<MapperFlush<Size4KiB>, UnmapError> {
        self.inner.unmap(page)
    }

    #[inline]
    unsafe fn update_flags(
        &mut self,
        page: Page<Size4KiB>,
        flags: PageTableFlags,
    ) -> Result<MapperFlush<Size4KiB>, FlagUpdateError> {
        unsafe { self.inner.update_flags(page, flags) }
    }

    #[inline]
    fn translate_page(&self, page: Page<Size4KiB>) -> Result<PhysFrame<Size4KiB>, TranslateError> {
        self.inner.translate_page(page)
    }
}

impl<'a> Translate for OffsetPageTable<'a> {
    #[inline]
    fn translate(&self, addr: VirtAddr) -> TranslateResult {
        self.inner.translate(addr)
    }
}

impl<'a> OffsetPageTable<'a> {
    pub fn copy_page_range(&mut self, src: &mut OffsetPageTable, range: RangeInclusive<VirtAddr>) {
        let mut map_to = |src: &mut OffsetPageTable, addr, frame, flags| match frame {
            MappedFrame::Size4KiB(frame) => {
                let page = Page::<Size4KiB>::containing_address(addr);

                unsafe {
                    self.map_to_with_table_flags(
                        page,
                        frame,
                        flags,
                        PageTableFlags::PRESENT
                            | PageTableFlags::USER_ACCESSIBLE
                            | PageTableFlags::WRITABLE,
                    )
                }
                .unwrap()
                // operating on an inactive page table
                .ignore();

                unsafe { src.update_flags(page, flags) }
                    .unwrap()
                    // caller is required to invalidate the TLB
                    .ignore();
            }
            _ => todo!(),
        };

        let mut addr = *range.start();

        while addr != *range.end() {
            match src.translate(addr) {
                TranslateResult::Mapped {
                    frame,
                    offset,
                    flags,
                } => {
                    assert_eq!(offset, 0, "unaligned page range");
                    map_to(src, addr, frame, flags & !PageTableFlags::WRITABLE);
                }

                TranslateResult::NotMapped => {}
                TranslateResult::InvalidFrameAddress(addr) => {
                    panic!("invalid frame address {:#x}", addr);
                }
            }

            addr += Size4KiB::SIZE;
        }
    }
}
