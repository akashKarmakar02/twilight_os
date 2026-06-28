use crate::sys::memory::{
    allocate_zeroed_frame, deallocate_frame, map_user_frame, phys_to_virt,
    user_page_flags_with_access,
};
use crate::sys::proc::mem::{PAGE, align_up};
use alloc::string::String;
use alloc::vec::Vec;
use twilight_common::syscall::types::{EINVAL, EIO};
use x86_64::structures::paging::{OffsetPageTable, PhysFrame, Size4KiB};

#[derive(Debug)]
pub struct MemFd {
    name: String,
    len: usize,
    pages: Vec<PhysFrame<Size4KiB>>,
}

impl MemFd {
    /// Creates a new empty `MemFd` with the given name.
    ///
    /// # Examples
    ///
    /// ```
    /// let fd = MemFd::new("tmp".to_string());
    /// assert_eq!(fd.debug_name(), "tmp");
    /// assert_eq!(fd.len(), 0);
    /// ```
    pub fn new(name: String) -> Self {
        Self {
            name,
            len: 0,
            pages: Vec::new(),
        }
    }

    /// Returns the memfd's name.
    ///
    /// # Examples
    ///
    /// ```
    /// let memfd = MemFd::new("tmp".to_string());
    /// assert_eq!(memfd.debug_name(), "tmp");
    /// ```
    pub fn debug_name(&self) -> &str {
        &self.name
    }

    /// Returns the logical length of the memfd.
    ///
    /// # Examples
    ///
    /// ```
    /// let memfd = MemFd::new("tmp".to_string());
    /// assert_eq!(memfd.len(), 0);
    /// ```
    pub fn len(&self) -> usize {
        self.len
    }

    /// Adjusts the logical length and backing storage to the given size.
    ///
    /// Extends the contents with zeroed bytes when the file grows.
    —
    ///
    /// # Errors
    ///
    /// Returns `Err(EIO)` if a required frame allocation fails.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut memfd = MemFd::new("demo".to_string());
    /// memfd.truncate(4096).unwrap();
    /// assert_eq!(memfd.len(), 4096);
    /// ```
    pub fn truncate(&mut self, len: usize) -> Result<(), i32> {
    pub fn truncate(&mut self, len: usize) -> Result<(), i32> {
        let needed_pages = if len == 0 { 0 } else { align_up(len, PAGE) / PAGE };
        while self.pages.len() < needed_pages {
            let Some(frame) = allocate_zeroed_frame() else {
                return Err(EIO);
            };
            self.pages.push(frame);
        }
        while self.pages.len() > needed_pages {
            if let Some(frame) = self.pages.pop() {
                deallocate_frame(frame);
            }
        }
        if len > self.len {
            self.zero_range(self.len, len - self.len)?;
        }
        self.len = len;
        Ok(())
    }

    /// Copies data from the file into a buffer starting at the given offset.
    
    ///
    
    /// # Examples
    
    ///
    
    /// ```
    
    /// let mut buf = [0u8; 8];
    
    /// let copied = memfd.read_at(0, &mut buf).unwrap();
    
    /// assert!(copied <= buf.len());
    
    /// ```
    pub fn read_at(&self, offset: usize, out: &mut [u8]) -> Result<usize, i32> {
        if offset >= self.len || out.is_empty() {
            return Ok(0);
        }
        let mut copied = 0usize;
        let mut file_off = offset;
        let want = out.len().min(self.len - offset);
        while copied < want {
            let page_index = file_off / PAGE;
            let page_off = file_off % PAGE;
            let count = (want - copied).min(PAGE - page_off);
            let Some(frame) = self.pages.get(page_index).copied() else {
                return Err(EIO);
            };
            let src = phys_to_virt(frame.start_address()).as_ptr::<u8>();
            // SAFETY: `frame` belongs to this memfd and the physical-memory
            // mapping covers the full page. Bounds are limited to PAGE bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(src.add(page_off), out[copied..].as_mut_ptr(), count);
            }
            copied += count;
            file_off += count;
        }
        Ok(copied)
    }

    /// Writes bytes into the file at the given offset.
    ///
    /// Extends the file if needed to fit the written data.
    ///
    /// # Errors
    ///
    /// Returns `EINVAL` if the offset and data length overflow, or `EIO` if backing
    /// storage is unavailable.
    ///
    /// # Examples
    ///
    /// ```
    /// let written = memfd.write_at(0, b"hello").unwrap();
    /// assert_eq!(written, 5);
    /// ```
    pub fn write_at(&mut self, offset: usize, data: &[u8]) -> Result<usize, i32> {
    pub fn write_at(&mut self, offset: usize, data: &[u8]) -> Result<usize, i32> {
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset.checked_add(data.len()).ok_or(EINVAL)?;
        if end > self.len {
            self.truncate(end)?;
        }
        let mut copied = 0usize;
        let mut file_off = offset;
        while copied < data.len() {
            let page_index = file_off / PAGE;
            let page_off = file_off % PAGE;
            let count = (data.len() - copied).min(PAGE - page_off);
            let Some(frame) = self.pages.get(page_index).copied() else {
                return Err(EIO);
            };
            let dst = phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
            // SAFETY: `frame` belongs to this memfd and is mapped in kernel
            // physical memory. Bounds are limited to PAGE bytes.
            unsafe {
                core::ptr::copy_nonoverlapping(data[copied..].as_ptr(), dst.add(page_off), count);
            }
            copied += count;
            file_off += count;
        }
        Ok(copied)
    }

    /// Maps a shared view of the backing pages into a user address range.
    
    ///
    
    /// The mapped range must start on a page boundary, fit within the current
    
    /// contents, and cover whole pages.
    
    ///
    
    /// # Errors
    
    ///
    
    /// Returns `EINVAL` if the offset is not page-aligned, the requested range
    
    /// overflows, or the requested data extends past the current length. Returns
    
    /// `EIO` if a backing page is missing or a page mapping fails.
    
    ///
    
    /// # Examples
    
    ///
    
    /// ```
    
    /// # let mut memfd = MemFd::new("example".to_string());
    
    /// # let mut mapper = unsafe { core::mem::MaybeUninit::zeroed().assume_init() };
    
    /// # let _ = memfd.truncate(4096);
    
    /// # let _ = memfd.map_shared(&mut mapper, 0x1000, 4096, 4096, PROT_READ, 0);
    
    /// ```
    pub fn map_shared(
        &mut self,
        mapper: &mut OffsetPageTable,
        va: usize,
        map_len: usize,
        requested_len: usize,
        prot: usize,
        offset: usize,
    ) -> Result<(), i32> {
        if offset & (PAGE - 1) != 0 {
            return Err(EINVAL);
        }
        let end = offset.checked_add(requested_len).ok_or(EINVAL)?;
        if end > self.len {
            return Err(EINVAL);
        }
        let writable = prot & crate::sys::proc::mem::PROT_WRITE != 0;
        let executable = prot & crate::sys::proc::mem::PROT_EXEC != 0;
        let flags = user_page_flags_with_access(true, writable, executable);
        let page_count = map_len / PAGE;
        let start_page = offset / PAGE;
        for index in 0..page_count {
            let Some(frame) = self.pages.get(start_page + index).copied() else {
                return Err(EIO);
            };
            map_user_frame(mapper, (va + index * PAGE) as u64, frame, flags).map_err(|_| EIO)?;
        }
        Ok(())
    }

    /// Zeroes a range within the file contents.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut memfd = MemFd::new("tmp".into());
    /// memfd.truncate(4096).unwrap();
    /// memfd.write_at(0, &[1, 2, 3, 4]).unwrap();
    /// memfd.zero_range(1, 2).unwrap();
    ///
    /// let mut buf = [0; 4];
    /// memfd.read_at(0, &mut buf).unwrap();
    /// assert_eq!(&buf, &[1, 0, 0, 4]);
    /// ```
    ```
    fn zero_range(&mut self, offset: usize, len: usize) -> Result<(), i32> {
        if len == 0 {
            return Ok(());
        }
        let end = offset.checked_add(len).ok_or(EINVAL)?;
        let mut file_off = offset;
        while file_off < end {
            let page_index = file_off / PAGE;
            let page_off = file_off % PAGE;
            let count = (end - file_off).min(PAGE - page_off);
            let Some(frame) = self.pages.get(page_index).copied() else {
                return Err(EIO);
            };
            let dst = phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
            // SAFETY: `frame` belongs to this memfd and the target byte range
            // is constrained to the single 4 KiB page.
            unsafe {
                core::ptr::write_bytes(dst.add(page_off), 0, count);
            }
            file_off += count;
        }
        Ok(())
    }
}

impl Drop for MemFd {
    /// Releases all physical frames owned by this `MemFd`.
    fn drop(&mut self) {
        for frame in self.pages.drain(..) {
            deallocate_frame(frame);
        }
    }
}
