use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use crate::sys::kmsg;

pub struct KmsgDev;

impl VfsNodeOps for KmsgDev {
    fn read(&self, _device: &mut BlockDev, offset: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let (n, _) = kmsg::read(offset, buf);
        Ok(n)
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, data: &[u8]) -> Result<(), ()> {
        kmsg::push_user(data);
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(true)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        match _cmd {
            kmsg::IOCTL_KMSG_GET_HEAD => Ok(kmsg::head_offset() as i64),
            _ => Ok(0),
        }
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}
