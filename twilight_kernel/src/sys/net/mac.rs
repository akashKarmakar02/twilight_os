use alloc::string::ToString;
use crate::driver::disk::ata::{FileIO, IO};
use crate::driver::nic::NET;

#[derive(Debug, Clone)]
pub struct NetMac;

impl NetMac {
    pub fn new() -> Self {
        Self
    }

    pub fn size() -> usize {
        17
    }
}

impl FileIO for NetMac {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        if let Some((ref mut iface, _)) = *NET.lock() {
            let s = iface.hardware_addr().to_string();
            let n = s.len();
            buf[0..n].copy_from_slice(s.as_bytes());
            return Ok(n);
        }
        Err(())
    }

    fn write(&mut self, _buf: &[u8]) -> Result<usize, ()> {
        Err(())
    }

    fn close(&mut self) {}

    fn poll(&mut self, event: IO) -> bool {
        match event {
            IO::Read => true,
            IO::Write => false,
        }
    }
}

