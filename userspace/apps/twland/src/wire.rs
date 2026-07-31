//! Wayland wire-protocol layer for twland.
//!
//! This module owns the byte-level framing of the Wayland stream: message
//! headers, the argument codec, SCM_RIGHTS file-descriptor passing, and the
//! raw sendmsg/recvmsg syscalls.  Everything above this layer speaks in terms
//! of [`ReceivedMessage`] values and [`send_message`] / [`send_message_with_fds`]
//! calls and never touches a cmsghdr.
//!
//! The fd-passing path here is the fix for the `wl_keyboard.keymap` bug where
//! the fd argument was being serialized into the payload as a `u32`.  Wayland
//! fd arguments travel out-of-band via `SCM_RIGHTS` and occupy zero payload
//! bytes; [`send_message_with_fds`] is the only correct way to emit them.

use core::ffi::c_void;
use std::collections::VecDeque;
use std::io::{self, ErrorKind};
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::ptr;

/// Maximum bytes read from the socket in a single `recvmsg`.
const MAX_WIRE_CHUNK: usize = 64 * 1024;
/// Control-message buffer for received `SCM_RIGHTS` fds.
const MAX_CONTROL_BYTES: usize = 128;

const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
const MSG_DONTWAIT: i32 = 0x40;

#[repr(C)]
struct Iovec {
    iov_base: *mut c_void,
    iov_len: usize,
}

#[repr(C)]
struct Msghdr {
    msg_name: *mut c_void,
    msg_namelen: u32,
    msg_iov: *mut Iovec,
    msg_iovlen: i32,
    __pad_iovlen: i32,
    msg_control: *mut c_void,
    msg_controllen: u32,
    __pad_controllen: u32,
    msg_flags: i32,
}

/// `cmsghdr` with explicit padding so `cmsg_len` occupies a full `size_t` on
/// 64-bit, matching the kernel's `struct cmsghdr` layout.
#[repr(C)]
#[derive(Clone, Copy)]
struct Cmsghdr {
    cmsg_len: u32,
    __pad_len: i32,
    cmsg_level: i32,
    cmsg_type: i32,
}

unsafe extern "C" {
    fn sendmsg(fd: i32, msg: *const Msghdr, flags: i32) -> isize;
    fn recvmsg(fd: i32, msg: *mut Msghdr, flags: i32) -> isize;
    fn memfd_create(name: *const u8, flags: u32) -> i32;
    fn close(fd: i32) -> i32;
}

#[derive(Debug, Clone, Copy)]
pub struct WaylandHeader {
    pub object_id: u32,
    pub opcode: u16,
    pub size: u16,
}

/// One decoded Wayland message: header, payload bytes, and any fds received
/// alongside it via `SCM_RIGHTS`.
pub struct ReceivedMessage {
    pub header: WaylandHeader,
    pub payload: Vec<u8>,
    pub fds: Vec<OwnedFdRaw>,
}

/// An owned raw file descriptor that is closed on drop unless moved out with
/// [`OwnedFdRaw::into_raw`].
#[derive(Debug)]
pub struct OwnedFdRaw {
    fd: i32,
}

impl OwnedFdRaw {
    /// Wrap a raw open file descriptor.  The wrapper closes it on drop unless
    /// moved out with [`OwnedFdRaw::into_raw`].
    pub fn new(fd: i32) -> Self {
        Self { fd }
    }

    /// The underlying raw file descriptor.
    pub fn as_raw(&self) -> i32 {
        self.fd
    }

    pub fn into_raw(mut self) -> i32 {
        let fd = self.fd;
        self.fd = -1;
        fd
    }
}

impl Drop for OwnedFdRaw {
    fn drop(&mut self) {
        if self.fd >= 0 {
            // SAFETY: fd is owned by this wrapper unless moved out via into_raw,
            // in which case it is set to -1.
            let _ = unsafe { close(self.fd) };
            self.fd = -1;
        }
    }
}

/// Create an empty, read/write memfd wrapped in [`OwnedFdRaw`].  Used for
/// `wl_keyboard.keymap` with `WL_KEYBOARD_KEYMAP_FORMAT_NO_KEYMAP`, where the
/// protocol still carries a file descriptor even though there is no keymap
/// data.  The wrapper closes the local copy after the kernel duplicates the
/// fd into the client's socket buffer.
pub fn create_empty_memfd() -> io::Result<OwnedFdRaw> {
    const MFD_CLOEXEC: u32 = 0x0001;
    let name = b"twland-keymap\0";
    // SAFETY: `memfd_create` is provided by musl (wrapping Twilight syscall
    // 319).  `name` is a NUL-terminated static slice and `MFD_CLOEXEC` is an
    // allowed flag.
    let fd = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(OwnedFdRaw::new(fd))
    }
}

/// Send a Wayland event with no file descriptors.
pub fn send_message(
    stream: &mut UnixStream,
    object_id: u32,
    opcode: u16,
    payload: &[u8],
) -> io::Result<()> {
    let message = encode_message(object_id, opcode, payload)?;
    send_bytes(stream, &message, &[])
}

/// Send a Wayland event carrying file descriptors via `SCM_RIGHTS`.
///
/// The fds are attached to the first byte of the message and consumed by the
/// kernel on the first `sendmsg`; if the message is larger than what a single
/// `sendmsg` accepts, remaining bytes are flushed in further sends with no
/// control message.  This matches the Wayland stream contract: fd arguments
/// occupy zero payload bytes and are delivered with the message prefix.
pub fn send_message_with_fds(
    stream: &mut UnixStream,
    object_id: u32,
    opcode: u16,
    payload: &[u8],
    fds: &[i32],
) -> io::Result<()> {
    let message = encode_message(object_id, opcode, payload)?;
    send_bytes(stream, &message, fds)
}

fn encode_message(object_id: u32, opcode: u16, payload: &[u8]) -> io::Result<Vec<u8>> {
    let size = 8_usize
        .checked_add(payload.len())
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "message too large"))?;
    if size > u16::MAX as usize {
        return Err(io::Error::new(ErrorKind::InvalidInput, "message too large"));
    }

    let mut message = Vec::with_capacity(size);
    push_u32(&mut message, object_id);
    push_u32(&mut message, ((size as u32) << 16) | u32::from(opcode));
    message.extend_from_slice(payload);
    Ok(message)
}

fn send_bytes(stream: &mut UnixStream, message: &[u8], fds: &[i32]) -> io::Result<()> {
    let mut offset = 0;
    // Fds attach to the first byte and are consumed by the first successful
    // sendmsg; subsequent iterations send plain bytes.
    let mut pending_fds = fds;

    while offset < message.len() {
        let mut iov = Iovec {
            // sendmsg does not mutate the buffer, but musl's iovec field is a
            // mutable pointer in C.  Keep the cast local to this syscall wrapper.
            iov_base: message[offset..].as_ptr() as *mut c_void,
            iov_len: message.len() - offset,
        };

        let mut control = [0u8; MAX_CONTROL_BYTES];
        let (control_ptr, control_len) = if pending_fds.is_empty() {
            (ptr::null_mut(), 0)
        } else {
            let len = build_scm_rights(&mut control, pending_fds)?;
            (control.as_mut_ptr().cast(), len)
        };

        let hdr = Msghdr {
            msg_name: ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &mut iov,
            msg_iovlen: 1,
            __pad_iovlen: 0,
            msg_control: control_ptr,
            msg_controllen: control_len,
            __pad_controllen: 0,
            msg_flags: 0,
        };

        // SAFETY: `hdr` and its single iovec point at the immutable `message`
        // slice for the duration of the syscall.  The control buffer, when
        // present, is a valid stack array for the call's duration.  This is a
        // connected AF_UNIX stream, so msg_name and control-without-fds are null.
        let sent = unsafe { sendmsg(stream.as_raw_fd(), &hdr, 0) };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if sent == 0 {
            return Err(io::Error::new(
                ErrorKind::WriteZero,
                "sendmsg wrote zero bytes",
            ));
        }
        offset += sent as usize;
        pending_fds = &[];
    }

    Ok(())
}

/// Build a single `SCM_RIGHTS` control message carrying `fds` into `control`
/// and return the aligned length to set as `msg_controllen`.
fn build_scm_rights(control: &mut [u8; MAX_CONTROL_BYTES], fds: &[i32]) -> io::Result<u32> {
    let header_len = size_of::<Cmsghdr>();
    let data_len = size_of_val(fds);
    let cmsg_len = header_len + data_len;
    let aligned = cmsg_align(cmsg_len);
    if aligned > control.len() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "too many fds for control buffer",
        ));
    }

    let cmsg = Cmsghdr {
        cmsg_len: cmsg_len as u32,
        __pad_len: 0,
        cmsg_level: SOL_SOCKET,
        cmsg_type: SCM_RIGHTS,
    };
    // SAFETY: Cmsghdr is repr(C) and the header_len bytes fit in the buffer.
    unsafe { ptr::write_unaligned(control.as_mut_ptr().cast::<Cmsghdr>(), cmsg) };

    let data_start = header_len;
    for (i, fd) in fds.iter().enumerate() {
        let off = data_start + i * size_of::<i32>();
        control[off..off + 4].copy_from_slice(&fd.to_ne_bytes());
    }
    // Zero-fill trailing alignment padding so the kernel does not read garbage.
    for byte in &mut control[cmsg_len..aligned] {
        *byte = 0;
    }

    Ok(aligned as u32)
}

/// Read one chunk from the socket.  Returns `Ok(None)` on EOF, `Err(WouldBlock)`
/// when no data is ready, or `Ok(Some((bytes, fds)))` with the received bytes
/// and any `SCM_RIGHTS` fds.
pub fn recv_raw(stream: &mut UnixStream) -> io::Result<Option<(Vec<u8>, Vec<OwnedFdRaw>)>> {
    let mut data = vec![0u8; MAX_WIRE_CHUNK];
    let mut control = [0u8; MAX_CONTROL_BYTES];
    let mut iov = Iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: data.len(),
    };
    let mut msg = Msghdr {
        msg_name: ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        __pad_iovlen: 0,
        msg_control: control.as_mut_ptr().cast(),
        msg_controllen: control.len() as u32,
        __pad_controllen: 0,
        msg_flags: 0,
    };

    // SAFETY: `msg` points at valid writable iovec/control buffers for the
    // duration of the call, and the fd comes from a live UnixStream.
    let received = unsafe { recvmsg(stream.as_raw_fd(), &mut msg, MSG_DONTWAIT) };
    if received < 0 {
        return Err(io::Error::last_os_error());
    }
    if received == 0 {
        return Ok(None);
    }

    let fds = parse_received_fds(&control, msg.msg_controllen as usize);
    data.truncate(received as usize);
    Ok(Some((data, fds)))
}

/// Decode a received chunk into one or more queued messages.  All fds from the
/// chunk attach to the first message, matching how clients send fd-carrying
/// requests on the stream.
pub fn parse_chunk(
    bytes: &[u8],
    mut fds: Vec<OwnedFdRaw>,
    queue: &mut VecDeque<ReceivedMessage>,
) -> io::Result<()> {
    let mut offset = 0usize;
    let mut first = true;

    while offset < bytes.len() {
        if offset + 8 > bytes.len() {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "truncated Wayland header",
            ));
        }

        let header = parse_header(&bytes[offset..offset + 8]);
        if header.size < 8 {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("invalid message size {}", header.size),
            ));
        }

        let end = offset + usize::from(header.size);
        if end > bytes.len() {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "truncated Wayland payload",
            ));
        }

        let message_fds = if first {
            first = false;
            std::mem::take(&mut fds)
        } else {
            Vec::new()
        };
        queue.push_back(ReceivedMessage {
            header,
            payload: bytes[offset + 8..end].to_vec(),
            fds: message_fds,
        });
        offset = end;
    }

    Ok(())
}

fn parse_received_fds(control: &[u8], controllen: usize) -> Vec<OwnedFdRaw> {
    let mut fds = Vec::new();
    if controllen < size_of::<Cmsghdr>() || controllen > control.len() {
        return fds;
    }

    let mut offset = 0usize;
    while offset + size_of::<Cmsghdr>() <= controllen {
        // SAFETY: Bounds above guarantee enough bytes for an unaligned cmsghdr.
        let cmsg = unsafe { ptr::read_unaligned(control.as_ptr().add(offset).cast::<Cmsghdr>()) };
        let cmsg_len = cmsg.cmsg_len as usize;
        if cmsg_len < size_of::<Cmsghdr>() || offset + cmsg_len > controllen {
            break;
        }

        if cmsg.cmsg_level == SOL_SOCKET && cmsg.cmsg_type == SCM_RIGHTS {
            let data_start = offset + size_of::<Cmsghdr>();
            let data_len = cmsg_len - size_of::<Cmsghdr>();
            for chunk in control[data_start..data_start + data_len].chunks_exact(size_of::<i32>()) {
                let fd = i32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if fd >= 0 {
                    fds.push(OwnedFdRaw { fd });
                }
            }
        }

        let next = cmsg_align(cmsg_len);
        if next <= offset {
            break;
        }
        offset += next;
    }

    fds
}

fn parse_header(raw: &[u8]) -> WaylandHeader {
    let object_id = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let packed = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    WaylandHeader {
        object_id,
        opcode: (packed & 0xffff) as u16,
        size: (packed >> 16) as u16,
    }
}

// --- argument codec -------------------------------------------------------

pub fn read_u32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let raw = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| io::Error::new(ErrorKind::UnexpectedEof, "missing u32 argument"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

pub fn read_i32(bytes: &[u8], offset: usize) -> io::Result<i32> {
    Ok(read_u32(bytes, offset)? as i32)
}

pub fn read_wayland_string(bytes: &[u8], offset: usize) -> io::Result<(String, usize)> {
    let length = read_u32(bytes, offset)? as usize;
    if length == 0 {
        return Ok((String::new(), align4(offset + 4)));
    }

    let start = offset + 4;
    let end = start
        .checked_add(length)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "string length overflow"))?;
    let raw = bytes
        .get(start..end)
        .ok_or_else(|| io::Error::new(ErrorKind::UnexpectedEof, "truncated string argument"))?;
    if raw.last() != Some(&0) {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "string argument is not NUL-terminated",
        ));
    }

    let value = std::str::from_utf8(&raw[..raw.len() - 1])
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "string argument is not UTF-8"))?
        .to_string();
    Ok((value, align4(end)))
}

pub fn push_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub fn push_fixed(bytes: &mut Vec<u8>, value: i32) {
    push_i32(bytes, value.saturating_mul(256));
}

pub fn push_wayland_string(bytes: &mut Vec<u8>, value: &str) {
    let length = value.len() + 1;
    push_u32(bytes, length as u32);
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

fn align4(value: usize) -> usize {
    (value + 3) & !3
}

fn cmsg_align(value: usize) -> usize {
    align4_to(value, size_of::<usize>())
}

fn align4_to(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}
