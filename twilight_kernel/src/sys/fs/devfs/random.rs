use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use crate::sys::rng;
use alloc::vec;
use alloc::vec::Vec;

fn random_bytes(len: usize) -> Vec<u8> {
    let len = len.max(1);
    let mut out = vec![0u8; len];
    let mut offset = 0;

    while offset < len {
        let chunk = rng::get_u64().to_le_bytes();
        let take = (len - offset).min(chunk.len());
        out[offset..offset + take].copy_from_slice(&chunk[..take]);
        offset += take;
    }

    out
}

pub struct RandomDev;
pub struct URandomDev;

impl VfsNodeOps for RandomDev {
    fn read(&self, _device: &mut BlockDev, _lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let random_bytes = random_bytes(buf.len());
        buf.copy_from_slice(&random_bytes);
        Ok(buf.len())
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

impl VfsNodeOps for URandomDev {
    fn read(&self, _device: &mut BlockDev, _lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let random_bytes = random_bytes(buf.len());
        buf.copy_from_slice(&random_bytes);
        Ok(buf.len())
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
