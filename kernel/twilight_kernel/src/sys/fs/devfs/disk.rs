use crate::driver::disk::{BLOCK_DEVICE, OPTICAL_BLOCK_DEVICE, USB_BLOCK_DEVICE};
use crate::sys::fs::vfs::BlockDev;
use crate::sys::fs::vfs::VfsNodeOps;
use alloc::vec;
use alloc::vec::Vec;
use core::cmp::min;
use core::convert::TryFrom;
use twilight_common::syscall::types::{EFAULT, ENOTTY};

// Linux-compatible block device ioctls (subset).
const BLKGETSIZE: u64 = 0x1260; // returns # of 512-byte sectors (unsigned long)
const BLKSSZGET: u64 = 0x1268; // returns sector size in bytes (int)
const BLKGETSIZE64: u64 = 0x8008_1272; // returns size in bytes (u64)

pub(crate) fn disk_geometry() -> Option<(usize, usize)> {
    #[allow(static_mut_refs)]
    unsafe {
        let dev = BLOCK_DEVICE.as_mut()?;
        let block_size = dev.block_size();
        let block_count = dev.block_count();
        if block_size == 0 || block_count == 0 {
            return None;
        }
        Some((block_size, block_count))
    }
}

pub(crate) fn disk_size_bytes() -> Option<usize> {
    let (bs, bc) = disk_geometry()?;
    bs.checked_mul(bc)
}

pub(crate) fn usb_disk_geometry() -> Option<(usize, usize)> {
    #[allow(static_mut_refs)]
    unsafe {
        let dev = USB_BLOCK_DEVICE.as_mut()?;
        let block_size = dev.block_size();
        let block_count = dev.block_count();
        if block_size == 0 || block_count == 0 {
            return None;
        }
        Some((block_size, block_count))
    }
}

pub(crate) fn usb_disk_size_bytes() -> Option<usize> {
    let (bs, bc) = usb_disk_geometry()?;
    bs.checked_mul(bc)
}

pub(crate) fn optical_disk_geometry() -> Option<(usize, usize)> {
    #[allow(static_mut_refs)]
    unsafe {
        let dev = OPTICAL_BLOCK_DEVICE.as_mut()?;
        let block_size = dev.block_size();
        let block_count = dev.block_count();
        if block_size == 0 || block_count == 0 {
            return None;
        }
        Some((block_size, block_count))
    }
}

pub(crate) fn optical_disk_size_bytes() -> Option<usize> {
    let (block_size, block_count) = optical_disk_geometry()?;
    block_size.checked_mul(block_count)
}

pub struct Disk0;
pub struct Disk1;
pub struct Cdrom0;

impl Cdrom0 {
    fn with_dev_mut<F, R>(f: F) -> Result<R, ()>
    where
        F: FnOnce(&mut dyn crate::driver::disk::BlockDeviceIO) -> Result<R, ()>,
    {
        #[allow(static_mut_refs)]
        unsafe {
            match OPTICAL_BLOCK_DEVICE.as_mut() {
                Some(dev) => f(*dev),
                None => Err(()),
            }
        }
    }

    fn read_exact_block(block_index: usize, out: &mut [u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.read(block_index, out))
    }

    fn read_blocks(block_index: usize, out: &mut [u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.read_blocks(block_index, out))
    }
}

impl VfsNodeOps for Cdrom0 {
    fn read(&self, _device: &mut BlockDev, offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
        if buf.is_empty() {
            return Ok(0);
        }

        let (block_size, block_count) = optical_disk_geometry().ok_or(())?;
        let disk_size = block_size.checked_mul(block_count).ok_or(())?;
        if offset >= disk_size {
            return Ok(0);
        }

        let mut remaining = min(buf.len(), disk_size - offset);
        let mut pos = offset;
        let mut out_off = 0usize;
        let mut scratch: Option<Vec<u8>> = None;

        while remaining > 0 {
            let block_index = pos / block_size;
            let block_off = pos % block_size;

            if block_off == 0 && remaining >= block_size {
                let whole = remaining - (remaining % block_size);
                if whole > 0 {
                    Self::read_blocks(block_index, &mut buf[out_off..out_off + whole])?;
                    pos += whole;
                    out_off += whole;
                    remaining -= whole;
                    continue;
                }
            }

            let take = min(remaining, block_size - block_off);
            let tmp = scratch.get_or_insert_with(|| vec![0u8; block_size]);
            Self::read_exact_block(block_index, tmp)?;
            buf[out_off..out_off + take].copy_from_slice(&tmp[block_off..block_off + take]);
            pos += take;
            out_off += take;
            remaining -= take;
        }

        Ok(out_off)
    }

    fn write(&mut self, _device: &mut BlockDev, _offset: usize, _data: &[u8]) -> Result<(), ()> {
        Err(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, arg: usize) -> Result<i64, ()> {
        let (block_size, block_count) = match optical_disk_geometry() {
            Some(v) => v,
            None => return Ok(-(ENOTTY as i64)),
        };
        let disk_size = match block_size.checked_mul(block_count) {
            Some(v) => v as u64,
            None => return Ok(-(ENOTTY as i64)),
        };

        if arg == 0 {
            return Ok(-(EFAULT as i64));
        }
        match cmd {
            BLKSSZGET => unsafe { *(arg as *mut i32) = block_size as i32 },
            BLKGETSIZE64 => unsafe { *(arg as *mut u64) = disk_size },
            BLKGETSIZE => unsafe { *(arg as *mut u64) = disk_size / 512 },
            _ => return Ok(-(ENOTTY as i64)),
        }
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

impl Disk0 {
    fn with_dev_mut<F, R>(f: F) -> Result<R, ()>
    where
        F: FnOnce(&mut dyn crate::driver::disk::BlockDeviceIO) -> Result<R, ()>,
    {
        #[allow(static_mut_refs)]
        unsafe {
            match BLOCK_DEVICE.as_mut() {
                Some(dev) => f(*dev),
                None => Err(()),
            }
        }
    }

    fn read_exact_block(block_index: usize, out: &mut [u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.read(block_index, out))
    }

    fn write_exact_block(block_index: usize, data: &[u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.write(block_index, data))
    }

    fn read_blocks(block_index: usize, out: &mut [u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.read_blocks(block_index, out))
    }

    fn write_blocks(block_index: usize, data: &[u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.write_blocks(block_index, data))
    }
}

impl VfsNodeOps for Disk0 {
    fn read(&self, _device: &mut BlockDev, offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
        if buf.is_empty() {
            return Ok(0);
        }

        let (block_size, block_count) = disk_geometry().ok_or(())?;
        let disk_size = block_size.checked_mul(block_count).ok_or(())?;
        if offset >= disk_size {
            return Ok(0);
        }

        let mut remaining = min(buf.len(), disk_size - offset);
        let mut pos = offset;
        let mut out_off = 0usize;
        let mut scratch: Option<Vec<u8>> = None;

        while remaining > 0 {
            let block_index = pos / block_size;
            let block_off = pos % block_size;

            // Fast path: full blocks, aligned.
            if block_off == 0 && remaining >= block_size {
                let whole = remaining - (remaining % block_size);
                if whole > 0 {
                    let out = &mut buf[out_off..out_off + whole];
                    Self::read_blocks(block_index, out)?;
                    pos += whole;
                    out_off += whole;
                    remaining -= whole;
                    continue;
                }
            }

            // Slow path: partial block.
            let take = min(remaining, block_size - block_off);
            let tmp = scratch.get_or_insert_with(|| vec![0u8; block_size]);
            Self::read_exact_block(block_index, tmp)?;
            buf[out_off..out_off + take].copy_from_slice(&tmp[block_off..block_off + take]);
            pos += take;
            out_off += take;
            remaining -= take;
        }

        Ok(out_off)
    }

    fn write(&mut self, _device: &mut BlockDev, offset: usize, data: &[u8]) -> Result<(), ()> {
        if data.is_empty() {
            return Ok(());
        }

        let (block_size, block_count) = disk_geometry().ok_or(())?;
        let disk_size = block_size.checked_mul(block_count).ok_or(())?;
        let end = offset.checked_add(data.len()).ok_or(())?;
        if end > disk_size {
            return Err(());
        }

        let mut remaining = data.len();
        let mut pos = offset;
        let mut in_off = 0usize;
        let mut scratch: Option<Vec<u8>> = None;

        while remaining > 0 {
            let block_index = pos / block_size;
            let block_off = pos % block_size;

            // Fast path: full blocks, aligned.
            if block_off == 0 && remaining >= block_size {
                let whole = remaining - (remaining % block_size);
                if whole > 0 {
                    let chunk = &data[in_off..in_off + whole];
                    Self::write_blocks(block_index, chunk)?;
                    pos += whole;
                    in_off += whole;
                    remaining -= whole;
                    continue;
                }
            }

            // Slow path: partial block (RMW).
            let take = min(remaining, block_size - block_off);
            let tmp = scratch.get_or_insert_with(|| vec![0u8; block_size]);
            Self::read_exact_block(block_index, tmp)?;
            tmp[block_off..block_off + take].copy_from_slice(&data[in_off..in_off + take]);
            Self::write_exact_block(block_index, tmp)?;

            pos += take;
            in_off += take;
            remaining -= take;
        }

        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, arg: usize) -> Result<i64, ()> {
        let (block_size, block_count) = match disk_geometry() {
            Some(v) => v,
            None => return Ok(-(ENOTTY as i64)),
        };
        let disk_size = match block_size.checked_mul(block_count) {
            Some(v) => v as u64,
            None => return Ok(-(ENOTTY as i64)),
        };

        match cmd {
            BLKSSZGET => {
                if arg == 0 {
                    return Ok(-(EFAULT as i64));
                }
                unsafe {
                    *(arg as *mut i32) = block_size as i32;
                }
                Ok(0)
            }
            BLKGETSIZE64 => {
                if arg == 0 {
                    return Ok(-(EFAULT as i64));
                }
                unsafe {
                    *(arg as *mut u64) = disk_size;
                }
                Ok(0)
            }
            BLKGETSIZE => {
                if arg == 0 {
                    return Ok(-(EFAULT as i64));
                }
                let sectors_512 = disk_size / 512;
                unsafe {
                    *(arg as *mut u64) = sectors_512;
                }
                Ok(0)
            }
            _ => Ok(-(ENOTTY as i64)),
        }
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

impl Disk1 {
    fn with_dev_mut<F, R>(f: F) -> Result<R, ()>
    where
        F: FnOnce(&mut dyn crate::driver::disk::BlockDeviceIO) -> Result<R, ()>,
    {
        #[allow(static_mut_refs)]
        unsafe {
            match USB_BLOCK_DEVICE.as_mut() {
                Some(dev) => f(*dev),
                None => Err(()),
            }
        }
    }

    fn read_exact_block(block_index: usize, out: &mut [u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.read(block_index, out))
    }

    fn write_exact_block(block_index: usize, data: &[u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.write(block_index, data))
    }

    fn read_blocks(block_index: usize, out: &mut [u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.read_blocks(block_index, out))
    }

    fn write_blocks(block_index: usize, data: &[u8]) -> Result<(), ()> {
        let block_index = u32::try_from(block_index).map_err(|_| ())?;
        Self::with_dev_mut(|dev| dev.write_blocks(block_index, data))
    }
}

impl VfsNodeOps for Disk1 {
    fn read(&self, _device: &mut BlockDev, offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
        if buf.is_empty() {
            return Ok(0);
        }

        let (block_size, block_count) = usb_disk_geometry().ok_or(())?;
        let disk_size = block_size.checked_mul(block_count).ok_or(())?;
        if offset >= disk_size {
            return Ok(0);
        }

        let mut remaining = min(buf.len(), disk_size - offset);
        let mut pos = offset;
        let mut out_off = 0usize;
        let mut scratch: Option<Vec<u8>> = None;

        while remaining > 0 {
            let block_index = pos / block_size;
            let block_off = pos % block_size;

            if block_off == 0 && remaining >= block_size {
                let whole = remaining - (remaining % block_size);
                if whole > 0 {
                    let out = &mut buf[out_off..out_off + whole];
                    Self::read_blocks(block_index, out)?;
                    pos += whole;
                    out_off += whole;
                    remaining -= whole;
                    continue;
                }
            }

            let take = min(remaining, block_size - block_off);
            let tmp = scratch.get_or_insert_with(|| vec![0u8; block_size]);
            Self::read_exact_block(block_index, tmp)?;
            buf[out_off..out_off + take].copy_from_slice(&tmp[block_off..block_off + take]);
            pos += take;
            out_off += take;
            remaining -= take;
        }

        Ok(out_off)
    }

    fn write(&mut self, _device: &mut BlockDev, offset: usize, data: &[u8]) -> Result<(), ()> {
        if data.is_empty() {
            return Ok(());
        }

        let (block_size, block_count) = usb_disk_geometry().ok_or(())?;
        let disk_size = block_size.checked_mul(block_count).ok_or(())?;
        let end = offset.checked_add(data.len()).ok_or(())?;
        if end > disk_size {
            return Err(());
        }

        let mut remaining = data.len();
        let mut pos = offset;
        let mut in_off = 0usize;
        let mut scratch: Option<Vec<u8>> = None;

        while remaining > 0 {
            let block_index = pos / block_size;
            let block_off = pos % block_size;

            if block_off == 0 && remaining >= block_size {
                let whole = remaining - (remaining % block_size);
                if whole > 0 {
                    let chunk = &data[in_off..in_off + whole];
                    Self::write_blocks(block_index, chunk)?;
                    pos += whole;
                    in_off += whole;
                    remaining -= whole;
                    continue;
                }
            }

            let take = min(remaining, block_size - block_off);
            let tmp = scratch.get_or_insert_with(|| vec![0u8; block_size]);
            Self::read_exact_block(block_index, tmp)?;
            tmp[block_off..block_off + take].copy_from_slice(&data[in_off..in_off + take]);
            Self::write_exact_block(block_index, tmp)?;

            pos += take;
            in_off += take;
            remaining -= take;
        }

        Ok(())
    }

    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, arg: usize) -> Result<i64, ()> {
        let (block_size, block_count) = match usb_disk_geometry() {
            Some(v) => v,
            None => return Ok(-(ENOTTY as i64)),
        };
        let disk_size = match block_size.checked_mul(block_count) {
            Some(v) => v as u64,
            None => return Ok(-(ENOTTY as i64)),
        };

        match cmd {
            BLKSSZGET => {
                if arg == 0 {
                    return Ok(-(EFAULT as i64));
                }
                unsafe {
                    *(arg as *mut i32) = block_size as i32;
                }
                Ok(0)
            }
            BLKGETSIZE64 => {
                if arg == 0 {
                    return Ok(-(EFAULT as i64));
                }
                unsafe {
                    *(arg as *mut u64) = disk_size;
                }
                Ok(0)
            }
            BLKGETSIZE => {
                if arg == 0 {
                    return Ok(-(EFAULT as i64));
                }
                let sectors_512 = disk_size / 512;
                unsafe {
                    *(arg as *mut u64) = sectors_512;
                }
                Ok(0)
            }
            _ => Ok(-(ENOTTY as i64)),
        }
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}
