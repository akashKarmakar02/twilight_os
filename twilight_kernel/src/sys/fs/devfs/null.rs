use alloc::vec::Vec;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};

pub struct Null;

impl VfsNodeOps for Null {
    fn read(&self, _device: &mut BlockDev, _lba: usize, _buf: &mut [u8]) -> Result<Vec<u8>, ()> {
        Ok(Vec::new())
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }
}