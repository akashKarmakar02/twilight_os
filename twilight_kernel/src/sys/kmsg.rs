use core::cmp::min;
use core::fmt::{self, Write};
use spin::mutex::Mutex;
use x86_64::instructions::interrupts;

pub const KMSG_RING_CAPACITY: usize = 64 * 1024;
pub const IOCTL_KMSG_GET_HEAD: u64 = 0x5457_1001;

struct KmsgRing {
    buf: [u8; KMSG_RING_CAPACITY],
    head: u64,
    tail: u64,
}

impl KmsgRing {
    const fn new() -> Self {
        Self {
            buf: [0; KMSG_RING_CAPACITY],
            head: 0,
            tail: 0,
        }
    }

    #[inline]
    fn append_byte(&mut self, byte: u8) {
        let idx = (self.tail as usize) % KMSG_RING_CAPACITY;
        self.buf[idx] = byte;
        self.tail = self.tail.saturating_add(1);

        let max_len = KMSG_RING_CAPACITY as u64;
        if self.tail.saturating_sub(self.head) > max_len {
            self.head = self.tail.saturating_sub(max_len);
        }
    }

    fn append_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.append_byte(b);
        }
    }

    fn read_from(&self, offset: u64, out: &mut [u8]) -> (usize, u64) {
        if out.is_empty() {
            return (0, offset);
        }

        let start = offset.max(self.head);
        if start >= self.tail {
            return (0, start);
        }

        let available = (self.tail - start) as usize;
        let n = min(out.len(), available);

        let ring_start = (start as usize) % KMSG_RING_CAPACITY;
        let first = min(n, KMSG_RING_CAPACITY - ring_start);
        out[..first].copy_from_slice(&self.buf[ring_start..ring_start + first]);
        if n > first {
            out[first..n].copy_from_slice(&self.buf[..(n - first)]);
        }

        (n, start.saturating_add(n as u64))
    }
}

impl Write for KmsgRing {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.append_bytes(s.as_bytes());
        Ok(())
    }
}

static KMSG_RING: Mutex<KmsgRing> = Mutex::new(KmsgRing::new());

pub fn push_log(args: fmt::Arguments) {
    interrupts::without_interrupts(|| {
        let mut ring = KMSG_RING.lock();
        let _ = ring.write_fmt(args);
        ring.append_byte(b'\n');
    });
}

pub fn push_user(data: &[u8]) {
    if data.is_empty() {
        return;
    }

    interrupts::without_interrupts(|| {
        let mut ring = KMSG_RING.lock();
        ring.append_bytes(data);
        if !data.ends_with(b"\n") {
            ring.append_byte(b'\n');
        }
    });
}

pub fn read(offset: usize, out: &mut [u8]) -> (usize, usize) {
    interrupts::without_interrupts(|| {
        let ring = KMSG_RING.lock();
        let (n, next) = ring.read_from(offset as u64, out);
        (n, next as usize)
    })
}

pub fn head_offset() -> usize {
    interrupts::without_interrupts(|| KMSG_RING.lock().head as usize)
}
