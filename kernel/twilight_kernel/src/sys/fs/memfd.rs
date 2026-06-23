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
    pub fn new(name: String) -> Self {
        Self {
            name,
            len: 0,
            pages: Vec::new(),
        }
    }

    pub fn debug_name(&self) -> &str {
        &self.name
    }

    pub fn len(&self) -> usize {
        self.len
    }

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
    fn drop(&mut self) {
        for frame in self.pages.drain(..) {
            deallocate_frame(frame);
        }
    }
}
