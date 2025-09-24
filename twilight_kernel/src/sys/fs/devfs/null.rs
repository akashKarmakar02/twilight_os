use alloc::vec::Vec;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};

pub struct Null;

impl VfsNodeOps for Null {
    fn read(&self, _device: &mut BlockDev) -> Result<Vec<u8>, ()> {
        Ok(Vec::new())
    }

    fn write(&mut self, _device: &mut BlockDev, _data: &[u8]) -> Result<(), ()> {
        Ok(())
    }
}