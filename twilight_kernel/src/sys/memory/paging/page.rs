use super::addr::{PhysAddr, VirtAddr};
use super::page_table::PageTableIndex;

use core::fmt;
use core::fmt::Write;
use core::marker::PhantomData;
use core::ops::{Add, AddAssign, Sub, SubAssign};

pub(crate) use x86_64::structures::paging::{
    PageSize,
    Size4KiB,
    Size2MiB,
    Size1GiB,
    PageTableIndex as X86PageTableIndex,
};

pub trait NotGiantPageSize: PageSize {}
impl NotGiantPageSize for Size4KiB {}
impl NotGiantPageSize for Size2MiB {}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub struct Page<S: PageSize = Size4KiB> {
    start_address: VirtAddr,
    size: PhantomData<S>,
}

impl<S: PageSize> Page<S> {
    #[inline]
    pub fn containing_address(address: VirtAddr) -> Self {
        Page {
            start_address: address.align_down(S::SIZE),
            size: PhantomData,
        }
    }

    #[inline]
    pub fn start_address(&self) -> VirtAddr {
        self.start_address
    }

    #[inline]
    pub fn p5_index(self) -> x86_64::structures::paging::PageTableIndex {
        self.start_address.p5_index()
    }

    #[inline]
    pub fn p4_index(self) -> x86_64::structures::paging::PageTableIndex {
        self.start_address().p4_index()
    }

    #[inline]
    pub fn p3_index(self) -> x86_64::structures::paging::PageTableIndex {
        self.start_address().p3_index()
    }

    /// Returns a range of pages, exclusive `end`.
    #[inline]
    pub const fn range(start: Self, end: Self) -> PageRange<S> {
        PageRange { start, end }
    }
}

impl<S: NotGiantPageSize> Page<S> {
    #[inline]
    pub fn p2_index(self) -> x86_64::structures::paging::PageTableIndex {
        self.start_address.p2_index()
    }
}

impl Page<Size4KiB> {
    #[inline]
    pub const fn p1_index(self) -> x86_64::structures::paging::PageTableIndex {
        self.start_address.p1_index()
    }
}

impl<S: PageSize> fmt::Debug for Page<S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_fmt(format_args!(
            "Page[{}]({:#x})",
            S::DEBUG_STR,
            self.start_address().as_u64()
        ))
    }
}

impl<S: PageSize> Add<u64> for Page<S> {
    type Output = Self;

    #[inline]
    fn add(self, rhs: u64) -> Self::Output {
        Page::containing_address(self.start_address() + rhs * S::SIZE)
    }
}

impl<S: PageSize> AddAssign<u64> for Page<S> {
    #[inline]
    fn add_assign(&mut self, rhs: u64) {
        *self = *self + rhs;
    }
}

impl<S: PageSize> Sub<u64> for Page<S> {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: u64) -> Self::Output {
        Page::containing_address(self.start_address() - rhs * S::SIZE)
    }
}

impl<S: PageSize> SubAssign<u64> for Page<S> {
    #[inline]
    fn sub_assign(&mut self, rhs: u64) {
        *self = *self - rhs;
    }
}

impl<S: PageSize> Sub<Self> for Page<S> {
    type Output = u64;

    #[inline]
    fn sub(self, rhs: Self) -> Self::Output {
        (self.start_address - rhs.start_address) / S::SIZE
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct PageRange<S: PageSize = Size4KiB> {
    pub start: Page<S>,
    pub end: Page<S>,
}

impl<S: PageSize> Iterator for PageRange<S> {
    type Item = Page<S>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.start < self.end {
            let page = self.start;
            self.start += 1;
            Some(page)
        } else {
            None
        }
    }
}

impl<S: PageSize> fmt::Debug for PageRange<S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PageRange")
            .field("start", &self.start)
            .field("end", &self.end)
            .finish()
    }
}

#[derive(Debug)]
pub struct AddressNotAligned;

impl fmt::Display for AddressNotAligned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the given address was not sufficiently aligned")
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub struct PhysFrame<S: PageSize = Size4KiB> {
    pub(crate) start_address: PhysAddr,
    size: PhantomData<S>,
}

impl<S: PageSize> PhysFrame<S> {
    #[inline]
    pub fn from_start_address(address: PhysAddr) -> Result<Self, AddressNotAligned> {
        if !address.is_aligned(S::SIZE) {
            return Err(AddressNotAligned);
        }
        Ok(PhysFrame::containing_address(address))
    }

    #[inline]
    pub const fn range(start: Self, end: Self) -> PhysFrameRange<S> {
        PhysFrameRange { start, end }
    }

    #[inline]
    pub fn containing_address(address: PhysAddr) -> Self {
        PhysFrame {
            start_address: address.align_down(S::SIZE),
            size: PhantomData,
        }
    }

    #[inline]
    pub fn start_address(&self) -> PhysAddr {
        self.start_address
    }

    pub fn as_slice_mut<T>(&self) -> &mut [T] {
        assert!(core::mem::size_of::<T>() < S::SIZE as usize);

        let virt = self.start_address().as_hhdm_virt();
        let ptr = virt.as_mut_ptr::<T>();

        unsafe {
            core::slice::from_raw_parts_mut(ptr, S::SIZE as usize / core::mem::size_of::<T>())
        }
    }
}

impl<S: PageSize> fmt::Debug for PhysFrame<S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_fmt(format_args!(
            "PhysFrame[{}]({:#x})",
            S::DEBUG_STR,
            self.start_address().as_u64()
        ))
    }
}

impl<S: PageSize> Add<u64> for PhysFrame<S> {
    type Output = Self;

    #[inline]
    fn add(self, rhs: u64) -> Self::Output {
        PhysFrame::containing_address(self.start_address() + rhs * S::SIZE)
    }
}

impl<S: PageSize> AddAssign<u64> for PhysFrame<S> {
    #[inline]
    fn add_assign(&mut self, rhs: u64) {
        *self = *self + rhs;
    }
}

impl<S: PageSize> Sub<u64> for PhysFrame<S> {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: u64) -> Self::Output {
        PhysFrame::containing_address(self.start_address() - rhs * S::SIZE)
    }
}

impl<S: PageSize> SubAssign<u64> for PhysFrame<S> {
    #[inline]
    fn sub_assign(&mut self, rhs: u64) {
        *self = *self - rhs;
    }
}

impl<S: PageSize> Sub<PhysFrame<S>> for PhysFrame<S> {
    type Output = u64;

    #[inline]
    fn sub(self, rhs: PhysFrame<S>) -> Self::Output {
        (self.start_address - rhs.start_address) / S::SIZE
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(C)]
pub struct PhysFrameRange<S: PageSize = Size4KiB> {
    pub start: PhysFrame<S>,
    pub end: PhysFrame<S>,
}

impl<S: PageSize> Iterator for PhysFrameRange<S> {
    type Item = PhysFrame<S>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.start < self.end {
            let frame = self.start;
            self.start += 1;
            Some(frame)
        } else {
            None
        }
    }
}

impl<S: PageSize> fmt::Debug for PhysFrameRange<S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PhysFrameRange")
            .field("start", &self.start)
            .field("end", &self.end)
            .finish()
    }
}

/// A range of physical memory frames, inclusive of the upper bound.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct PhysFrameRangeInclusive<S: PageSize = Size4KiB> {
    pub start: PhysFrame<S>,
    pub end: PhysFrame<S>,
}

impl<S: PageSize> Iterator for PhysFrameRangeInclusive<S> {
    type Item = PhysFrame<S>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.start <= self.end {
            let frame = self.start;
            self.start += 1;
            Some(frame)
        } else {
            None
        }
    }
}

impl<S: PageSize> fmt::Debug for PhysFrameRangeInclusive<S> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("PhysFrameRangeInclusive")
            .field("start", &self.start)
            .field("end", &self.end)
            .finish()
    }
}
