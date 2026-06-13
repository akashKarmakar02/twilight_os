pub mod ps2;

use crate::arch::x86_64::halt;
use crate::sys::fs::vfs::{BlockDev, VfsNodeOps};
use core::sync::atomic::{AtomicU64, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;

pub(crate) const PS2_PACKET_SIZE: usize = 3;

lazy_static! {
    static ref LAST_PACKET: Mutex<[u8; PS2_PACKET_SIZE]> = Mutex::new([0; PS2_PACKET_SIZE]);
}

static PACKET_SEQ: AtomicU64 = AtomicU64::new(0);
static LAST_SEQ_SEEN_BY_POLL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn enqueue_packet(packet: [u8; PS2_PACKET_SIZE]) {
    *LAST_PACKET.lock() = packet;
    PACKET_SEQ.fetch_add(1, Ordering::Release);
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

        // Important: do not replay buffered mouse movement when no process was reading.
        // We always wait for a *new* packet after the read begins.
        let start_seq = PACKET_SEQ.load(Ordering::Acquire);
        loop {
            let now = PACKET_SEQ.load(Ordering::Acquire);
            if now != start_seq {
                break;
            }
            halt();
        }

        let packet = *LAST_PACKET.lock();
        buf[..PS2_PACKET_SIZE].copy_from_slice(&packet);
        Ok(PS2_PACKET_SIZE)
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, _data: &[u8]) -> Result<(), ()> {
        Ok(())
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        let seq = PACKET_SEQ.load(Ordering::Acquire);
        let last = LAST_SEQ_SEEN_BY_POLL.load(Ordering::Relaxed);
        if seq != last {
            LAST_SEQ_SEEN_BY_POLL.store(seq, Ordering::Relaxed);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn ioctl(&mut self, _device: &mut BlockDev, _cmd: u64, _arg: usize) -> Result<i64, ()> {
        Ok(0)
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}
