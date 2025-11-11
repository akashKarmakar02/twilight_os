use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use alloc::vec;
use alloc::vec::Vec;

pub struct Zero;

impl VfsNodeOps for Zero {
    fn read(&self, _device: &mut BlockDev, _lba: usize, buf: &mut [u8]) -> Result<Vec<u8>, ()> {
        let len = buf.len().max(1);
        Ok(vec![0u8; len])
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}
