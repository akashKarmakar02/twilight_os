pub mod ps2;

use crate::arch::x86_64::halt;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use alloc::collections::VecDeque;
use lazy_static::lazy_static;
use crate::utils::sync::Mutex;

pub(crate) const PS2_PACKET_SIZE: usize = 3;
const MAX_PACKETS: usize = 256;

lazy_static! {
    static ref PACKETS: Mutex<VecDeque<[u8; PS2_PACKET_SIZE]>> =
        Mutex::new(VecDeque::with_capacity(MAX_PACKETS));
}

pub(crate) fn enqueue_packet(packet: [u8; PS2_PACKET_SIZE]) {
    let mut packets = PACKETS.lock_irq();
    if packets.len() == MAX_PACKETS {
        packets.pop_front();
    }
    packets.push_back(packet);
}

pub struct MouseDev;

impl MouseDev {
    pub const fn new() -> Self {
        Self
    }
}

impl VfsNodeOps for MouseDev {
    fn read(&self, _device: &mut BlockDev, _lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        if buf.len() < PS2_PACKET_SIZE {
            return Ok(0);
        }

        loop {
            if let Some(packet) = PACKETS.lock_irq().pop_front() {
                buf[..PS2_PACKET_SIZE].copy_from_slice(&packet);
                return Ok(PS2_PACKET_SIZE);
            }
            halt();
        }
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(!PACKETS.lock_irq().is_empty())
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}
