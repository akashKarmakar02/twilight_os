use crate::driver::disk::dummy_blockdev;
use crate::sys::fs::vfs::{BlockDev, FileType, Metadata, VfsNode, VfsNodeOps};
use crate::task::executor::halt;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use spin::mutex::Mutex;
use spin::rwlock::RwLock;
use twilight_common::syscall::types::{EAGAIN, EPIPE};
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};

pub const IOCTL_PIPE_GET_ERRNO: u64 = 0x5457_0001; // "TW" private
pub const IOCTL_PIPE_GET_LAST_WRITE: u64 = 0x5457_0002;

const PIPE_CAPACITY: usize = 4096;

static NEXT_PIPE_INO: AtomicU32 = AtomicU32::new(10_000);

struct PipeInner {
    buf: VecDeque<u8>,
    readers: usize,
    writers: usize,
}

impl PipeInner {
    fn new() -> Self {
        Self {
            buf: VecDeque::with_capacity(PIPE_CAPACITY),
            readers: 0,
            writers: 0,
        }
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PipeEndKind {
    Read,
    Write,
}

pub struct PipeEnd {
    inner: Arc<Mutex<PipeInner>>,
    kind: PipeEndKind,
    nonblock: bool,
    last_errno: AtomicI32, // positive errno
    last_write: AtomicUsize,
}

impl PipeEnd {
    fn new(inner: Arc<Mutex<PipeInner>>, kind: PipeEndKind, nonblock: bool) -> Self {
        {
            let mut g = inner.lock();
            match kind {
                PipeEndKind::Read => g.readers += 1,
                PipeEndKind::Write => g.writers += 1,
            }
        }
        Self {
            inner,
            kind,
            nonblock,
            last_errno: AtomicI32::new(0),
            last_write: AtomicUsize::new(0),
        }
    }

    fn set_errno(&self, errno: i32) {
        self.last_errno.store(errno, Ordering::Relaxed);
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        let mut g = self.inner.lock();
        match self.kind {
            PipeEndKind::Read => g.readers = g.readers.saturating_sub(1),
            PipeEndKind::Write => g.writers = g.writers.saturating_sub(1),
        }
    }
}

impl VfsNodeOps for PipeEnd {
    fn read(&self, _device: &mut BlockDev, _lba: usize, buf: &mut [u8]) -> Result<usize, ()> {
        if self.kind != PipeEndKind::Read {
            self.set_errno(EPIPE);
            return Err(());
        }
        if buf.is_empty() {
            return Ok(0);
        }

        loop {
            let mut inner = self.inner.lock();

            if !inner.buf.is_empty() {
                let mut n = 0usize;
                while n < buf.len() {
                    match inner.buf.pop_front() {
                        Some(b) => {
                            buf[n] = b;
                            n += 1;
                        }
                        None => break,
                    }
                }
                self.set_errno(0);
                return Ok(n);
            }

            // Empty buffer: EOF if no writers.
            if inner.writers == 0 {
                self.set_errno(0);
                return Ok(0);
            }

            if self.nonblock {
                self.set_errno(EAGAIN);
                return Err(());
            }

            drop(inner);
            halt();
        }
    }

    fn write(&mut self, _device: &mut BlockDev, _lba: usize, data: &[u8]) -> Result<(), ()> {
        self.last_write.store(0, Ordering::Relaxed);

        if self.kind != PipeEndKind::Write {
            self.set_errno(EPIPE);
            return Err(());
        }
        if data.is_empty() {
            self.set_errno(0);
            return Ok(());
        }

        loop {
            let mut inner = self.inner.lock();

            // No readers: EPIPE
            if inner.readers == 0 {
                self.set_errno(EPIPE);
                return Err(());
            }

            let space = PIPE_CAPACITY.saturating_sub(inner.buf.len());
            if space == 0 {
                if self.nonblock {
                    self.set_errno(EAGAIN);
                    return Err(());
                }
                drop(inner);
                halt();
                continue;
            }

            let n = core::cmp::min(space, data.len());
            inner.buf.extend(data[..n].iter().copied());
            self.last_write.store(n, Ordering::Relaxed);
            self.set_errno(0);
            return Ok(());
        }
    }

    fn poll(&self, _device: &mut BlockDev) -> Result<bool, ()> {
        let inner = self.inner.lock();
        match self.kind {
            PipeEndKind::Read => Ok(!inner.buf.is_empty() || inner.writers == 0),
            PipeEndKind::Write => Ok(inner.readers > 0 && inner.buf.len() < PIPE_CAPACITY),
        }
    }

    fn ioctl(&mut self, _device: &mut BlockDev, cmd: u64, _arg: usize) -> Result<i64, ()> {
        match cmd {
            IOCTL_PIPE_GET_ERRNO => Ok(self.last_errno.load(Ordering::Relaxed) as i64),
            IOCTL_PIPE_GET_LAST_WRITE => Ok(self.last_write.load(Ordering::Relaxed) as i64),
            _ => Ok(0),
        }
    }

    fn unlink(&mut self, _device: &mut BlockDev) -> Result<i32, ()> {
        Ok(-1)
    }
}

pub fn make_pipe_nodes(nonblock: bool) -> (VfsNode, VfsNode) {
    let inner = Arc::new(Mutex::new(PipeInner::new()));
    let ino_r = NEXT_PIPE_INO.fetch_add(1, Ordering::Relaxed);
    let ino_w = NEXT_PIPE_INO.fetch_add(1, Ordering::Relaxed);

    let meta_r = Metadata {
        ino: ino_r,
        name: "pipe".into(),
        uid: 0,
        gid: 0,
        file_type: FileType::CharDevice,
        size: 0,
        created_time: 0,
        access_time: 0,
        modified_time: 0,
    };
    let meta_w = Metadata {
        ino: ino_w,
        name: "pipe".into(),
        file_type: FileType::CharDevice,
        uid: 0,
        gid: 0,
        size: 0,
        created_time: 0,
        access_time: 0,
        modified_time: 0,
    };

    let r_end: Arc<RwLock<dyn VfsNodeOps>> =
        Arc::new(RwLock::new(PipeEnd::new(inner.clone(), PipeEndKind::Read, nonblock)));
    let w_end: Arc<RwLock<dyn VfsNodeOps>> =
        Arc::new(RwLock::new(PipeEnd::new(inner.clone(), PipeEndKind::Write, nonblock)));

    let dev = dummy_blockdev();
    let r_node = VfsNode::new(dev.clone(), meta_r, r_end);
    let w_node = VfsNode::new(dev, meta_w, w_end);

    (r_node, w_node)
}
