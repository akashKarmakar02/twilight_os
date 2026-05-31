use crate::sys;

pub mod tcp;
pub mod udp;

use crate::driver::disk::ata::{FileIO, IO};
use crate::task::executor::sleep;
use alloc::vec;
use lazy_static::lazy_static;
use smoltcp::iface::SocketSet;
use smoltcp::time::Duration;
use smoltcp::wire::{IpAddress, IpEndpoint};
use spin::Mutex;

lazy_static! {
    pub static ref SOCKETS: Mutex<SocketSet<'static>> = Mutex::new(SocketSet::new(vec![]));
}

#[derive(Debug)]
pub enum SocketFile {
    Tcp(tcp::TcpSocket),
    Udp(udp::UdpSocket),
}

impl SocketFile {
    pub fn close(&mut self) {
        match self {
            SocketFile::Tcp(s) => s.close(),
            SocketFile::Udp(s) => s.close(),
        }
    }

    pub fn poll(&mut self, event: IO) -> bool {
        match self {
            SocketFile::Tcp(s) => s.poll(event),
            SocketFile::Udp(s) => s.poll(event),
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        match self {
            SocketFile::Tcp(s) => s.read(buf),
            SocketFile::Udp(s) => s.read(buf),
        }
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, ()> {
        match self {
            SocketFile::Tcp(s) => s.write(buf),
            SocketFile::Udp(s) => s.write(buf),
        }
    }

    pub fn connect(&mut self, addr: IpAddress, port: u16) -> Result<(), ()> {
        match self {
            SocketFile::Tcp(s) => s.connect(addr, port),
            SocketFile::Udp(s) => s.connect(addr, port),
        }
    }

    pub fn bind(&mut self, port: u16) -> Result<(), ()> {
        match self {
            SocketFile::Tcp(s) => s.bind(port),
            SocketFile::Udp(s) => s.bind(port),
        }
    }

    pub fn listen(&mut self, port: u16) -> Result<(), ()> {
        match self {
            SocketFile::Tcp(s) => s.listen(port),
            SocketFile::Udp(_) => Err(()),
        }
    }

    pub fn accept_new(&mut self) -> Result<(SocketFile, IpEndpoint), ()> {
        match self {
            SocketFile::Tcp(s) => {
                let (sock, ep) = s.accept_new()?;
                Ok((SocketFile::Tcp(sock), ep))
            }
            SocketFile::Udp(_) => Err(()),
        }
    }

    pub fn try_accept_new(&mut self) -> Result<Option<(SocketFile, IpEndpoint)>, ()> {
        match self {
            SocketFile::Tcp(s) => match s.try_accept_new()? {
                Some((sock, ep)) => Ok(Some((SocketFile::Tcp(sock), ep))),
                None => Ok(None),
            },
            SocketFile::Udp(_) => Err(()),
        }
    }

    pub fn send_to(&mut self, buf: &[u8], ep: IpEndpoint) -> Result<usize, ()> {
        match self {
            SocketFile::Tcp(s) => s.write(buf),
            SocketFile::Udp(s) => s.send_to(buf, ep),
        }
    }

    pub fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, IpEndpoint), ()> {
        match self {
            SocketFile::Tcp(_) => Err(()),
            SocketFile::Udp(s) => s.recv_from(buf),
        }
    }
}

fn random_port() -> u16 {
    49152 + sys::rng::get_u16() % 16384
}

fn wait(duration: Duration) {
    sleep((duration.total_micros() as f64) / 1000000.0);
}
