pub mod ps2;

use crate::arch::x86_64::halt;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use alloc::collections::VecDeque;
use lazy_static::lazy_static;
use spin::Mutex;

pub(crate) const PS2_PACKET_SIZE: usize = 3;
const MAX_BUFFERED_PACKETS: usize = 256;

lazy_static! {
    static ref MOUSE_BUFFER: Mutex<VecDeque<u8>> =
        Mutex::new(VecDeque::with_capacity(MAX_BUFFERED_PACKETS * PS2_PACKET_SIZE));
}

pub(crate) fn enqueue_packet(packet: [u8; PS2_PACKET_SIZE]) {
    let mut queue = MOUSE_BUFFER.lock();
    for byte in packet {
        if queue.len() >= MAX_BUFFERED_PACKETS * PS2_PACKET_SIZE {
            queue.pop_front();
        }
        queue.push_back(byte);
    }
}

fn queued_bytes() -> usize {
    MOUSE_BUFFER.lock().len()
}

fn pop_bytes(buf: &mut [u8]) -> usize {
    let mut queue = MOUSE_BUFFER.lock();
    let mut bytes_read = 0;

    while bytes_read < buf.len() && queue.len() >= PS2_PACKET_SIZE {
        for _ in 0..PS2_PACKET_SIZE {
            if bytes_read >= buf.len() {
                break;
            }
            if let Some(byte) = queue.pop_front() {
                buf[bytes_read] = byte;
                bytes_read += 1;
            }
        }
    }

    bytes_read
}

pub struct MouseDev;

impl MouseDev {
    pub const fn new() -> Self {
        Self
    }
}

impl VfsNodeOps for MouseDev {
    fn read(&self, _device: &mut BlockDev, _lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        let packet_quota = buf.len() / PS2_PACKET_SIZE;
        if packet_quota == 0 {
            return Ok(0);
        }

        let bytes_target = packet_quota * PS2_PACKET_SIZE;

        let bytes_read = loop {
            let read_now = pop_bytes(&mut buf[..bytes_target]);
            if read_now > 0 {
                break read_now;
            }
            halt();
        };

        Ok(bytes_read)
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        Ok(queued_bytes() >= PS2_PACKET_SIZE)
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}
