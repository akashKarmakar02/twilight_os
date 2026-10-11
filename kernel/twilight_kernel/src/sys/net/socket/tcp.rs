//! TCP descriptor state. Operations make one nonblocking attempt; the syscall
//! layer owns sleeping and drops its OpenFile guard before doing so. The network
//! service owns packet processing, protocol timers and detached close state.

use super::{SOCKETS, random_port};
use crate::driver::disk::ata::{FileIO, IO};
use crate::driver::nic::NET;
use crate::sys::net::service;
use alloc::vec;
use smoltcp::iface::SocketHandle;
use smoltcp::phy::Device;
use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, IpEndpoint};
use twilight_common::syscall::types::{
    EAGAIN, EALREADY, ECONNREFUSED, EINVAL, EIO, EISCONN, EPIPE,
};

#[derive(Debug)]
pub struct TcpSocket {
    pub handle: SocketHandle,
    pub bound_port: Option<u16>,
    pub listen_port: Option<u16>,
}

fn buffers() -> tcp::Socket<'static> {
    tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0; 1024]),
        tcp::SocketBuffer::new(vec![0; 1024]),
    )
}

impl TcpSocket {
    /// Maximum payload size advertised by the attached Ethernet interface.
    pub fn size() -> usize {
        NET.lock().as_mut().map_or(1, |(_, device)| {
            device.capabilities().max_transmission_unit - 54
        })
    }

    /// Allocate protocol buffers before taking the socket-set lock.
    pub fn new() -> Self {
        let socket = buffers();
        let handle = SOCKETS.lock().add(socket);
        Self {
            handle,
            bound_port: None,
            listen_port: None,
        }
    }

    /// Remember the local port; listen/connect installs it in smoltcp.
    pub fn bind(&mut self, port: u16) -> Result<(), ()> {
        self.bound_port = Some(port);
        Ok(())
    }

    /// Start connection establishment without waiting under network locks.
    pub fn start_connect(&mut self, addr: IpAddress, port: u16) -> Result<(), i32> {
        let mut net = NET.lock();
        let (iface, _) = net.as_mut().ok_or(EIO)?;
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<tcp::Socket>(self.handle);
        if socket.state() == tcp::State::SynSent {
            return Err(EALREADY);
        }
        if socket.state() != tcp::State::Closed {
            return Err(EISCONN);
        }
        socket
            .connect(
                iface.context(),
                (addr, port),
                self.bound_port.unwrap_or_else(random_port),
            )
            .map_err(|_| EINVAL)?;
        drop(sockets);
        drop(net);
        service::request();
        Ok(())
    }

    /// Report completion of a previously started connection.
    pub fn connect_status(&self) -> Result<(), i32> {
        match SOCKETS.lock().get::<tcp::Socket>(self.handle).state() {
            tcp::State::Established | tcp::State::CloseWait => Ok(()),
            tcp::State::SynSent | tcp::State::SynReceived => Err(EAGAIN),
            _ => Err(ECONNREFUSED),
        }
    }

    /// Compatibility entry point; callers needing to block use net_io::retry.
    pub fn connect(&mut self, addr: IpAddress, port: u16) -> Result<(), ()> {
        self.start_connect(addr, port).map_err(|_| ())
    }

    /// Install a listener synchronously; listening itself never waits for input.
    pub fn listen(&mut self, port: u16) -> Result<(), ()> {
        SOCKETS
            .lock()
            .get_mut::<tcp::Socket>(self.handle)
            .listen(port)
            .map_err(|_| ())?;
        self.listen_port = Some(port);
        service::request();
        Ok(())
    }

    /// Return the bound endpoint without driving the network stack.
    pub fn local_endpoint(&self) -> Option<IpEndpoint> {
        SOCKETS
            .lock()
            .get::<tcp::Socket>(self.handle)
            .local_endpoint()
    }

    /// Return the peer endpoint without driving the network stack.
    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        SOCKETS
            .lock()
            .get::<tcp::Socket>(self.handle)
            .remote_endpoint()
    }

    /// Compatibility nonblocking accept; blocking is owned by the syscall layer.
    pub fn accept_new(&mut self) -> Result<(TcpSocket, IpEndpoint), ()> {
        self.try_accept_new()?.ok_or(())
    }

    /// Accept a completed handshake even when the peer has sent no payload.
    pub fn try_accept_new(&mut self) -> Result<Option<(TcpSocket, IpEndpoint)>, ()> {
        let port = self.listen_port.ok_or(())?;
        let endpoint = {
            let mut sockets = SOCKETS.lock();
            let socket = sockets.get_mut::<tcp::Socket>(self.handle);
            if socket.state() == tcp::State::Closed {
                socket.listen(port).map_err(|_| ())?;
            }
            if !matches!(
                socket.state(),
                tcp::State::Established | tcp::State::CloseWait
            ) {
                return Ok(None);
            }
            socket.remote_endpoint().ok_or(())?
        };
        // Keep the accepted socket in place and allocate just a new listener.
        let mut listener = buffers();
        listener.listen(port).map_err(|_| ())?;
        let listener_handle = SOCKETS.lock().add(listener);
        let accepted_handle = core::mem::replace(&mut self.handle, listener_handle);
        service::request();
        Ok(Some((
            Self {
                handle: accepted_handle,
                bound_port: None,
                listen_port: None,
            },
            endpoint,
        )))
    }

    /// Read currently buffered data, EOF, or EAGAIN; never halt in a syscall.
    pub fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, i32> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<tcp::Socket>(self.handle);
        if matches!(
            socket.state(),
            tcp::State::SynSent | tcp::State::SynReceived
        ) {
            return Err(EAGAIN);
        }
        let result = if socket.can_recv() {
            socket.recv_slice(buf).map_err(|_| EIO)
        } else if !socket.may_recv() {
            Ok(0)
        } else {
            Err(EAGAIN)
        };
        drop(sockets);
        if result.is_ok() {
            service::request();
        }
        result
    }

    /// Queue available bytes and immediately return the partial count.
    pub fn try_write(&mut self, buf: &[u8]) -> Result<usize, i32> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<tcp::Socket>(self.handle);
        if matches!(
            socket.state(),
            tcp::State::SynSent | tcp::State::SynReceived
        ) {
            return Err(EAGAIN);
        }
        let result = if socket.can_send() {
            socket.send_slice(buf).map_err(|_| EIO)
        } else if !socket.may_send() {
            Err(EPIPE)
        } else {
            Err(EAGAIN)
        };
        drop(sockets);
        if result.is_ok() {
            service::request();
        }
        result
    }
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        service::detach(self.handle);
    }
}

impl FileIO for TcpSocket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        self.try_read(buf).map_err(|_| ())
    }
    fn write(&mut self, buf: &[u8]) -> Result<usize, ()> {
        self.try_write(buf).map_err(|_| ())
    }
    fn close(&mut self) {
        SOCKETS.lock().get_mut::<tcp::Socket>(self.handle).close();
        service::request();
    }
    fn poll(&mut self, event: IO) -> bool {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<tcp::Socket>(self.handle);
        if let Some(port) = self.listen_port {
            if socket.state() == tcp::State::Closed {
                let _ = socket.listen(port);
            }
            return matches!(event, IO::Read)
                && matches!(
                    socket.state(),
                    tcp::State::Established | tcp::State::CloseWait
                );
        }
        if matches!(
            socket.state(),
            tcp::State::SynSent | tcp::State::SynReceived
        ) {
            return false;
        }
        match event {
            IO::Read => socket.can_recv() || !socket.may_recv(),
            IO::Write => socket.can_send() || !socket.may_send(),
        }
    }
}
