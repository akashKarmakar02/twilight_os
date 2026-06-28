//! IPC primitives for twinit (SCM_RIGHTS, SO_PEERCRED)
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::ptr;

use crate::os;

pub fn get_peercred(fd: RawFd) -> io::Result<os::Ucred> {
    let mut cred = os::Ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<os::Ucred>() as u32;

    // SAFETY: cred points to a valid struct, len is initialized to its size.
    let ret = unsafe {
        os::getsockopt(
            fd,
            os::SOL_SOCKET,
            os::SO_PEERCRED,
            &mut cred as *mut _ as *mut _,
            &mut len,
        )
    };

    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(cred)
    }
}

pub fn create_socketpair() -> io::Result<(UnixStream, UnixStream)> {
    let mut fds = [-1, -1];
    // SAFETY: fds is a valid array of 2 integers.
    let ret = unsafe { os::socketpair(os::AF_UNIX, os::SOCK_STREAM, 0, fds.as_mut_ptr()) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: socketpair succeeded, the returned FDs are valid and owned.
    let s1 = unsafe { UnixStream::from_raw_fd(fds[0]) };
    let s2 = unsafe { UnixStream::from_raw_fd(fds[1]) };
    Ok((s1, s2))
}

pub fn send_fd(stream: &UnixStream, message: &str, fd: RawFd) -> io::Result<()> {
    let mut iov = os::Iovec {
        iov_base: message.as_ptr() as *mut _,
        iov_len: message.len(),
    };

    #[repr(C)]
    struct CmsgBuffer {
        hdr: os::Cmsghdr,
        fd: RawFd,
    }

    let mut cmsg_buf = CmsgBuffer {
        hdr: os::Cmsghdr {
            cmsg_len: std::mem::size_of::<CmsgBuffer>() as u32,
            __pad_len: 0,
            cmsg_level: os::SOL_SOCKET,
            cmsg_type: os::SCM_RIGHTS,
        },
        fd,
    };

    let msg = os::Msghdr {
        msg_name: ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        __pad_iovlen: 0,
        msg_control: &mut cmsg_buf as *mut _ as *mut _,
        msg_controllen: std::mem::size_of::<CmsgBuffer>() as u32,
        __pad_controllen: 0,
        msg_flags: 0,
    };

    // SAFETY: msg is properly formed. message buffer is valid.
    let sent = unsafe { os::sendmsg(stream.as_raw_fd(), &msg, 0) };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

pub fn recv_fd(stream: &UnixStream) -> io::Result<(String, Option<RawFd>)> {
    let mut buf = [0u8; 1024];
    let mut iov = os::Iovec {
        iov_base: buf.as_mut_ptr() as *mut _,
        iov_len: buf.len(),
    };

    #[repr(C)]
    struct CmsgBuffer {
        hdr: os::Cmsghdr,
        fd: RawFd,
    }

    // Initialize the control buffer with 0 to ensure padding and fields are clean
    let mut cmsg_buf: CmsgBuffer = unsafe { std::mem::zeroed() };

    let mut msg = os::Msghdr {
        msg_name: ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        __pad_iovlen: 0,
        msg_control: &mut cmsg_buf as *mut _ as *mut _,
        msg_controllen: std::mem::size_of::<CmsgBuffer>() as u32,
        __pad_controllen: 0,
        msg_flags: 0,
    };

    // SAFETY: msg is properly formed. buf is valid.
    let received = unsafe { os::recvmsg(stream.as_raw_fd(), &mut msg, 0) };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    if received == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "EOF"));
    }

    let payload = String::from_utf8_lossy(&buf[..received as usize]).to_string();

    let mut passed_fd = None;
    if msg.msg_controllen >= std::mem::size_of::<os::Cmsghdr>() as u32 {
        if cmsg_buf.hdr.cmsg_level == os::SOL_SOCKET && cmsg_buf.hdr.cmsg_type == os::SCM_RIGHTS {
            passed_fd = Some(cmsg_buf.fd);
        }
    }

    Ok((payload, passed_fd))
}
