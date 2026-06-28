use crate::sys;

pub mod tcp;
pub mod udp;
pub mod unix;

use crate::driver::disk::ata::{FileIO, IO};
use crate::task::executor::sleep;
use alloc::vec;
use lazy_static::lazy_static;
use smoltcp::iface::SocketSet;
use smoltcp::time::Duration;
use smoltcp::wire::{IpAddress, IpEndpoint};
use crate::utils::sync::Mutex;
use unix::UnixAddr;

lazy_static! {
    pub static ref SOCKETS: Mutex<SocketSet<'static>> = Mutex::new(SocketSet::new(vec![]));
}

#[derive(Debug)]
pub enum SocketFile {
    Tcp(tcp::TcpSocket),
    Udp(udp::UdpSocket),
    Unix(unix::UnixSocket),
}

impl SocketFile {
    pub fn close(&mut self) {
        match self {
            SocketFile::Tcp(s) => s.close(),
            SocketFile::Udp(s) => s.close(),
            SocketFile::Unix(s) => s.close(),
        }
    }

    pub fn poll(&mut self, event: IO) -> bool {
        match self {
            SocketFile::Tcp(s) => s.poll(event),
            SocketFile::Udp(s) => s.poll(event),
            SocketFile::Unix(s) => {
                let ps = s.poll();
                match event {
                    IO::Read => ps.readable,
                    IO::Write => ps.writable,
                }
            }
        }
    }

    pub fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        match self {
            SocketFile::Tcp(s) => s.read(buf),
            SocketFile::Udp(s) => s.read(buf),
            SocketFile::Unix(s) => s.read(buf, false).map_err(|_| ()),
        }
    }

    pub fn read_nonblock(&mut self, buf: &mut [u8], nonblock: bool) -> Result<usize, ()> {
        match self {
            SocketFile::Tcp(_) | SocketFile::Udp(_) => self.read(buf),
            SocketFile::Unix(s) => s.read(buf, nonblock).map_err(|_| ()),
        }
    }

    pub fn write(&mut self, buf: &[u8]) -> Result<usize, ()> {
        match self {
            SocketFile::Tcp(s) => s.write(buf),
            SocketFile::Udp(s) => s.write(buf),
            SocketFile::Unix(s) => s.write(buf, false).map_err(|_| ()),
        }
    }

    pub fn connect(&mut self, addr: IpAddress, port: u16) -> Result<(), ()> {
        match self {
            SocketFile::Tcp(s) => s.connect(addr, port),
            SocketFile::Udp(s) => s.connect(addr, port),
            SocketFile::Unix(_) => Err(()),
        }
    }

    pub fn bind(&mut self, port: u16) -> Result<(), ()> {
        match self {
            SocketFile::Tcp(s) => s.bind(port),
            SocketFile::Udp(s) => s.bind(port),
            SocketFile::Unix(_) => Err(()),
        }
    }

    pub fn listen(&mut self, port: u16) -> Result<(), ()> {
        match self {
            SocketFile::Tcp(s) => s.listen(port),
            SocketFile::Udp(_) => Err(()),
            SocketFile::Unix(_) => Err(()),
        }
    }

    pub fn accept_new(&mut self) -> Result<(SocketFile, IpEndpoint), ()> {
        match self {
            SocketFile::Tcp(s) => {
                let (sock, ep) = s.accept_new()?;
                Ok((SocketFile::Tcp(sock), ep))
            }
            SocketFile::Udp(_) => Err(()),
            SocketFile::Unix(_) => Err(()),
        }
    }

    pub fn try_accept_new(&mut self) -> Result<Option<(SocketFile, IpEndpoint)>, ()> {
        match self {
            SocketFile::Tcp(s) => match s.try_accept_new()? {
                Some((sock, ep)) => Ok(Some((SocketFile::Tcp(sock), ep))),
                None => Ok(None),
            },
            SocketFile::Udp(_) => Err(()),
            SocketFile::Unix(_) => Err(()),
        }
    }

    pub fn send_to(&mut self, buf: &[u8], ep: IpEndpoint) -> Result<usize, ()> {
        match self {
            SocketFile::Tcp(s) => s.write(buf),
            SocketFile::Udp(s) => s.send_to(buf, ep),
            SocketFile::Unix(_) => Err(()),
        }
    }

    pub fn recv_from(&mut self, buf: &mut [u8]) -> Result<(usize, IpEndpoint), ()> {
        match self {
            SocketFile::Tcp(_) => Err(()),
            SocketFile::Udp(s) => s.recv_from(buf),
            SocketFile::Unix(_) => Err(()),
        }
    }

    // ---- Unix-specific dispatch ----

    /// Connects a Unix socket to a peer address with peer credentials.
    ///
    /// # Examples
    ///
    /// ```
    /// let result = socket.connect_unix(addr, cred);
    /// assert!(result.is_ok() || result == Err(twilight_common::syscall::types::EOPNOTSUPP));
    /// ```
    ///
    /// @param addr The peer Unix address.
    /// @param cred The peer credentials to associate with the connection.
    /// @returns `Ok(())` if the socket connects, or `EOPNOTSUPP` for non-Unix sockets.
    pub fn connect_unix(&mut self, addr: UnixAddr, cred: unix::PeerCredentials) -> Result<(), i32> {
        match self {
            SocketFile::Unix(s) => s.connect(addr, cred),
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn bind_unix(&mut self, addr: UnixAddr) -> Result<(), i32> {
        match self {
            SocketFile::Unix(s) => s.bind(addr),
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn listen_unix(&mut self, backlog: i32) -> Result<(), i32> {
        match self {
            SocketFile::Unix(s) => s.listen(backlog),
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn accept_new_unix(&mut self) -> Result<(SocketFile, UnixAddr), i32> {
        match self {
            SocketFile::Unix(s) => {
                let (sock, addr) = s.accept_new()?;
                Ok((SocketFile::Unix(sock), addr))
            }
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn try_accept_new_unix(&mut self) -> Result<Option<(SocketFile, UnixAddr)>, i32> {
        match self {
            SocketFile::Unix(s) => match s.try_accept_new()? {
                Some((sock, addr)) => Ok(Some((SocketFile::Unix(sock), addr))),
                None => Ok(None),
            },
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn read_unix(&mut self, buf: &mut [u8], nonblock: bool) -> Result<usize, i32> {
        match self {
            SocketFile::Unix(s) => s.read(buf, nonblock),
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn write_unix(&mut self, buf: &[u8], nonblock: bool) -> Result<usize, i32> {
        match self {
            SocketFile::Unix(s) => s.write(buf, nonblock),
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn send_to_unix(&mut self, buf: &[u8], addr: &UnixAddr) -> Result<usize, i32> {
        match self {
            SocketFile::Unix(s) => s.send_to(buf, addr),
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn recv_from_unix(
        &mut self,
        buf: &mut [u8],
        nonblock: bool,
    ) -> Result<(usize, UnixAddr), i32> {
        match self {
            SocketFile::Unix(s) => match s.sock_type() {
                unix::SockType::Stream => {
                    let n = s.read(buf, nonblock)?;
                    let peer = s.remote_endpoint().unwrap_or(UnixAddr {
                        path: alloc::string::String::new(),
                    });
                    Ok((n, peer))
                }
                unix::SockType::Dgram => s.recv_from(buf, nonblock),
            },
            _ => Err(twilight_common::syscall::types::EOPNOTSUPP),
        }
    }

    pub fn shutdown_unix(&mut self, how: i32) {
        if let SocketFile::Unix(s) = self {
            s.shutdown(how);
        }
    }

    pub fn poll_unix(&self) -> unix::UnixPollState {
        match self {
            SocketFile::Unix(s) => s.poll(),
            _ => unix::UnixPollState::default(),
        }
    }

    pub fn local_endpoint_unix(&self) -> Option<UnixAddr> {
        match self {
            SocketFile::Unix(s) => s.local_endpoint(),
            _ => None,
        }
    }

    pub fn remote_endpoint_unix(&self) -> Option<UnixAddr> {
        match self {
            SocketFile::Unix(s) => s.remote_endpoint(),
            _ => None,
        }
    }

    /// Gets the stored length of the Unix socket address.
    ///
    /// # Returns
    ///
    /// The Unix socket address length, or `0` for non-Unix sockets.
    ///
    /// # Examples
    ///
    /// ```
    /// let len = socket.addr_len_unix();
    /// assert_eq!(len, 0);
    /// ```
    pub fn addr_len_unix(&self) -> u32 {
        match self {
            SocketFile::Unix(s) => s.addr_len,
            _ => 0,
        }
    }

    /// Returns the peer credentials for a Unix socket.
    ///
    /// # Examples
    ///
    /// ```
    /// let cred = socket_file.peer_cred_unix();
    /// ```
    pub fn peer_cred_unix(&self) -> Option<unix::PeerCredentials> {
        match self {
            SocketFile::Unix(s) => s.peer_cred,
            _ => None,
        }
    }
}

/// Generates a random ephemeral port number.
///
/// # Examples
///
/// ```
/// let port = random_port();
/// assert!((49152..=65535).contains(&port));
/// ```
fn random_port() -> u16 {
    49152 + sys::rng::get_u16() % 16384
}

fn wait(duration: Duration) {
    sleep((duration.total_micros() as f64) / 1000000.0);
}
