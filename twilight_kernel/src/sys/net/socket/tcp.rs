use crate::{println, sys};


use super::SOCKETS;
use super::random_port;

use crate::arch::x86_64::halt;
use crate::driver::disk::ata::{FileIO, IO};
use crate::driver::nic::{SocketStatus, NET};
use crate::driver::timer::cmos::CMOS;
use crate::task::executor::sleep;
use alloc::vec;
use bit_field::BitField;
use smoltcp::iface::SocketHandle;
use smoltcp::phy::Device;
use smoltcp::socket::Socket;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, IpEndpoint};

fn tcp_socket_status(socket: &tcp::Socket) -> u8 {
    let mut status = 0;
    status.set_bit(SocketStatus::IsListening as usize, socket.is_listening());
    status.set_bit(SocketStatus::IsActive as usize, socket.is_active());
    status.set_bit(SocketStatus::IsOpen as usize, socket.is_open());
    status.set_bit(SocketStatus::MaySend as usize, socket.may_send());
    status.set_bit(SocketStatus::CanSend as usize, socket.can_send());
    status.set_bit(SocketStatus::MayRecv as usize, socket.may_recv());
    status.set_bit(SocketStatus::CanRecv as usize, socket.can_recv());
    status
}

#[derive(Debug)]
pub struct TcpSocket {
    pub handle: SocketHandle,
    pub bound_port: Option<u16>,
    pub listen_port: Option<u16>,
}

impl TcpSocket {
    pub fn size() -> usize {
        if let Some((_, ref mut device)) = *NET.lock() {
            let mtu = device.capabilities().max_transmission_unit;
            let eth_header = 14;
            let ip_header = 20;
            let tcp_header = 20;
            mtu - eth_header - ip_header - tcp_header
        } else {
            1
        }
    }

    pub fn new() -> Self {
        let mut sockets = SOCKETS.lock();
        let tcp_rx_buffer = tcp::SocketBuffer::new(vec![0; 1024]);
        let tcp_tx_buffer = tcp::SocketBuffer::new(vec![0; 1024]);
        let tcp_socket = tcp::Socket::new(tcp_rx_buffer, tcp_tx_buffer);
        let handle = sockets.add(tcp_socket);

        Self {
            handle,
            bound_port: None,
            listen_port: None,
        }
    }

    pub fn bind(&mut self, port: u16) -> Result<(), ()> {
        self.bound_port = Some(port);
        Ok(())
    }

    pub fn connect(&mut self, addr: IpAddress, port: u16) -> Result<(), ()> {
        let mut connecting = false;

        let mut cmos = CMOS::new();
        let timeout = 5;
        let started = cmos.unix_time();
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            loop {
                if cmos.unix_time() - started > timeout {
                    return Err(());
                }
                let mut sockets = SOCKETS.lock();

                let ms = (cmos.unix_time() * 1000000) as i64;
                let time = Instant::from_micros(ms);
                iface.poll(time, device, &mut sockets);
                let socket = sockets.get_mut::<tcp::Socket>(self.handle);

                match socket.state() {
                    tcp::State::Closed => {
                        if connecting {
                            return Err(());
                        }
                        let cx = iface.context();
                        let dest = (addr, port);
                        let local_port = self.bound_port.unwrap_or_else(random_port);
                        if socket.connect(cx, dest, local_port).is_err() {
                            return Err(());
                        }
                        connecting = true;
                    }
                    tcp::State::SynSent => {}
                    tcp::State::Established => {
                        break;
                    }
                    _ => {
                        // Did something get sent before the connection closed?
                        return if socket.can_recv() {
                            Ok(())
                        } else {
                            println!("can't receive");
                            Err(())
                        };
                    }
                }

                if let Some(_d) = iface.poll_delay(sys::net::time(), &sockets) {
                    sleep(0.004);
                }
                halt();
            }
        }
        Ok(())
    }

    pub fn listen(&mut self, port: u16) -> Result<(), ()> {
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            iface.poll(sys::net::time(), device, &mut sockets);
            let socket = sockets.get_mut::<tcp::Socket>(self.handle);

            if socket.listen(port).is_err() {
                return Err(());
            }
            self.listen_port = Some(port);

            if let Some(_d) = iface.poll_delay(sys::net::time(), &sockets) {
                sleep(0.004);
            }
            halt();
            Ok(())
        } else {
            Err(())
        }
    }

    pub fn accept(&mut self) -> Result<IpAddress, ()> {
        let timeout = 5.0;
        let started = sys::clk::epoch_time();
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            loop {
                if sys::clk::epoch_time() - started > timeout {
                    return Err(());
                }
                let mut sockets = SOCKETS.lock();
                iface.poll(sys::net::time(), device, &mut sockets);
                let socket = sockets.get_mut::<tcp::Socket>(self.handle);

                if let Some(endpoint) = socket.remote_endpoint() {
                    return Ok(endpoint.addr);
                }

                if let Some(_d) = iface.poll_delay(sys::net::time(), &sockets) {
                    sleep(0.004);
                }
                halt();
            }
        } else {
            Err(())
        }
    }

    pub fn local_endpoint(&self) -> Option<IpEndpoint> {
        let sockets = SOCKETS.lock();
        let socket = sockets.get::<tcp::Socket>(self.handle);
        socket.local_endpoint()
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        let sockets = SOCKETS.lock();
        let socket = sockets.get::<tcp::Socket>(self.handle);
        socket.remote_endpoint()
    }

    /// Accept an incoming connection and return a new socket handle for the established
    /// connection while keeping this socket listening.
    pub fn accept_new(&mut self) -> Result<(TcpSocket, IpEndpoint), ()> {
        let listen_port = self.listen_port.ok_or(())?;

        let timeout = 5.0;
        let started = sys::clk::epoch_time();
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            loop {
                if sys::clk::epoch_time() - started > timeout {
                    return Err(());
                }
                let mut sockets = SOCKETS.lock();
                iface.poll(sys::net::time(), device, &mut sockets);
                let socket = sockets.get_mut::<tcp::Socket>(self.handle);

                let Some(endpoint) = socket.remote_endpoint() else {
                    if let Some(_d) = iface.poll_delay(sys::net::time(), &sockets) {
                        sleep(0.004);
                    }
                    halt();
                    continue;
                };

                // Move the established connection into a new socket.
                let removed = sockets.remove(self.handle);
                let Socket::Tcp(accepted) = removed else {
                    return Err(());
                };
                let accepted_handle = sockets.add(accepted);

                // Recreate the listener socket in-place.
                let tcp_rx_buffer = tcp::SocketBuffer::new(vec![0; 1024]);
                let tcp_tx_buffer = tcp::SocketBuffer::new(vec![0; 1024]);
                let listener = tcp::Socket::new(tcp_rx_buffer, tcp_tx_buffer);
                let listener_handle = sockets.add(listener);
                self.handle = listener_handle;
                self.bound_port = Some(listen_port);
                self.listen_port = Some(listen_port);

                // Start listening again.
                let listener_sock = sockets.get_mut::<tcp::Socket>(self.handle);
                let _ = listener_sock.listen(listen_port);

                return Ok((
                    TcpSocket {
                        handle: accepted_handle,
                        bound_port: None,
                        listen_port: None,
                    },
                    endpoint,
                ));
            }
        }
        Err(())
    }

    pub fn try_accept_new(&mut self) -> Result<Option<(TcpSocket, IpEndpoint)>, ()> {
        let listen_port = self.listen_port.ok_or(())?;

        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            iface.poll(sys::net::time(), device, &mut sockets);
            let socket = sockets.get_mut::<tcp::Socket>(self.handle);

            let Some(endpoint) = socket.remote_endpoint() else {
                return Ok(None);
            };

            // Move the established connection into a new socket.
            let removed = sockets.remove(self.handle);
            let Socket::Tcp(accepted) = removed else {
                return Err(());
            };
            let accepted_handle = sockets.add(accepted);

            // Recreate the listener socket in-place.
            let tcp_rx_buffer = tcp::SocketBuffer::new(vec![0; 1024]);
            let tcp_tx_buffer = tcp::SocketBuffer::new(vec![0; 1024]);
            let listener = tcp::Socket::new(tcp_rx_buffer, tcp_tx_buffer);
            let listener_handle = sockets.add(listener);
            self.handle = listener_handle;
            self.bound_port = Some(listen_port);
            self.listen_port = Some(listen_port);

            // Start listening again.
            let listener_sock = sockets.get_mut::<tcp::Socket>(self.handle);
            let _ = listener_sock.listen(listen_port);

            return Ok(Some((
                TcpSocket {
                    handle: accepted_handle,
                    bound_port: None,
                    listen_port: None,
                },
                endpoint,
            )));
        }

        Err(())
    }

}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        // Best-effort cleanup: when the last fd referencing this socket is dropped,
        // remove it from the global SocketSet to reclaim buffers.
        let mut sockets = SOCKETS.lock();
        let _ = sockets.remove(self.handle);
    }
}

impl FileIO for TcpSocket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        let timeout = 5.0;
        let started = sys::clk::epoch_time();
        let mut bytes = 0;
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            loop {
                if sys::clk::epoch_time() - started > timeout {
                    return Err(());
                }
                iface.poll(sys::net::time(), device, &mut sockets);
                let socket = sockets.get_mut::<tcp::Socket>(self.handle);

                if buf.len() == 1 {
                    // 1 byte status read
                    buf[0] = tcp_socket_status(socket);
                    return Ok(1);
                }

                if socket.can_recv() {
                    bytes = socket.recv_slice(buf).map_err(|_| ())?;
                    break;
                }
                if !socket.may_recv() {
                    break;
                }
                if let Some(_d) = iface.poll_delay(sys::net::time(), &sockets) {
                    sleep(0.004);
                }
                halt();
            }
            Ok(bytes)
        } else {
            Err(())
        }
    }

    fn write(&mut self, buf: &[u8]) -> Result<usize, ()> {
        let timeout = 5.0;
        let started = sys::clk::epoch_time();
        let mut written = 0;
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            loop {
                if sys::clk::epoch_time() - started > timeout {
                    return Err(());
                }
                iface.poll(sys::net::time(), device, &mut sockets);
                let socket = sockets.get_mut::<tcp::Socket>(self.handle);

                if written > 0 {
                    break;
                }
                if socket.can_send() {
                    match socket.send_slice(buf.as_ref()) {
                        Ok(n) => {
                            written = n;
                        }
                        Err(_) => return Err(()),
                    }
                }

                if let Some(_d) = iface.poll_delay(sys::net::time(), &sockets) {
                    sleep(0.004);
                }
                halt();
            }
            Ok(written)
        } else {
            Err(())
        }
    }

    fn close(&mut self) {
        let mut closed = false;
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            loop {
                iface.poll(sys::net::time(), device, &mut sockets);
                let socket = sockets.get_mut::<tcp::Socket>(self.handle);

                if closed {
                    break;
                }
                socket.close();
                closed = true;

                if let Some(_d) = iface.poll_delay(sys::net::time(), &sockets) {
                    sleep(0.004);
                }
                halt();
            }
        }
    }

    fn poll(&mut self, event: IO) -> bool {
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            iface.poll(sys::net::time(), device, &mut sockets);
            let socket = sockets.get_mut::<tcp::Socket>(self.handle);

            match event {
                IO::Read => socket.can_recv(),
                IO::Write => socket.can_send(),
            }
        } else {
            false
        }
    }
}
