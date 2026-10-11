//! UDP descriptor state and single-attempt buffer operations. Blocking and
//! interruption belong to the syscall layer; the network service drives I/O.

use super::{SOCKETS, random_port};
use crate::driver::disk::ata::{FileIO, IO};
use crate::driver::nic::NET;
use crate::sys::net::service;
use alloc::vec;
use smoltcp::iface::SocketHandle;
use smoltcp::phy::Device;
use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint};
use twilight_common::syscall::types::{EAGAIN, EDESTADDRREQ, EIO, EMSGSIZE};

#[derive(Debug)]
pub struct UdpSocket {
    pub handle: SocketHandle,
    pub remote_endpoint: Option<IpEndpoint>,
    pub bound_port: Option<u16>,
}

impl UdpSocket {
    /// Maximum UDP payload size advertised by the attached interface.
    pub fn size() -> usize {
        NET.lock().as_mut().map_or(1, |(_, device)| {
            device.capabilities().max_transmission_unit - 42
        })
    }

    /// Allocate datagram buffers outside the socket-set critical section.
    pub fn new() -> Self {
        let rx = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY], vec![0; 1024]);
        let tx = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY], vec![0; 1024]);
        let handle = SOCKETS.lock().add(udp::Socket::new(rx, tx));
        Self {
            handle,
            remote_endpoint: None,
            bound_port: None,
        }
    }

    /// Bind an unbound datagram socket without waiting on packet processing.
    pub fn bind(&mut self, port: u16) -> Result<(), ()> {
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<udp::Socket>(self.handle);
        if !socket.is_open() {
            socket.bind(port).map_err(|_| ())?;
        }
        self.bound_port = Some(port);
        Ok(())
    }

    /// UDP connect only selects a default peer and binds a local port.
    pub fn connect(&mut self, addr: IpAddress, port: u16) -> Result<(), ()> {
        self.bind(self.bound_port.unwrap_or_else(random_port))?;
        self.remote_endpoint = Some(IpEndpoint::new(addr, port));
        Ok(())
    }

    /// Queue one whole datagram or return EAGAIN/EMSGSIZE.
    pub fn try_send_to(&mut self, buf: &[u8], endpoint: IpEndpoint) -> Result<usize, i32> {
        self.bind(self.bound_port.unwrap_or_else(random_port))
            .map_err(|_| EIO)?;
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<udp::Socket>(self.handle);
        if buf.len() > socket.payload_send_capacity() {
            return Err(EMSGSIZE);
        }
        if !socket.can_send() {
            return Err(EAGAIN);
        }
        socket.send_slice(buf, endpoint).map_err(|_| EIO)?;
        drop(sockets);
        service::request();
        Ok(buf.len())
    }

    /// Receive an available datagram, truncating to the user buffer like recvfrom.
    pub fn try_recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, IpEndpoint), i32> {
        self.bind(self.bound_port.unwrap_or_else(random_port))
            .map_err(|_| EIO)?;
        let mut sockets = SOCKETS.lock();
        let socket = sockets.get_mut::<udp::Socket>(self.handle);
        if !socket.can_recv() {
            return Err(EAGAIN);
        }
        let (data, meta) = socket.recv().map_err(|_| EIO)?;
        let count = data.len().min(buf.len());
        buf[..count].copy_from_slice(&data[..count]);
        let endpoint = meta.endpoint;
        drop(sockets);
        service::request();
        Ok((count, endpoint))
    }

    /// Queue a datagram to the connected peer.
    pub fn try_write(&mut self, buf: &[u8]) -> Result<usize, i32> {
        self.try_send_to(buf, self.remote_endpoint.ok_or(EDESTADDRREQ)?)
    }

    /// Compatibility nonblocking send; syscall callers retain errno information.
    pub fn send_to(&mut self, buf: &[u8], endpoint: IpEndpoint) -> Result<usize, ()> {
        self.try_send_to(buf, endpoint).map_err(|_| ())
    }

    /// Compatibility nonblocking receive.
    pub fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, IpEndpoint), ()> {
        self.try_recv_from(buf).map_err(|_| ())
    }

    /// Return the bound port without pumping the stack.
    pub fn local_port(&self) -> Option<u16> {
        let port = SOCKETS
            .lock()
            .get::<udp::Socket>(self.handle)
            .endpoint()
            .port;
        (port != 0).then_some(port)
    }

    /// Return the default datagram peer.
    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        self.remote_endpoint
    }
}

impl FileIO for UdpSocket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        self.recv_from(buf).map(|(count, _)| count)
    }
    fn write(&mut self, buf: &[u8]) -> Result<usize, ()> {
        self.try_write(buf).map_err(|_| ())
    }
    fn close(&mut self) {
        SOCKETS.lock().get_mut::<udp::Socket>(self.handle).close();
    }
    fn poll(&mut self, event: IO) -> bool {
        let sockets = SOCKETS.lock();
        let socket = sockets.get::<udp::Socket>(self.handle);
        match event {
            IO::Read => socket.can_recv(),
            IO::Write => socket.can_send(),
        }
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        SOCKETS.lock().remove(self.handle);
    }
}
