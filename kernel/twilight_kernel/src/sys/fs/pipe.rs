use crate::sys::proc;
use crate::utils::sync::WaitQueue;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::mutex::Mutex;
use twilight_common::syscall::types::{EAGAIN, EINTR, EPIPE};

pub const PIPE_BUF: usize = 4096;
const PIPE_CAPACITY: usize = PIPE_BUF;
static NEXT_PIPE_ID: AtomicU32 = AtomicU32::new(10_000);

struct PipeInner {
    buffer: VecDeque<u8>,
    readers: usize,
    writers: usize,
}

impl PipeInner {
    fn new() -> Self {
        Self {
            buffer: VecDeque::with_capacity(PIPE_CAPACITY),
            readers: 1,
            writers: 1,
        }
    }
}

struct PipeState {
    id: u32,
    inner: Mutex<PipeInner>,
    readers: WaitQueue,
    writers: WaitQueue,
}

impl PipeState {
    fn new() -> Self {
        Self {
            id: NEXT_PIPE_ID.fetch_add(1, Ordering::Relaxed),
            inner: Mutex::new(PipeInner::new()),
            readers: WaitQueue::new(),
            writers: WaitQueue::new(),
        }
    }

    fn notify_state_change(&self) {
        self.readers.notify_all();
        self.writers.notify_all();
        proc::poll_wait_queue().notify_all();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipeEndKind {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PipePollState {
    pub readable: bool,
    pub writable: bool,
    pub hangup: bool,
    pub error: bool,
}

pub struct PipeEnd {
    state: Arc<PipeState>,
    kind: PipeEndKind,
}

impl PipeEnd {
    fn new(state: Arc<PipeState>, kind: PipeEndKind) -> Self {
        Self { state, kind }
    }

    pub fn read(&self, out: &mut [u8], nonblock: bool) -> Result<usize, i32> {
        if self.kind != PipeEndKind::Read {
            return Err(EPIPE);
        }
        if out.is_empty() {
            return Ok(0);
        }

        loop {
            let mut inner = self.state.inner.lock();

            if !inner.buffer.is_empty() {
                let count = out.len().min(inner.buffer.len());
                for slot in &mut out[..count] {
                    *slot = inner.buffer.pop_front().expect("pipe length checked");
                }
                drop(inner);
                self.state.writers.notify_all();
                proc::poll_wait_queue().notify_all();
                return Ok(count);
            }

            if inner.writers == 0 {
                return Ok(0);
            }
            if nonblock {
                return Err(EAGAIN);
            }
            if proc::current_has_unblocked_signal() {
                return Err(EINTR);
            }

            let pid = self.state.readers.prepare_current();
            drop(inner);
            proc::await_io();
            self.state.readers.finish_wait(pid);

            if proc::current_has_unblocked_signal() {
                return Err(EINTR);
            }
        }
    }

    pub fn write(&self, data: &[u8], nonblock: bool) -> Result<usize, i32> {
        if self.kind != PipeEndKind::Write {
            return Err(EPIPE);
        }
        if data.is_empty() {
            return Ok(0);
        }

        loop {
            let mut inner = self.state.inner.lock();

            if inner.readers == 0 {
                return Err(EPIPE);
            }

            let available = PIPE_CAPACITY.saturating_sub(inner.buffer.len());
            let atomic_write = data.len() <= PIPE_BUF;
            let can_write = if atomic_write {
                available >= data.len()
            } else {
                available > 0
            };

            if can_write {
                let count = if atomic_write {
                    data.len()
                } else {
                    available.min(data.len())
                };
                inner.buffer.extend(data[..count].iter().copied());
                drop(inner);
                self.state.readers.notify_all();
                proc::poll_wait_queue().notify_all();
                return Ok(count);
            }

            if nonblock {
                return Err(EAGAIN);
            }
            if proc::current_has_unblocked_signal() {
                return Err(EINTR);
            }

            let pid = self.state.writers.prepare_current();
            drop(inner);
            proc::await_io();
            self.state.writers.finish_wait(pid);

            if proc::current_has_unblocked_signal() {
                return Err(EINTR);
            }
        }
    }

    pub fn poll(&self) -> PipePollState {
        let inner = self.state.inner.lock();
        match self.kind {
            PipeEndKind::Read => PipePollState {
                readable: !inner.buffer.is_empty(),
                hangup: inner.writers == 0,
                ..PipePollState::default()
            },
            PipeEndKind::Write => PipePollState {
                writable: inner.readers > 0 && inner.buffer.len() < PIPE_CAPACITY,
                error: inner.readers == 0,
                ..PipePollState::default()
            },
        }
    }

    pub fn id(&self) -> u32 {
        self.state.id
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        {
            let mut inner = self.state.inner.lock();
            match self.kind {
                PipeEndKind::Read => inner.readers = inner.readers.saturating_sub(1),
                PipeEndKind::Write => inner.writers = inner.writers.saturating_sub(1),
            }
        }
        self.state.notify_state_change();
    }
}

pub fn make_pipe_ends() -> (PipeEnd, PipeEnd) {
    let state = Arc::new(PipeState::new());
    (
        PipeEnd::new(state.clone(), PipeEndKind::Read),
        PipeEnd::new(state, PipeEndKind::Write),
    )
}
