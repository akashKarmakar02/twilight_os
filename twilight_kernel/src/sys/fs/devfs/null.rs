use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};

pub struct Null;

impl VfsNodeOps for Null {
    fn read(&self, _device: &mut BlockDev, _lba: usize, _buf: &mut [u8]) -> Result<usize, ()> {
        Ok(0)
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
