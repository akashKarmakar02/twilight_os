use crate::sys;

use super::SOCKETS;
use super::{random_port, wait};

use crate::arch::x86_64::halt;
use crate::driver::disk::ata::{FileIO, IO};
use crate::driver::nic::NET;
use crate::driver::timer::cmos::CMOS;
use crate::task::executor::sleep;
use alloc::vec;
use smoltcp::iface::SocketHandle;
use smoltcp::phy::Device;
use smoltcp::socket::udp;
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, IpEndpoint, IpListenEndpoint};

#[derive(Debug)]
pub struct UdpSocket {
    pub handle: SocketHandle,
    pub remote_endpoint: Option<IpEndpoint>,
    pub bound_port: Option<u16>,
}

impl UdpSocket {
    pub fn size() -> usize {
        if let Some((_, ref mut device)) = *NET.lock() {
            let mtu = device.capabilities().max_transmission_unit;
            let eth_header = 14;
            let ip_header = 20;
            let udp_header = 8;
            mtu - eth_header - ip_header - udp_header
        } else {
            1
        }
    }

    pub fn new() -> Self {
        let mut sockets = SOCKETS.lock();
        let udp_rx_buffer = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY], vec![0; 1024]);
        let udp_tx_buffer = udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY], vec![0; 1024]);
        let udp_socket = udp::Socket::new(udp_rx_buffer, udp_tx_buffer);
        let handle = sockets.add(udp_socket);
        let remote_endpoint = None;

        Self {
            handle,
            remote_endpoint,
            bound_port: None,
        }
    }

    pub fn bind(&mut self, port: u16) -> Result<(), ()> {
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            iface.poll(sys::net::time(), device, &mut sockets);
            let socket = sockets.get_mut::<udp::Socket>(self.handle);

            if !socket.is_open() {
                let local_endpoint = IpListenEndpoint::from(port);
                socket.bind(local_endpoint).map_err(|_| ())?;
            }
            self.bound_port = Some(port);
            Ok(())
        } else {
            Err(())
        }
    }

    pub fn connect(&mut self, addr: IpAddress, port: u16) -> Result<(), ()> {
        let timeout = 5.0;
        let started = sys::clk::epoch_time();
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            loop {
                if sys::clk::epoch_time() - started > timeout {
                    return Err(());
                }
                let mut sockets = SOCKETS.lock();
                iface.poll(sys::net::time(), device, &mut sockets);
                let socket = sockets.get_mut::<udp::Socket>(self.handle);

                if !socket.is_open() {
                    let local_endpoint =
                        IpListenEndpoint::from(self.bound_port.unwrap_or_else(random_port));
                    socket.bind(local_endpoint).unwrap();
                    break;
                }

                if let Some(d) = iface.poll_delay(sys::net::time(), &sockets) {
                    wait(d);
                }
                halt();
            }
        }
        self.remote_endpoint = Some(IpEndpoint::new(addr, port));
        Ok(())
    }

    pub fn send_to(&mut self, buf: &[u8], endpoint: IpEndpoint) -> Result<usize, ()> {
        let mut cmos = CMOS::new();
        let timeout = 5;
        let started = cmos.unix_time();

        let mut sent = false;
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            loop {
                if cmos.unix_time() - started > timeout {
                    return Err(());
                }
                let ms = (cmos.unix_time() * 1000000) as i64;
                let time = Instant::from_micros(ms);
                iface.poll(time, device, &mut sockets);
                let socket = sockets.get_mut::<udp::Socket>(self.handle);

                if sent {
                    break;
                }
                if !socket.is_open() {
                    let local_endpoint =
                        IpListenEndpoint::from(self.bound_port.unwrap_or_else(random_port));
                    socket.bind(local_endpoint).map_err(|_| ())?;
                }
                if socket.can_send() {
                    socket.send_slice(buf, endpoint).map_err(|_| ())?;
                    sent = true;
                }

                if let Some(d) = iface.poll_delay(sys::net::time(), &sockets) {
                    wait(d);
                }
                halt();
            }
            Ok(buf.len())
        } else {
            Err(())
        }
    }

    pub fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, IpEndpoint), ()> {
        let mut cmos = CMOS::new();
        let timeout = 5;
        let started = cmos.unix_time();

        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            loop {
                if cmos.unix_time() - started > timeout {
                    return Err(());
                }
                let ms = (cmos.unix_time() * 1000000) as i64;
                let time = Instant::from_micros(ms);

                iface.poll(time, device, &mut sockets);
                let socket = sockets.get_mut::<udp::Socket>(self.handle);

                if !socket.is_open() {
                    let local_endpoint =
                        IpListenEndpoint::from(self.bound_port.unwrap_or_else(random_port));
                    socket.bind(local_endpoint).map_err(|_| ())?;
                }

                if socket.can_recv() {
                    let (n, meta) = socket.recv_slice(buf).map_err(|_| ())?;
                    return Ok((n, meta.endpoint));
                }
                let pd = sys::net::time();
                if let Some(_d) = iface.poll_delay(pd.clone(), &sockets) {
                    sleep(0.004);
                }
                halt();
            }
        } else {
            Err(())
        }
    }

    pub fn listen(&mut self, _port: u16) -> Result<(), ()> {
        todo!()
    }

    pub fn accept(&mut self) -> Result<IpAddress, ()> {
        todo!()
    }

    pub fn local_port(&self) -> Option<u16> {
        let sockets = SOCKETS.lock();
        let socket = sockets.get::<udp::Socket>(self.handle);
        let ep = socket.endpoint();
        if ep.port == 0 { None } else { Some(ep.port) }
    }

    pub fn remote_endpoint(&self) -> Option<IpEndpoint> {
        self.remote_endpoint
    }
}

impl FileIO for UdpSocket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        let mut cmos = CMOS::new();
        let timeout = 5;
        let started = cmos.unix_time();

        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let bytes;
            let mut sockets = SOCKETS.lock();
            loop {
                if cmos.unix_time() - started > timeout {
                    return Err(());
                }
                let ms = (cmos.unix_time() * 1000000) as i64;
                let time = Instant::from_micros(ms);

                iface.poll(time, device, &mut sockets);
                let socket = sockets.get_mut::<udp::Socket>(self.handle);

                if socket.can_recv() {
                    (bytes, _) = socket.recv_slice(buf).map_err(|_| ())?;
                    break;
                }
                let pd = sys::net::time();
                if let Some(_d) = iface.poll_delay(pd.clone(), &sockets) {
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
        let mut cmos = CMOS::new();
        let timeout = 5;
        let started = cmos.unix_time();

        let mut sent = false;
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            loop {
                if cmos.unix_time() - started > timeout {
                    return Err(());
                }
                let ms = (cmos.unix_time() * 1000000) as i64;
                let time = Instant::from_micros(ms);
                iface.poll(time, device, &mut sockets);
                let socket = sockets.get_mut::<udp::Socket>(self.handle);

                if sent {
                    break;
                }
                if socket.can_send() {
                    let endpoint = self.remote_endpoint.ok_or(())?;
                    socket.send_slice(buf.as_ref(), endpoint).map_err(|_| ())?;
                    sent = true; // Break after next poll
                }

                if let Some(d) = iface.poll_delay(sys::net::time(), &sockets) {
                    wait(d);
                }
                halt();
            }
            Ok(buf.len())
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
                let socket = sockets.get_mut::<udp::Socket>(self.handle);

                if closed {
                    break;
                }
                socket.close();
                closed = true;

                if let Some(d) = iface.poll_delay(sys::net::time(), &sockets) {
                    wait(d);
                }
                halt();
            }
        }
    }

    fn poll(&mut self, event: IO) -> bool {
        if let Some((ref mut iface, ref mut device)) = *NET.lock() {
            let mut sockets = SOCKETS.lock();
            iface.poll(sys::net::time(), device, &mut sockets);
            let socket = sockets.get_mut::<udp::Socket>(self.handle);

            match event {
                IO::Read => socket.can_recv(),
                IO::Write => socket.can_send(),
            }
        } else {
            false
        }
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        let mut sockets = SOCKETS.lock();
        let _ = sockets.remove(self.handle);
    }
}
