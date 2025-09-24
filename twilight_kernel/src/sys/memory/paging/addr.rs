use crate::fs::twilight_fs::FsError;
use core::fmt;
use core::iter::Step;
use core::ops::{Add, AddAssign, Sub, SubAssign};
use core::sync::atomic::Ordering;

use super::frame::VmFrame;
use x86_64::structures::paging::{PageOffset, PageSize, Size4KiB};

use crate::sys::memory::paging::PageTableIndex;
use crate::sys::syscall;
use bit_field::BitField;
pub(crate) use x86_64::{align_down, align_up};


#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct VirtAddr(u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PhysAddr(u64);

#[derive(Clone, Copy, Debug)]
pub enum ReadErr{
    Null,
    NotAligned,
}

impl From<ReadErr> for FsError {
    fn from(_: ReadErr) -> Self {
        FsError::NotSupported
    }
}

impl From<ReadErr> for syscall::SyscallError {
    fn from(value: ReadErr) -> Self {
        match value {
            ReadErr::Null => Self::EINVAL,
            ReadErr::NotAligned => Self::EACCES,
        }
    }
}

impl VirtAddr {
    #[inline]
    pub const fn new(addr: u64) -> VirtAddr {
        VirtAddr(addr)
    }

    #[inline]
    pub const fn zero() -> VirtAddr {
        VirtAddr(0)
    }

    #[inline]
    pub const fn as_u64(&self) -> u64 {
        self.0
    }

    #[cfg(target_pointer_width = "64")]
    #[inline]
    pub const fn as_ptr<T>(self) -> *const T {
        self.as_u64() as *const T
    }

    #[cfg(target_pointer_width = "64")]
    #[inline]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.as_ptr::<T>().cast_mut()
    }

    #[inline]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub fn is_canonical(self) -> bool {
        let virtual_mask_shift = if super::level_5_paging_enabled() {
            56
        } else {
            47
        };

        let shift = 64 - (virtual_mask_shift + 1);

        ((self.as_u64() << shift) as i64 >> shift) as u64 == self.as_u64()
    }

    pub fn read_mut<'a, T: Sized>(&self) -> Result<&'a mut T, ReadErr> {
        self.validate_read::<T>()?;
        Ok(unsafe { &mut *self.as_mut_ptr() })
    }

    pub fn as_bytes_mut<'a>(&self, size_bytes: usize) -> &'a mut [u8] {
        self.validate_read::<&[u8]>().unwrap();
        unsafe { core::slice::from_raw_parts_mut(self.as_mut_ptr() as *mut u8, size_bytes) }
    }

    pub fn as_hhdm_phys(&self) -> PhysAddr {
        // TODO: from unsafe { PhysAddr::new(*self - crate::memory::PHYSICAL_MEMORY_OFFSET)} to
        #[allow(static_mut_refs)]
        unsafe { PhysAddr::new(self.as_u64() - crate::memory::PHYSICAL_MEMORY_OFFSET.load(Ordering::Relaxed))}
    }

    fn validate_read<T: Sized>(&self) -> Result<(), ReadErr> {
        let raw = self.as_ptr::<T>();

        if raw.is_null() {
            return Err(ReadErr::Null);
        } else if !raw.is_aligned() {
            return Err(ReadErr::NotAligned);
        }

        Ok(())
    }

    #[inline]
    pub fn align_down<U>(self, align: U) -> Self
    where
    U: Into<u64>,
    {
        VirtAddr(align_down(self.0, align.into()))
    }

    pub fn is_aligned<U>(&self, align: U) -> bool
    where
        U: Into<u64>,
    {
        // TODO: Don't know
        is_aligned(self.0, align.into())
    }

    #[inline]
    pub const fn page_offset(self) -> PageOffset {
        PageOffset::new_truncate(self.0 as u16)
    }

    #[inline]
    pub const fn p1_index(self) -> PageTableIndex {
        PageTableIndex::new_truncate((self.0 >> 12) as u16)
    }

    #[inline]
    pub const fn p2_index(self) -> PageTableIndex {
        PageTableIndex::new_truncate((self.0 >> 12 >> 9) as u16)
    }

    #[inline]
    pub const fn p3_index(self) -> PageTableIndex {
        PageTableIndex::new_truncate((self.0 >> 12 >> 9 >> 9) as u16)
    }

    #[inline]
    pub const fn p4_index(self) -> PageTableIndex {
        PageTableIndex::new_truncate((self.0 >> 12 >> 9 >> 9 >> 9) as u16)
    }

    #[inline]
    pub const fn p5_index(self) -> PageTableIndex {
        PageTableIndex::new_truncate((self.0 >> 12 >> 9 >> 9 >> 9 >> 9) as u16)
    }

    pub const fn const_sub_u64(self, other: u64) -> VirtAddr {
        VirtAddr(self.0 - other)
    }
}

impl fmt::Debug for VirtAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("VirtAddr")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

impl fmt::Binary for VirtAddr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Binary::fmt(&self.0, f)
    }
}

impl fmt::LowerHex for VirtAddr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

impl fmt::Octal for VirtAddr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Octal::fmt(&self.0, f)
    }
}

impl fmt::UpperHex for VirtAddr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::UpperHex::fmt(&self.0, f)
    }
}

impl fmt::Pointer for VirtAddr {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&(self.0 as *const ()), f)
    }
}

impl Add<u64> for VirtAddr {
    type Output = Self;

    #[inline]
    fn add(self, rhs: u64) -> Self::Output {
        VirtAddr(self.0 + rhs)
    }
}

impl AddAssign<u64> for VirtAddr {
    #[inline]
    fn add_assign(&mut self, rhs: u64) {
        *self = *self + rhs;
    }
}

#[cfg(target_pointer_width = "64")]
impl Add<usize> for VirtAddr {
    type Output = Self;

    #[inline]
    fn add(self, rhs: usize) -> Self::Output {
        self + rhs as u64
    }
}

#[cfg(target_pointer_width = "64")]
impl AddAssign<usize> for VirtAddr {
    #[inline]
    fn add_assign(&mut self, rhs: usize) {
        self.add_assign(rhs as u64)
    }
}

impl Sub<u64> for VirtAddr {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: u64) -> Self::Output {
        VirtAddr::new(self.0.checked_sub(rhs).unwrap())
    }
}

impl SubAssign<u64> for VirtAddr {
    #[inline]
    fn sub_assign(&mut self, rhs: u64) {
        *self = *self - rhs;
    }
}

#[cfg(target_pointer_width = "64")]
impl Sub<usize> for VirtAddr {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: usize) -> Self::Output {
        self -rhs as u64
    }
}

#[cfg(target_pointer_width = "64")]
impl SubAssign<usize> for VirtAddr {
    #[inline]
    fn sub_assign(&mut self, rhs: usize) {
        self.sub_assign(rhs as u64)
    }
}

impl Sub<VirtAddr> for VirtAddr {
    type Output = u64;

    #[inline]
    fn sub(self, rhs: VirtAddr) -> Self::Output {
        self.as_u64().checked_sub(rhs.as_u64()).unwrap()
    }
}

impl Step for VirtAddr {
    #[inline]
    fn steps_between(start: &Self, end: &Self) -> (usize, Option<usize>) {
        if start < end {
            let n = (end.as_u64() - start.as_u64()) as usize;
            (n, Some(n))
        } else {
            (0, None)
        }
    }

    #[inline]
    fn forward_checked(start: Self, count: usize) -> Option<Self> {
        Some(start + count)
    }

    #[inline]
    fn backward_checked(start: Self, count: usize) -> Option<Self> {
        Some(start - count)
    }
}

impl PhysAddr {
    pub fn new(addr: u64) -> PhysAddr {
        assert_eq!(
            addr.get_bits(52..64),
            0,
            "physical addresses must not have any bits in the range 52 to 64 set"
        );

        unsafe { PhysAddr::new_unchecked(addr) }
    }

    pub const fn zero() -> Self {
        unsafe { Self::new_unchecked(0) }
    }

    pub fn as_vm_frame(&self) -> Option<&'static VmFrame> {
        let frames = super::get_vm_frames();

        if let Some(frames) = frames {
            let index = (self.align_down(Size4KiB::SIZE).as_u64() / Size4KiB::SIZE) as usize;

            if index >= frames.len() {
                return None;
            }

            Some(&frames[index])
        } else {
            None
        }
    }

    pub const unsafe fn new_unchecked(addr: u64) -> PhysAddr {
        PhysAddr(addr)
    }

    pub fn as_hhdm_virt(&self) -> VirtAddr {
        // TODO: from unsafe { crate::PHYSICAL_MEMORY_OFFSET + self.as_u64() } to
        #[allow(static_mut_refs)]
        let addr = unsafe { crate::sys::memory::PHYSICAL_MEMORY_OFFSET.load(Ordering::Relaxed) + self.as_u64() };
        VirtAddr(addr)
    }

    pub const fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn align_down<U>(self, align: U) -> Self
    where
        U: Into<u64>,
    {
        PhysAddr(align_down(self.0, align.into()))
    }

    pub fn align_up<U>(self, align: U) -> Self
    where
        U: Into<u64>,
    {
        PhysAddr(align_up(self.0, align.into()))
    }

    pub fn is_aligned<U>(self, align: U) -> bool
    where
        U: Into<u64>,
    {
        self.align_down(align) == self
    }
}

impl fmt::Debug for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_tuple("PhysAddr")
            .field(&format_args!("{:#x}", self.0))
            .finish()
    }
}

impl fmt::Binary for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Binary::fmt(&self.0, f)
    }
}

impl fmt::LowerHex for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

impl fmt::Octal for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Octal::fmt(&self.0, f)
    }
}

impl fmt::UpperHex for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::UpperHex::fmt(&self.0, f)
    }
}

impl fmt::Pointer for PhysAddr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Pointer::fmt(&(self.0 as *const ()), f)
    }
}

impl Add<u64> for PhysAddr {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        PhysAddr::new(self.0 + rhs)
    }
}

impl AddAssign<u64> for PhysAddr {
    fn add_assign(&mut self, rhs: u64) {
        *self = *self + rhs;
    }
}

#[cfg(target_pointer_width = "64")]
impl Add<usize> for PhysAddr {
    type Output = Self;

    fn add(self, rhs: usize) -> Self::Output {
        self + rhs as u64
    }
}

#[cfg(target_pointer_width = "64")]
impl AddAssign<usize> for PhysAddr {
    fn add_assign(&mut self, rhs: usize) {
        self.add_assign(rhs as u64)
    }
}

impl Sub<u64> for PhysAddr {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        PhysAddr::new(self.0.checked_sub(rhs).unwrap())
    }
}

impl SubAssign<u64> for PhysAddr {
    fn sub_assign(&mut self, rhs: u64) {
        *self = *self - rhs;
    }
}

#[cfg(target_pointer_width = "64")]
impl Sub<usize> for PhysAddr {
    type Output = Self;

    #[inline]
    fn sub(self, rhs: usize) -> Self::Output {
        self - rhs as u64
    }
}

#[cfg(target_pointer_width = "64")]
impl SubAssign<usize> for PhysAddr {
    fn sub_assign(&mut self, rhs: usize) {
        self.sub_assign(rhs as u64)
    }
}

impl Sub<PhysAddr> for PhysAddr {
    type Output = u64;

    fn sub(self, rhs: PhysAddr) -> Self::Output {
        self.as_u64().checked_sub(rhs.as_u64()).unwrap()
    }
}

// `align_down` & 'align_up' is defined multiple times [E0255]
// #[inline]
// pub fn align_down(addr: u64, align: u64) -> u64 {
//     assert!(align.is_power_of_two(), "align must be a power of two");
//     addr & !(align - 1)
// }
//
// #[inline]
// pub const fn align_up(addr: u64, align: u64) -> u64 {
//     let align_mask = align - 1;
//
//     if addr & align_mask == 0 {
//         addr // already aligned
//     } else {
//         (addr | align_mask) + 1
//     }
// }

#[inline]
pub const fn is_aligned(addr: u64, align: u64) -> bool {
    align_up(addr, align) == addr
}

#[cfg(test)]
mod tests {
    use super::*;

    // #[test]
    pub fn test_align_down() {
        assert!(is_aligned(0x1000, 0x1000));
        assert!(!is_aligned(69, 0x1000));
    }

    // #[test]
    pub fn test_align_up() {
        // align 1
        assert_eq!(align_up(0, 1), 0);
        assert_eq!(align_up(1234, 1), 1234);
        assert_eq!(align_up(0xffff_ffff_ffff_ffff, 1), 0xffff_ffff_ffff_ffff);

        // align 2
        assert_eq!(align_up(0, 2), 0);
        assert_eq!(align_up(1233, 2), 1234);
        assert_eq!(align_up(0xffff_ffff_ffff_fffe, 2), 0xffff_ffff_ffff_fffe);

        // address 0
        assert_eq!(align_up(0, 128), 0);
        assert_eq!(align_up(0, 1), 0);
        assert_eq!(align_up(0, 2), 0);
        assert_eq!(align_up(0, 0x8000_0000_0000_0000), 0);
    }
}