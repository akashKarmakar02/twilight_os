use crate::sys::proc;
use crate::utils::sync::{Mutex, WaitQueue};
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicU8, Ordering};
use lazy_static::lazy_static;
use twilight_common::syscall::types::{
    EADDRINUSE, EAGAIN, ECONNREFUSED, EDESTADDRREQ, EINTR, EINVAL, EISCONN, ENOTCONN, EOPNOTSUPP,
    EPIPE, SHUT_RD, SHUT_RDWR, SHUT_WR,
};

const UNIX_PATH_MAX: usize = 108;
const CHANNEL_CAPACITY: usize = 4096;

// ---- UnixAddr ----

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct UnixAddr {
    pub path: String,
}

impl UnixAddr {
    pub fn from_bytes(raw: &[u8]) -> Result<Self, i32> {
        if raw.is_empty() {
            return Err(EINVAL);
        }
        let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let len = len.min(UNIX_PATH_MAX);
        if len == 0 {
            return Err(EINVAL);
        }
        let path = String::from_utf8(raw[..len].to_vec()).map_err(|_| EINVAL)?;
        Ok(Self { path })
    }

    pub fn as_bytes(&self) -> Vec<u8> {
        let mut out = alloc::vec![0u8; UNIX_PATH_MAX];
        let bytes = self.path.as_bytes();
        let n = bytes.len().min(UNIX_PATH_MAX);
        out[..n].copy_from_slice(&bytes[..n]);
        out
    }

    pub fn addr_len(&self) -> u32 {
        let path_len = self.path.len().min(UNIX_PATH_MAX);
        (2 + path_len + 1) as u32
    }
}

impl fmt::Display for UnixAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.path)
    }
}

// ---- SockType ----

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SockType {
    Stream,
    Dgram,
}

// ---- Channel (unidirectional byte pipe) ----

struct Channel {
    buffer: VecDeque<u8>,
    readers: WaitQueue,
    writers: WaitQueue,
    read_closed: bool,
    write_closed: bool,
}

impl Channel {
    fn new() -> Self {
        Self {
            buffer: VecDeque::with_capacity(CHANNEL_CAPACITY),
            readers: WaitQueue::new(),
            writers: WaitQueue::new(),
            read_closed: false,
            write_closed: false,
        }
    }

    fn readable(&self) -> bool {
        !self.buffer.is_empty()
    }

    fn writable(&self) -> bool {
        !self.write_closed && self.buffer.len() < CHANNEL_CAPACITY
    }

    fn hangup(&self) -> bool {
        self.read_closed
    }

    fn error(&self) -> bool {
        self.write_closed
    }

    fn close_both(&mut self) {
        self.read_closed = true;
        self.write_closed = true;
        self.readers.notify_all();
        self.writers.notify_all();
        proc::poll_wait_queue().notify_all();
    }
}

// ---- UnixConnection (shared between two connected peers) ----

struct UnixConnection {
    channels: [Mutex<Channel>; 2],
    refcount: AtomicU8,
}

impl UnixConnection {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            channels: [Mutex::new(Channel::new()), Mutex::new(Channel::new())],
            refcount: AtomicU8::new(2),
        })
    }
}

// ---- UnixRole ----

#[derive(Clone, Debug, Eq, PartialEq)]
enum UnixRole {
    Unbound,
    Bound,
    Listening,
    Connected,
}

// ---- PendingAccept ----

struct PendingAccept {
    state: Arc<Mutex<UnixState>>,
}

// ---- DgramMessage ----

struct DgramMessage {
    data: Vec<u8>,
    src: UnixAddr,
}

// ---- UnixState ----

struct UnixState {
    role: UnixRole,
    sock_type: SockType,
    bound_addr: Option<UnixAddr>,
    conn: Option<Arc<UnixConnection>>,
    reader_half: usize,
    writer_half: usize,
    peer_addr: Option<UnixAddr>,

    backlog: VecDeque<PendingAccept>,
    accept_waiters: WaitQueue,

    dgram_queue: VecDeque<DgramMessage>,
    dgram_readers: WaitQueue,
    default_peer: Option<UnixAddr>,
}

impl UnixState {
    fn new(sock_type: SockType) -> Self {
        Self {
            role: UnixRole::Unbound,
            sock_type,
            bound_addr: None,
            conn: None,
            reader_half: 0,
            writer_half: 0,
            peer_addr: None,
            backlog: VecDeque::new(),
            accept_waiters: WaitQueue::new(),
            dgram_queue: VecDeque::new(),
            dgram_readers: WaitQueue::new(),
            default_peer: None,
        }
    }
}

// ---- Registry ----

lazy_static! {
    static ref UNIX_REGISTRY: Mutex<BTreeMap<String, Arc<Mutex<UnixState>>>> =
        Mutex::new(BTreeMap::new());
}

// ---- PollState ----

#[derive(Clone, Copy, Debug, Default)]
pub struct UnixPollState {
    pub readable: bool,
    pub writable: bool,
    pub hangup: bool,
    pub error: bool,
}

// ---- UnixSocket ----

#[derive(Clone)]
pub struct UnixSocket {
    state: Arc<Mutex<UnixState>>,
    pub addr_len: u32,
}

impl core::fmt::Debug for UnixSocket {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UnixSocket")
            .field("addr_len", &self.addr_len)
            .finish()
    }
}

impl UnixSocket {
    pub fn new(sock_type: SockType) -> Self {
        Self {
            state: Arc::new(Mutex::new(UnixState::new(sock_type))),
            addr_len: 0,
        }
    }

    pub fn bind(&mut self, addr: UnixAddr) -> Result<(), i32> {
        let path_str = addr.to_string();
        if path_str.is_empty() {
            return Err(EINVAL);
        }
        {
            let reg = UNIX_REGISTRY.lock();
            if reg.contains_key(&path_str) {
                return Err(EADDRINUSE);
            }
        }
        {
            let mut state = self.state.lock();
            state.role = UnixRole::Bound;
            state.bound_addr = Some(addr.clone());
        }
        self.addr_len = addr.addr_len();
        {
            let mut reg = UNIX_REGISTRY.lock();
            reg.insert(path_str, self.state.clone());
        }
        Ok(())
    }

    pub fn listen(&mut self, _backlog: i32) -> Result<(), i32> {
        let mut state = self.state.lock();
        if state.sock_type != SockType::Stream {
            return Err(EOPNOTSUPP);
        }
        if state.role != UnixRole::Bound {
            return Err(EINVAL);
        }
        state.role = UnixRole::Listening;
        Ok(())
    }

    pub fn connect(&mut self, addr: UnixAddr) -> Result<(), i32> {
        {
            let state = self.state.lock();
            if state.sock_type != SockType::Stream {
                return Err(EOPNOTSUPP);
            }
            if state.role == UnixRole::Connected {
                return Err(EISCONN);
            }
            if state.role != UnixRole::Unbound && state.role != UnixRole::Bound {
                return Err(EINVAL);
            }
        }

        let path_str = addr.to_string();
        let listener_state = {
            let reg = UNIX_REGISTRY.lock();
            reg.get(&path_str).cloned()
        };

        let listener_state = match listener_state {
            Some(s) => s,
            None => return Err(ECONNREFUSED),
        };

        let conn = UnixConnection::new();

        // Push accepted peer into listener's backlog
        // channels[0] = connecter→acceptee, channels[1] = acceptee→connecter
        {
            let mut listener = listener_state.lock();
            if listener.role != UnixRole::Listening {
                return Err(ECONNREFUSED);
            }
            let client_state = Arc::new(Mutex::new(UnixState {
                role: UnixRole::Connected,
                sock_type: SockType::Stream,
                bound_addr: None,
                conn: Some(conn.clone()),
                reader_half: 0, // acceptee reads what connecter writes (channels[0])
                writer_half: 1, // acceptee writes to channels[1]
                peer_addr: None,
                backlog: VecDeque::new(),
                accept_waiters: WaitQueue::new(),
                dgram_queue: VecDeque::new(),
                dgram_readers: WaitQueue::new(),
                default_peer: None,
            }));

            listener
                .backlog
                .push_back(PendingAccept { state: client_state });
            listener.accept_waiters.notify_all();
        }

        // Connecter uses the other half of the same connection
        self.addr_len = addr.addr_len();
        {
            let mut state = self.state.lock();
            state.conn = Some(conn);
            state.reader_half = 1; // connecter reads from channels[1] (what acceptee writes)
            state.writer_half = 0; // connecter writes to channels[0]
            state.peer_addr = Some(addr);
            state.role = UnixRole::Connected;
        }

        Ok(())
    }

    pub fn accept_new(&mut self) -> Result<(UnixSocket, UnixAddr), i32> {
        loop {
            {
                let mut state = self.state.lock();
                if state.sock_type != SockType::Stream {
                    return Err(EOPNOTSUPP);
                }
                if state.role != UnixRole::Listening {
                    return Err(EINVAL);
                }
                if let Some(pending) = state.backlog.pop_front() {
                    let peer;
                    let bound_addr = state.bound_addr.clone();
                    {
                        let mut accepted_state = pending.state.lock();
                        peer = accepted_state.peer_addr.clone().unwrap_or(UnixAddr {
                            path: String::new(),
                        });
                        accepted_state.bound_addr = bound_addr.clone();
                    }

                    let addr_len = bound_addr
                        .as_ref()
                        .map(|a| a.addr_len())
                        .unwrap_or(0);

                    return Ok((
                        UnixSocket {
                            state: pending.state.clone(),
                            addr_len,
                        },
                        peer,
                    ));
                }
                if proc::current_has_unblocked_signal() {
                    return Err(EINTR);
                }
                let pid = state.accept_waiters.prepare_current();
                drop(state);
                proc::await_io();
                {
                    let state = self.state.lock();
                    state.accept_waiters.finish_wait(pid);
                }
            }
        }
    }

    pub fn try_accept_new(&mut self) -> Result<Option<(UnixSocket, UnixAddr)>, i32> {
        let mut state = self.state.lock();
        if state.sock_type != SockType::Stream {
            return Err(EOPNOTSUPP);
        }
        if state.role != UnixRole::Listening {
            return Err(EINVAL);
        }
        if let Some(pending) = state.backlog.pop_front() {
            let peer;
            let bound_addr = state.bound_addr.clone();
            {
                let mut accepted_state = pending.state.lock();
                peer = accepted_state.peer_addr.clone().unwrap_or(UnixAddr {
                    path: String::new(),
                });
                accepted_state.bound_addr = bound_addr.clone();
            }

            let addr_len = bound_addr
                .as_ref()
                .map(|a| a.addr_len())
                .unwrap_or(0);

            Ok(Some((
                UnixSocket {
                    state: pending.state.clone(),
                    addr_len,
                },
                peer,
            )))
        } else {
            Ok(None)
        }
    }

    pub fn read(&mut self, out: &mut [u8], nonblock: bool) -> Result<usize, i32> {
        let is_dgram = {
            let state = self.state.lock();
            state.sock_type == SockType::Dgram
        };
        if is_dgram {
            self.dgram_recv_impl(out, nonblock)
        } else {
            self.stream_read_impl(out, nonblock)
        }
    }

    pub fn write(&mut self, data: &[u8], nonblock: bool) -> Result<usize, i32> {
        let is_dgram = {
            let state = self.state.lock();
            state.sock_type == SockType::Dgram
        };
        if is_dgram {
            self.dgram_send_impl(data, nonblock)
        } else {
            self.stream_write_impl(data, nonblock)
        }
    }

    fn stream_read_impl(&self, out: &mut [u8], nonblock: bool) -> Result<usize, i32> {
        let state = self.state.lock();
        if state.role != UnixRole::Connected {
            return Err(ENOTCONN);
        }
        let conn = state.conn.as_ref().ok_or(ENOTCONN)?.clone();
        let reader_idx = state.reader_half;
        drop(state);

        loop {
            let mut channel = conn.channels[reader_idx].lock();
            if !channel.buffer.is_empty() {
                let count = out.len().min(channel.buffer.len());
                for slot in &mut out[..count] {
                    *slot = channel.buffer.pop_front().unwrap();
                }
                channel.writers.notify_all();
                proc::poll_wait_queue().notify_all();
                return Ok(count);
            }
            if channel.read_closed {
                return Ok(0);
            }
            if nonblock {
                return Err(EAGAIN);
            }
            if proc::current_has_unblocked_signal() {
                return Err(EINTR);
            }
            let pid = channel.readers.prepare_current();
            drop(channel);
            proc::await_io();
            {
                let channel = conn.channels[reader_idx].lock();
                channel.readers.finish_wait(pid);
            }
        }
    }

    fn stream_write_impl(&self, data: &[u8], nonblock: bool) -> Result<usize, i32> {
        if data.is_empty() {
            return Ok(0);
        }
        let state = self.state.lock();
        if state.role != UnixRole::Connected {
            return Err(ENOTCONN);
        }
        let conn = state.conn.as_ref().ok_or(ENOTCONN)?.clone();
        let writer_idx = state.writer_half;
        drop(state);

        loop {
            let mut channel = conn.channels[writer_idx].lock();
            if channel.write_closed {
                return Err(EPIPE);
            }
            let available = CHANNEL_CAPACITY.saturating_sub(channel.buffer.len());
            let atomic_write = data.len() <= CHANNEL_CAPACITY;
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
                channel.buffer.extend(data[..count].iter().copied());
                channel.readers.notify_all();
                proc::poll_wait_queue().notify_all();
                return Ok(count);
            }
            if nonblock {
                return Err(EAGAIN);
            }
            if proc::current_has_unblocked_signal() {
                return Err(EINTR);
            }
            let pid = channel.writers.prepare_current();
            drop(channel);
            proc::await_io();
            {
                let channel = conn.channels[writer_idx].lock();
                channel.writers.finish_wait(pid);
            }
        }
    }

    fn dgram_send_impl(&mut self, data: &[u8], _nonblock: bool) -> Result<usize, i32> {
        let dest = {
            let state = self.state.lock();
            state.default_peer.clone()
        };
        match dest {
            Some(addr) => self.send_to(data, &addr),
            None => Err(EDESTADDRREQ),
        }
    }

    pub fn send_to(&mut self, buf: &[u8], addr: &UnixAddr) -> Result<usize, i32> {
        let target_state = {
            let reg = UNIX_REGISTRY.lock();
            reg.get(&addr.to_string()).cloned()
        };
        let target_state = target_state.ok_or(ECONNREFUSED)?;

        let mut target = target_state.lock();
        if target.sock_type != SockType::Dgram {
            return Err(EOPNOTSUPP);
        }
        let src = {
            let state = self.state.lock();
            state
                .bound_addr
                .clone()
                .unwrap_or(UnixAddr { path: String::new() })
        };
        target.dgram_queue.push_back(DgramMessage {
            data: buf.to_vec(),
            src,
        });
        target.dgram_readers.notify_all();
        proc::poll_wait_queue().notify_all();
        Ok(buf.len())
    }

    pub fn recv_from(&mut self, buf: &mut [u8], nonblock: bool) -> Result<(usize, UnixAddr), i32> {
        loop {
            let mut state = self.state.lock();
            if state.sock_type != SockType::Dgram {
                return Err(EOPNOTSUPP);
            }
            if let Some(msg) = state.dgram_queue.pop_front() {
                let count = buf.len().min(msg.data.len());
                buf[..count].copy_from_slice(&msg.data[..count]);
                let src = msg.src;
                return Ok((count, src));
            }
            if nonblock {
                return Err(EAGAIN);
            }
            if proc::current_has_unblocked_signal() {
                return Err(EINTR);
            }
            let pid = state.dgram_readers.prepare_current();
            drop(state);
            proc::await_io();
            {
                let state = self.state.lock();
                state.dgram_readers.finish_wait(pid);
            }
        }
    }

    fn dgram_recv_impl(&self, out: &mut [u8], nonblock: bool) -> Result<usize, i32> {
        loop {
            let mut state = self.state.lock();
            if let Some(msg) = state.dgram_queue.pop_front() {
                let count = out.len().min(msg.data.len());
                out[..count].copy_from_slice(&msg.data[..count]);
                return Ok(count);
            }
            if nonblock {
                return Err(EAGAIN);
            }
            if proc::current_has_unblocked_signal() {
                return Err(EINTR);
            }
            let pid = state.dgram_readers.prepare_current();
            drop(state);
            proc::await_io();
            {
                let s = self.state.lock();
                s.dgram_readers.finish_wait(pid);
            }
        }
    }

    pub fn close(&mut self) {
        let conn;
        {
            let mut state = self.state.lock();

            if let Some(bound) = state.bound_addr.take() {
                let mut reg = UNIX_REGISTRY.lock();
                reg.remove(&bound.to_string());
            }

            conn = state.conn.clone();
            state.role = UnixRole::Unbound;
            state.peer_addr = None;

            state.accept_waiters.notify_all();
            state.dgram_readers.notify_all();
        }

        if let Some(conn) = conn {
            let prev = conn.refcount.fetch_sub(1, Ordering::SeqCst);
            if prev <= 1 {
                for ch in &conn.channels {
                    ch.lock().close_both();
                }
            } else {
                let half;
                {
                    let state = self.state.lock();
                    half = (state.reader_half, state.writer_half);
                }
                conn.channels[half.1].lock().close_both();
                conn.channels[half.0].lock().close_both();
            }
        }
    }

    pub fn shutdown(&mut self, how: i32) {
        let state = self.state.lock();
        if state.sock_type != SockType::Stream {
            return;
        }
        let conn = match state.conn.as_ref() {
            Some(c) => c.clone(),
            None => return,
        };
        let writer_idx = state.writer_half;
        let reader_idx = state.reader_half;
        drop(state);

        match how {
            SHUT_RD => {
                conn.channels[reader_idx].lock().read_closed = true;
            }
            SHUT_WR => {
                let mut ch = conn.channels[writer_idx].lock();
                ch.write_closed = true;
                ch.readers.notify_all();
                proc::poll_wait_queue().notify_all();
            }
            SHUT_RDWR => {
                conn.channels[reader_idx].lock().read_closed = true;
                let mut ch = conn.channels[writer_idx].lock();
                ch.write_closed = true;
                ch.readers.notify_all();
                proc::poll_wait_queue().notify_all();
            }
            _ => {}
        }
    }

    pub fn poll(&self) -> UnixPollState {
        let state = self.state.lock();
        if state.sock_type == SockType::Dgram {
            let readable = !state.dgram_queue.is_empty();
            UnixPollState {
                readable,
                writable: true,
                ..UnixPollState::default()
            }
        } else if state.role == UnixRole::Listening {
            let has_conn = !state.backlog.is_empty();
            UnixPollState {
                readable: has_conn,
                ..UnixPollState::default()
            }
        } else if let Some(ref conn) = state.conn {
            let reader_idx = state.reader_half;
            let writer_idx = state.writer_half;
            let rch = conn.channels[reader_idx].lock();
            let wch = conn.channels[writer_idx].lock();
            UnixPollState {
                readable: rch.readable(),
                writable: wch.writable(),
                hangup: rch.hangup(),
                error: wch.error(),
            }
        } else {
            UnixPollState::default()
        }
    }

    pub fn local_endpoint(&self) -> Option<UnixAddr> {
        self.state.lock().bound_addr.clone()
    }

    pub fn remote_endpoint(&self) -> Option<UnixAddr> {
        self.state.lock().peer_addr.clone()
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        self.close();
    }
}
