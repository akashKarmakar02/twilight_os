use crate::arch::x86_64::io::{IA32_FS_BASE, rdmsr, wrmsr};
use crate::driver::disk::ata::IO;
use crate::driver::timer::pit::uptime;
use crate::sys::fs::pipe::make_pipe_ends;
use crate::sys::fs::vfs::{FileType, VFS};
use crate::sys::kmsg::IOCTL_KMSG_GET_HEAD;
use crate::sys::net::bind_map::GLOBAL_PORT_MAP;
use crate::sys::net::socket::{SocketFile, tcp::TcpSocket, udp::UdpSocket};
use crate::sys::proc::{
    FdEntry, OpenFile, OpenFileKind, PROCESS_TABLE, Process, SIGCHLD, SIGKILL, SIGPIPE, SIGSTOP,
    SignalAction, SignalAltStack, USER_STACK_SIZE, signal_bit,
};
use crate::sys::syscall::fs_attr::IFLAG_ENCRYPTED;
use crate::sys::syscall::utils::{UserPtr, copy_cstr_from_user, copy_user_ptr_array, format_path};
use crate::task::executor::halt;
use crate::{logger, sys};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::{format, vec};
use core::arch::asm;
use core::mem::size_of;
use core::sync::atomic::{AtomicU64, Ordering};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use spin::mutex::Mutex;
use twilight_common::syscall::types::*;

fn join_paths(base: &str, rel: &str) -> String {
    if rel.is_empty() || rel == "." {
        return base.to_string();
    }
    if rel.starts_with('/') {
        return rel.to_string();
    }
    if base == "/" {
        format!("/{}", rel.trim_start_matches('/'))
    } else {
        format!("{}/{}", base.trim_end_matches('/'), rel)
    }
}

#[inline(always)]
fn parent_path(path: &str) -> &str {
    // Remove trailing slash (except root)
    let path = if path != "/" && path.ends_with('/') {
        &path[..path.len() - 1]
    } else {
        path
    };

    // Find the last '/'
    match path.rfind('/') {
        Some(0) => "/", // parent of "/foo" is "/"
        Some(idx) => &path[..idx],
        None => ".", // no slash → current directory
    }
}

fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            out.pop();
        } else {
            out.push(seg);
        }
    }
    if out.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", out.join("/"))
    }
}

fn home_root_for_path(path: &str) -> Option<String> {
    if !path.starts_with("/home/") {
        return None;
    }
    let rest = &path["/home/".len()..];
    let user = rest.split('/').next().unwrap_or("");
    if user.is_empty() {
        return None;
    }
    Some(format!("/home/{}", user))
}

fn check_encrypted_home_access(path: &str) -> Result<(), i64> {
    let Some(home_root) = home_root_for_path(path) else {
        return Ok(());
    };

    #[allow(static_mut_refs)]
    let encrypted = match unsafe { VFS.get_mut().get_attr(&home_root, IFLAG_ENCRYPTED) } {
        Ok(v) => (v & IFLAG_ENCRYPTED) != 0,
        Err(_) => false,
    };
    if !encrypted {
        return Ok(());
    }

    let current_uid = sys::proc::user::get_uid() as u32;
    if current_uid == 0 {
        return Ok(());
    }

    #[allow(static_mut_refs)]
    let home_meta = match unsafe { VFS.get_mut().metadata(&home_root) } {
        Ok(meta) => meta,
        Err(_) => return Ok(()),
    };
    if home_meta.uid == current_uid {
        Ok(())
    } else {
        Err(-(EACCES as i64))
    }
}

#[inline(always)]
fn fill_stat_from_meta(out: &mut Stat, meta: &sys::fs::vfs::Metadata) {
    out.st_size = meta.size as i64;
    out.st_mode = meta.mode as u32;
    out.st_uid = meta.uid;
    out.st_gid = meta.gid;
    out.st_ino = meta.ino as u64;
    out.st_nlink = 1;
    out.st_rdev = 0;
    out.st_atim = Timespec {
        tv_sec: meta.access_time as i64,
        tv_nsec: 0,
    };
    out.st_ctim = Timespec {
        tv_sec: meta.created_time as i64,
        tv_nsec: 0,
    };
    out.st_mtim = Timespec {
        tv_sec: meta.modified_time as i64,
        tv_nsec: 0,
    };
}

fn fill_statfs(out: &mut StatFs, stats: sys::fs::vfs::FsStats) {
    *out = StatFs {
        f_type: stats.fs_type,
        f_bsize: stats.block_size,
        f_blocks: stats.blocks,
        f_bfree: stats.blocks_free,
        f_bavail: stats.blocks_available,
        f_files: stats.files,
        f_ffree: stats.files_free,
        f_fsid: [0, 0],
        f_namelen: stats.name_length,
        f_frsize: stats.fragment_size,
        f_flags: stats.flags,
        f_spare: [0; 4],
    };
}

const FD_CLOEXEC: i32 = 0x1;
const STATUS_FLAG_MUTABLE: i32 = O_APPEND | O_NONBLOCK;

#[inline]
fn random_ephemeral_port() -> u16 {
    49152 + sys::rng::get_u16() % 16384
}

fn parse_sockaddr_in(addr_ptr: usize, addr_len: usize) -> Result<IpEndpoint, i32> {
    if addr_ptr == 0 {
        return Err(EFAULT as i32);
    }
    if addr_len < size_of::<SockAddrIn>() {
        return Err(EINVAL);
    }

    // SAFETY: userspace ABI expects a valid pointer.
    let sin = unsafe { &*(addr_ptr as *const SockAddrIn) };
    if sin.sin_family != AF_INET {
        return Err(EAFNOSUPPORT);
    }

    let port = u16::from_be(sin.sin_port);
    let ip_bytes = u32::from_be(sin.sin_addr).to_be_bytes();
    let addr = IpAddress::Ipv4(Ipv4Address::from_octets(ip_bytes));

    Ok(IpEndpoint::new(addr, port))
}

fn write_sockaddr_in(addr_ptr: usize, addrlen_ptr: usize, ep: IpEndpoint) -> Result<(), i64> {
    if addr_ptr == 0 {
        return Ok(());
    }
    if addrlen_ptr == 0 {
        return Err(-(EFAULT as i64));
    }

    let addrlen = unsafe { &mut *(addrlen_ptr as *mut SocklenT) };
    let need = size_of::<SockAddrIn>() as SocklenT;
    if *addrlen < need {
        *addrlen = need;
        return Ok(());
    }

    let (ip, port) = match ep.addr {
        IpAddress::Ipv4(a) => (a.octets(), ep.port),
        // _ => return Err(-(EAFNOSUPPORT as i64)),
    };

    let out = unsafe { &mut *(addr_ptr as *mut SockAddrIn) };
    out.sin_family = AF_INET;
    out.sin_port = port.to_be();
    out.sin_addr = u32::from_be_bytes(ip).to_be();
    out.sin_zero = [0u8; 8];
    *addrlen = need;
    Ok(())
}

fn status_flags_from_open(flags: i32) -> i32 {
    let mut status = flags & O_ACCMODE;
    status |= flags & (O_APPEND | O_NONBLOCK | O_DIRECTORY | O_PATH);
    status
}

fn apply_umask(mode: u32, umask: u16) -> u16 {
    ((mode as u16) & 0o7777) & !(umask & 0o777)
}

fn fd_slot(process: &Process, fd: i32) -> Result<&FdEntry, i32> {
    process.fd_entry(fd).ok_or(-EBADF)
}

fn fd_slot_mut(process: &mut Process, fd: i32) -> Result<&mut FdEntry, i32> {
    process.fd_entry_mut(fd).ok_or(-EBADF)
}

fn clone_open_file(process: &Process, fd: i32) -> Result<Arc<Mutex<OpenFile>>, i32> {
    Ok(fd_slot(process, fd)?.file.clone())
}

fn install_fd_entry(process: &mut Process, entry: FdEntry, min_fd: i32) -> Result<i32, i32> {
    process.install_fd(entry, min_fd)
}

fn duplicate_fd(process: &mut Process, oldfd: i32, min_fd: i32, fd_flags: i32) -> Result<i32, i32> {
    let file = fd_slot(process, oldfd)?.file.clone();
    install_fd_entry(process, FdEntry { file, fd_flags }, min_fd).map_err(|errno| -errno)
}

fn base_for_dirfd(process: &mut Process, dirfd: i32) -> Result<String, i32> {
    if dirfd == AT_FDCWD {
        return Ok(process.pwd.clone());
    }
    let entry = fd_slot(process, dirfd)?;
    let file = entry.file.lock();
    match &file.kind {
        OpenFileKind::Vfs(node) => {
            if node.lock().metadata.file_type != FileType::Dir {
                return Err(-ENOTDIR);
            }
        }
        OpenFileKind::Pipe(_) => return Err(-ENOTDIR),
        OpenFileKind::Socket(_) => return Err(-ENOTDIR),
    }

    Ok(file.path.clone())
}
fn split_parent_name(path: &str) -> (&str, &str) {
    if let Some(p) = path.rfind('/') {
        if p == 0 {
            ("/", &path[1..])
        } else {
            (&path[..p], &path[p + 1..])
        }
    } else {
        (".", path)
    }
}

pub fn write(arg1: i32, arg2: usize, arg3: usize) -> i64 {
    let buf = arg2 as *const u8;
    let len = arg3;
    let buf = unsafe { core::slice::from_raw_parts(buf, len) };

    let current_pid = crate::sys::proc::id();
    #[allow(static_mut_refs)]
    let process_opt = unsafe { PROCESS_TABLE.get_mut().unwrap().get_process(current_pid) };

    let process = match process_opt {
        Some(process) => process,
        None => return -(ESRCH as i64),
    };

    let file_ref = match clone_open_file(process, arg1) {
        Ok(file) => file,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    if file.status_flags & O_ACCMODE == O_RDONLY {
        return -(EBADF as i64);
    }

    let status_flags = file.status_flags;
    let nonblock = (status_flags & O_NONBLOCK) != 0;
    let seek = file.seek;

    if let OpenFileKind::Pipe(pipe) = &file.kind {
        let pipe = pipe.clone();
        drop(file);
        let result = match pipe.write(buf, nonblock) {
            Ok(written) => written as i64,
            Err(EPIPE) => {
                crate::sys::proc::queue_signal(current_pid, SIGPIPE);
                -(EPIPE as i64)
            }
            Err(errno) => -(errno as i64),
        };
        return result;
    }

    let (result, new_seek) = match &mut file.kind {
        OpenFileKind::Vfs(node_ref) => {
            let append = (status_flags & O_APPEND) != 0;
            let mut node = node_ref.lock();
            let file_type = node.metadata.file_type;
            let start = match file_type {
                FileType::File if append => node.metadata.size,
                FileType::File | FileType::BlockDevice => seek,
                FileType::Dir | FileType::CharDevice => 0,
            };
            let end = start.saturating_add(buf.len());

            match node.write(start, buf) {
                Ok(()) => {
                    if matches!(file_type, FileType::File) && end > node.metadata.size {
                        node.metadata.size = end;
                    }
                    let new_seek =
                        matches!(file_type, FileType::File | FileType::BlockDevice).then_some(end);
                    (buf.len() as i64, new_seek)
                }
                Err(_) => (-(EIO as i64), None),
            }
        }
        OpenFileKind::Pipe(_) => unreachable!(),
        OpenFileKind::Socket(sock) => {
            if nonblock && !sock.poll(IO::Write) {
                (-(EAGAIN as i64), None)
            } else {
                match sock.write(buf) {
                    Ok(written) => (written as i64, None),
                    Err(_) => (-(EIO as i64), None),
                }
            }
        }
    };

    if let Some(new_seek) = new_seek {
        file.seek = new_seek;
    }

    result
}

pub fn close(fd: i32) -> i64 {
    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    match process.close_fd(fd) {
        Ok(entry) => {
            drop(entry);
            0
        }
        Err(errno) => -(errno as i64),
    }
}

pub fn ftruncate(fd: i32, length: u64) -> i64 {
    if fd < 0 {
        return -(EBADF as i64);
    }
    if length > (usize::MAX as u64) {
        return -(EINVAL as i64);
    }
    let new_len = length as usize;

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    match clone_open_file(process, fd) {
        Ok(file_ref) => {
            let mut file = file_ref.lock();

            let accmode = file.status_flags & O_ACCMODE;
            if accmode == O_RDONLY {
                return -(EBADF as i64);
            }

            let truncate_res = {
                match &mut file.kind {
                    OpenFileKind::Vfs(node_ref) => {
                        let mut node = node_ref.lock();
                        if node.metadata.file_type != FileType::File {
                            return -(EINVAL as i64);
                        }
                        node.truncate(new_len)
                    }
                    OpenFileKind::Pipe(_) => return -(EINVAL as i64),
                    OpenFileKind::Socket(_) => return -(EINVAL as i64),
                }
            };

            match truncate_res {
                Ok(()) => {
                    if file.seek > new_len {
                        file.seek = new_len;
                    }
                    0
                }
                Err(errno) => errno as i64,
            }
        }
        Err(code) => code as i64,
    }
}

pub fn dup2(oldfd: i32, newfd: i32) -> i64 {
    if oldfd < 0 || newfd < 0 {
        return -(EBADF as i64);
    }
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let old_entry = match fd_slot(process, oldfd) {
        Ok(entry) => entry,
        Err(code) => return code as i64,
    };
    if oldfd == newfd {
        return newfd as i64;
    }

    let cloned = FdEntry {
        file: old_entry.file.clone(),
        fd_flags: 0,
    };
    match process.replace_fd(newfd, cloned) {
        Ok(replaced) => {
            drop(replaced);
            newfd as i64
        }
        Err(errno) => -(errno as i64),
    }
}

pub fn dup(oldfd: i32) -> i64 {
    if oldfd < 0 {
        return -(EBADF as i64);
    }

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return -(ESRCH as i64);
    };

    match duplicate_fd(process, oldfd, 0, 0) {
        Ok(newfd) => newfd as i64,
        Err(code) => code as i64,
    }
}

pub fn pipe(pipefd_ptr: usize) -> i64 {
    pipe2(pipefd_ptr, 0)
}

pub fn pipe2(pipefd_ptr: usize, flags: i32) -> i64 {
    if pipefd_ptr == 0 {
        return -(EFAULT as i64);
    }

    let allowed = O_CLOEXEC | O_NONBLOCK;
    if (flags & !allowed) != 0 {
        return -(EINVAL as i64);
    }

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let (read_end, write_end) = make_pipe_ends();
    let cloexec = (flags & O_CLOEXEC) != 0;

    let r_open = OpenFile {
        kind: OpenFileKind::Pipe(Arc::new(read_end)),
        seek: 0,
        path: "pipe".to_string(),
        status_flags: status_flags_from_open(O_RDONLY | (flags & O_NONBLOCK)),
    };
    let w_open = OpenFile {
        kind: OpenFileKind::Pipe(Arc::new(write_end)),
        seek: 0,
        path: "pipe".to_string(),
        status_flags: status_flags_from_open(O_WRONLY | (flags & O_NONBLOCK)),
    };

    let r_entry = FdEntry {
        file: Arc::new(Mutex::new(r_open)),
        fd_flags: if cloexec { FD_CLOEXEC } else { 0 },
    };
    let w_entry = FdEntry {
        file: Arc::new(Mutex::new(w_open)),
        fd_flags: if cloexec { FD_CLOEXEC } else { 0 },
    };

    let rfd = match install_fd_entry(process, r_entry, 0) {
        Ok(fd) => fd,
        Err(code) => return -(code as i64),
    };
    let wfd = match install_fd_entry(process, w_entry, 0) {
        Ok(fd) => fd,
        Err(code) => {
            // Roll back read end
            let _ = process.close_fd(rfd);
            return -(code as i64);
        }
    };

    unsafe {
        let out = pipefd_ptr as *mut i32;
        out.add(0).write(rfd);
        out.add(1).write(wfd);
    }
    0
}

pub fn read(fd: usize, buf: &mut [u8]) -> i64 {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    fn read_from_fd(process: &mut Process, fd: i32, buf: &mut [u8]) -> i64 {
        let file_ref = match clone_open_file(process, fd) {
            Ok(f) => f,
            Err(code) => return code as i64,
        };
        let mut file = file_ref.lock();
        let status_flags = file.status_flags;
        let accmode = status_flags & O_ACCMODE;
        if accmode == O_WRONLY {
            return -(EBADF as i64);
        }
        let seek = file.seek;

        if let OpenFileKind::Pipe(pipe) = &file.kind {
            let pipe = pipe.clone();
            let nonblock = (status_flags & O_NONBLOCK) != 0;
            drop(file);
            let result = match pipe.read(buf, nonblock) {
                Ok(count) => count as i64,
                Err(errno) => -(errno as i64),
            };
            return result;
        }

        let (ret, advance_seek) = match &mut file.kind {
            OpenFileKind::Vfs(node_ref) => {
                let mut vfs_node = node_ref.lock();
                match vfs_node.metadata.file_type {
                    FileType::Dir => (-(EISDIR as i64), None),
                    FileType::CharDevice => {
                        let mut effective_seek = seek;
                        if vfs_node.metadata.name == "kmsg"
                            && let Ok(head) = vfs_node.ioctl(IOCTL_KMSG_GET_HEAD, 0)
                            && head >= 0
                        {
                            effective_seek = effective_seek.max(head as usize);
                        }
                        if status_flags & O_NONBLOCK != 0 {
                            match vfs_node.poll() {
                                Ok(true) => {}
                                Ok(false) => return -(EAGAIN as i64),
                                Err(_) => return -(EIO as i64),
                            }
                        }

                        match vfs_node.read(effective_seek, buf) {
                            Ok(n) => {
                                let advance = effective_seek.saturating_sub(seek).saturating_add(n);
                                (n as i64, Some(advance))
                            }
                            Err(_) => (-(EIO as i64), None),
                        }
                    }
                    _ => match vfs_node.read(seek, buf) {
                        Ok(copy_len) => (copy_len as i64, Some(copy_len)),
                        Err(_) => (-(EIO as i64), None),
                    },
                }
            }
            OpenFileKind::Pipe(_) => unreachable!(),
            OpenFileKind::Socket(sock) => {
                let nonblock = (status_flags & O_NONBLOCK) != 0;
                if nonblock && !sock.poll(IO::Read) {
                    (-(EAGAIN as i64), None)
                } else {
                    match sock.read(buf) {
                        Ok(n) => (n as i64, None),
                        Err(_) => (-(EIO as i64), None),
                    }
                }
            }
        };

        if let Some(n) = advance_seek {
            file.seek = file.seek.saturating_add(n);
        }

        ret
    }

    read_from_fd(process, fd as i32, buf)
}

pub fn open(path: &str, flags: i32, mode: u32) -> i64 {
    openat(AT_FDCWD, path, flags, mode)
}

pub fn openat(dirfd: i32, path: &str, flags: i32, mode: u32) -> i64 {
    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };
    let creation_mode = apply_umask(mode, process.umask);

    // Resolve full path
    let full_path = if path.starts_with('/') {
        normalize_path(path)
    } else {
        match base_for_dirfd(process, dirfd) {
            Ok(base) => normalize_path(&join_paths(&base, path)),
            Err(e) => return e as i64,
        }
    };
    if let Err(e) = check_encrypted_home_access(&full_path) {
        return e;
    }

    // Try open existing
    let mut existed = true;
    #[allow(static_mut_refs)]
    let node = unsafe { VFS.get_mut().open(&full_path) };
    let mut node = match (node, (flags & O_CREAT) != 0) {
        (Ok(n), _) => n,
        (Err(_), true) => {
            // create new file with mode
            let (parent, name) = split_parent_name(&full_path);
            // parent must exist and be a dir
            #[allow(static_mut_refs)]
            if let Ok(meta) = unsafe { VFS.get_mut().metadata(parent) } {
                if meta.file_type != FileType::Dir {
                    return -(ENOTDIR as i64);
                }
            } else {
                return -(ENOENT as i64);
            }

            #[allow(static_mut_refs)]
            if unsafe { VFS.get_mut().touch(parent, name, creation_mode) }.is_err() {
                return -(EIO as i64);
            }
            existed = false;
            #[allow(static_mut_refs)]
            // reopen
            match unsafe { VFS.get_mut().open(&full_path) } {
                Ok(n2) => n2,
                Err(_) => return -(EIO as i64),
            }
        }
        (Err(_), false) => return -(ENOENT as i64),
    };

    // Enforce O_EXCL if it existed
    if existed && (flags & O_CREAT) != 0 && (flags & O_EXCL) != 0 {
        return -(EEXIST as i64);
    }

    // O_DIRECTORY: must be a directory
    if (flags & O_DIRECTORY) != 0 && node.metadata.file_type != FileType::Dir {
        return -(ENOTDIR as i64);
    }

    // Cannot open directory for write unless you implement it
    let accmode = flags & O_ACCMODE;
    if node.metadata.file_type == FileType::Dir && (accmode == O_WRONLY || accmode == O_RDWR) {
        return -(EISDIR as i64);
    }

    // O_TRUNC (only for regular files)
    if (flags & O_TRUNC) != 0 && node.metadata.file_type == FileType::File {
        if accmode != O_RDONLY {
            if let Err(errno) = node.truncate(0) {
                return errno as i64;
            }
        }
    }

    let mut initial_seek: usize = 0;
    if node.metadata.file_type == FileType::File {
        let file_len = node.metadata.size;
        if (flags & O_APPEND) != 0 {
            initial_seek = file_len;
        }
    }

    // Install FD
    let open_file = OpenFile {
        kind: OpenFileKind::Vfs(Arc::new(Mutex::new(node))),
        seek: initial_seek,
        path: full_path,
        status_flags: status_flags_from_open(flags),
    };
    let entry = FdEntry {
        file: Arc::new(Mutex::new(open_file)),
        fd_flags: if (flags & O_CLOEXEC) != 0 {
            FD_CLOEXEC
        } else {
            0
        },
    };
    match install_fd_entry(process, entry, 0) {
        Ok(fd) => fd as i64,
        Err(code) => -(code as i64),
    }
}

pub fn execve(
    arg1: usize,
    arg2: usize,
    arg3: usize,
    stack_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    _regs: &mut crate::arch::x86_64::idt::Registers,
) -> i64 {
    execev(arg1, arg2, arg3, stack_frame)
}

fn resolve_exec_path(path: &str) -> Result<String, i64> {
    let path = path.trim();
    if path.is_empty() {
        return Err(-(ENOENT as i64));
    }
    if path.starts_with('/') {
        return Ok(normalize_path(path));
    }

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return Err(-(ESRCH as i64));
    };

    Ok(normalize_path(&join_paths(&process.pwd, path)))
}

fn read_exec_file(path: &str) -> Result<Vec<u8>, i64> {
    #[allow(static_mut_refs)]
    let Ok(mut node) = (unsafe { VFS.read().open(path) }) else {
        return Err(-(ENOENT as i64));
    };

    let size = node.metadata.size;
    let mut buf = vec![0u8; size];
    node.read(0, &mut buf).map_err(|_| -(EIO as i64))?;
    Ok(buf)
}

fn parse_shebang(content: &[u8]) -> Result<Option<(String, Option<String>)>, i64> {
    if !content.starts_with(b"#!") {
        return Ok(None);
    }

    let line_end = content
        .iter()
        .position(|&byte| byte == b'\n')
        .unwrap_or(content.len());
    let mut line = &content[2..line_end];
    if line.ends_with(b"\r") {
        line = &line[..line.len() - 1];
    }

    let line = core::str::from_utf8(line).map_err(|_| -(ENOEXEC as i64))?;
    let mut parts = line.split_ascii_whitespace();
    let Some(interpreter) = parts.next() else {
        return Err(-(ENOEXEC as i64));
    };
    let interpreter_arg = parts.next().map(ToString::to_string);

    let interpreter = match interpreter {
        "/bin/oksh" => "/bin/oksh",
        "/bin/bash" => "/bin/oksh",
        _ => return Err(-(ENOEXEC as i64)),
    };

    Ok(Some((interpreter.to_string(), interpreter_arg)))
}

fn copy_envp(arg3: usize) -> Result<Vec<String>, i64> {
    if arg3 == 0 {
        return Ok(Vec::new());
    }

    copy_user_ptr_array(UserPtr(arg3 as *const usize), 128, 4096).map_err(|_| -(EFAULT as i64))
}

fn argv_refs(argv: &[String]) -> Vec<&str> {
    argv.iter().map(|arg| arg.as_str()).collect()
}

pub fn execev(
    arg1: usize,
    arg2: usize,
    arg3: usize,
    stack_frame: &mut x86_64::structures::idt::InterruptStackFrame,
) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(arg1 as *const u8), 4096) else {
        crate::serial_println!(
            "[execve] pid={} bad path ptr={:#x}",
            crate::sys::proc::id(),
            arg1
        );
        return -(EFAULT as i64);
    };
    crate::serial_println!("[execve] pid={} path={}", crate::sys::proc::id(), path);

    let exec_path = match resolve_exec_path(&path) {
        Ok(path) => path,
        Err(code) => return code,
    };

    let file_buf = match read_exec_file(&exec_path) {
        Ok(buf) => buf,
        Err(code) => {
            let err_path = if exec_path.is_empty() {
                path.as_str()
            } else {
                exec_path.as_str()
            };
            crate::serial_println!(
                "[execve] pid={} open failed path={}",
                crate::sys::proc::id(),
                err_path
            );
            return code;
        }
    };

    let argv = match copy_user_ptr_array(UserPtr(arg2 as *const usize), 128, 4096) {
        Ok(v) => v,
        Err(_) => return -(EFAULT as i64),
    };
    let env = match copy_envp(arg3) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let (image_buf, image_path, final_argv) = if file_buf.starts_with(&[0x7f, b'E', b'L', b'F']) {
        (file_buf, exec_path.clone(), argv)
    } else if let Some((interpreter, interpreter_arg)) = match parse_shebang(&file_buf) {
        Ok(value) => value,
        Err(code) => return code,
    } {
        let interpreter_buf = match read_exec_file(&interpreter) {
            Ok(buf) => buf,
            Err(code) => {
                crate::serial_println!(
                    "[execve] pid={} interpreter open failed path={}",
                    crate::sys::proc::id(),
                    interpreter
                );
                return code;
            }
        };
        if interpreter_buf.starts_with(b"#!")
            || !interpreter_buf.starts_with(&[0x7f, b'E', b'L', b'F'])
        {
            return -(ENOEXEC as i64);
        }

        let mut script_argv = Vec::new();
        script_argv.push(interpreter.clone());
        if let Some(arg) = interpreter_arg {
            script_argv.push(arg);
        }
        script_argv.push(exec_path.clone());
        for arg in argv.iter().skip(1) {
            script_argv.push(arg.clone());
        }

        (interpreter_buf, interpreter, script_argv)
    } else {
        return -(ENOEXEC as i64);
    };

    if image_buf.is_empty() {
        crate::serial_println!(
            "[execve] pid={} empty executable path={}",
            crate::sys::proc::id(),
            image_path
        );
        return -(ENOEXEC as i64);
    }

    #[allow(static_mut_refs)]
    let process_table = unsafe { PROCESS_TABLE.get_mut().unwrap() };

    // We execute on the current process.
    if let Some(p) = process_table.get_process(crate::sys::proc::id()) {
        let argv_strs = argv_refs(&final_argv);
        let env_strs = argv_refs(&env);

        match p.exec(&image_buf, &argv_strs, &env_strs) {
            Ok((entry, sp)) => {
                p.exe_path = image_path.clone();
                p.set_comm_from_path(&image_path);
                crate::serial_println!(
                    "[execve] pid={} loaded path={} entry={:#x} sp={:#x}",
                    crate::sys::proc::id(),
                    image_path,
                    entry,
                    sp,
                );
                // Determine Code Segment/Stack Segment for user (Ring 3).
                // They should be USER_CS/USER_SS.
                // The interrupt frame likely already has them, but we ensure RIP/RSP are set.
                // Stack frame struct:
                // pub instruction_pointer: VirtAddr,
                // pub code_segment: u64,
                // pub cpu_flags: RFlags,
                // pub stack_pointer: VirtAddr,
                // pub stack_segment: u64,

                use x86_64::VirtAddr;
                unsafe {
                    let frame_ptr = stack_frame as *mut _
                        as *mut x86_64::structures::idt::InterruptStackFrameValue;
                    (*frame_ptr).instruction_pointer = VirtAddr::new(entry);
                    (*frame_ptr).stack_pointer = VirtAddr::new(sp);
                }
                0
            }
            Err(_) => {
                crate::serial_println!(
                    "[execve] pid={} exec failed path={}",
                    crate::sys::proc::id(),
                    image_path,
                );
                -(ENOEXEC as i64)
            }
        }
    } else {
        crate::serial_println!("[execve] no process pid={}", crate::sys::proc::id());
        -(ESRCH as i64)
    }
}

pub fn exit(_status: i32) -> i64 {
    sys::proc::exit(_status);

    unreachable!()
}

pub fn exit_group(status: i32) -> i64 {
    sys::proc::exit_group(status)
}

pub fn fork(
    stack_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    regs: &mut crate::arch::x86_64::idt::Registers,
) -> i64 {
    use crate::sys::proc::{InterruptStack, IretRegisters, PreservedRegisters, ScratchRegisters};

    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };

    // We need to find the current process to call fork on it.
    let current_pid = crate::sys::proc::id();
    crate::serial_println!(
        "[sys_fork] current={} user_rip={:#x} user_rsp={:#x}",
        current_pid,
        stack_frame.instruction_pointer.as_u64(),
        stack_frame.stack_pointer.as_u64(),
    );
    let process_opt = table.proc_list.iter_mut().find(|p| p.pid == current_pid);

    if let Some(process) = process_opt {
        // Construct the TrapFrame (InterruptStack) from the syscall context
        let tf = InterruptStack {
            preserved: PreservedRegisters {
                r15: regs.r15,
                r14: regs.r14,
                r13: regs.r13,
                r12: regs.r12,
                rbp: regs.rbp,
                rbx: regs.rbx,
            },
            scratch: ScratchRegisters {
                r11: regs.r11,
                r10: regs.r10,
                r9: regs.r9,
                r8: regs.r8,
                rsi: regs.rsi,
                rdi: regs.rdi,
                rdx: regs.rdx,
                rcx: regs.rcx,
                rax: regs.rax,
            },
            iret: IretRegisters {
                rip: stack_frame.instruction_pointer.as_u64(),
                cs: stack_frame.code_segment.0 as u64,
                rflags: stack_frame.cpu_flags.bits() | 0x202,
                rsp: stack_frame.stack_pointer.as_u64(),
                ss: stack_frame.stack_segment.0 as u64,
            },
        };

        if let Ok(child) = process.fork(&tf) {
            let child_pid = child.pid;
            table.proc_list.push_back(child);
            crate::serial_println!(
                "[sys_fork] parent={} child={} return",
                current_pid,
                child_pid
            );
            return child_pid as i64;
        }
    }

    crate::serial_println!("[sys_fork] current={} failed", current_pid);
    -(ENOSYS as i64)
}

pub fn clone(
    flags: u64,
    child_stack: u64,
    _parent_tid: usize,
    _child_tid: usize,
    tls: u64,
    stack_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    regs: &mut crate::arch::x86_64::idt::Registers,
) -> i64 {
    const CLONE_VM: u64 = 0x0000_0100;
    const CLONE_FS: u64 = 0x0000_0200;
    const CLONE_FILES: u64 = 0x0000_0400;
    const CLONE_SIGHAND: u64 = 0x0000_0800;
    const CLONE_THREAD: u64 = 0x0001_0000;
    const CLONE_SYSVSEM: u64 = 0x0004_0000;
    const CLONE_SETTLS: u64 = 0x0008_0000;

    let required = CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD | CLONE_SYSVSEM;
    let supported = required | CLONE_SETTLS;
    if child_stack == 0 || flags & required != required || flags & !supported != 0 {
        return -(EINVAL as i64);
    }

    use crate::sys::proc::{InterruptStack, IretRegisters, PreservedRegisters, ScratchRegisters};

    let tf = InterruptStack {
        preserved: PreservedRegisters {
            r15: regs.r15,
            r14: regs.r14,
            r13: regs.r13,
            r12: regs.r12,
            rbp: regs.rbp,
            rbx: regs.rbx,
        },
        scratch: ScratchRegisters {
            r11: regs.r11,
            r10: regs.r10,
            r9: regs.r9,
            r8: regs.r8,
            rsi: regs.rsi,
            rdi: regs.rdi,
            rdx: regs.rdx,
            rcx: regs.rcx,
            rax: regs.rax,
        },
        iret: IretRegisters {
            rip: stack_frame.instruction_pointer.as_u64(),
            cs: stack_frame.code_segment.0 as u64,
            rflags: stack_frame.cpu_flags.bits() | 0x202,
            rsp: stack_frame.stack_pointer.as_u64(),
            ss: stack_frame.stack_segment.0 as u64,
        },
    };

    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    let current_pid = crate::sys::proc::id();
    let Some(process) = table
        .proc_list
        .iter_mut()
        .find(|process| process.pid == current_pid)
    else {
        return -(ESRCH as i64);
    };

    let tls_base = if flags & CLONE_SETTLS != 0 {
        tls
    } else {
        process.fs_base.as_u64()
    };
    let child = match process.clone_thread(&tf, child_stack, tls_base) {
        Ok(child) => child,
        Err(()) => return -(EAGAIN as i64),
    };
    let child_pid = child.pid;
    table.proc_list.push_back(child);
    child_pid as i64
}

pub fn wait4(pid: i32, status_ptr: usize, options: i32, _rusage_ptr: usize) -> i64 {
    let current_pid = crate::sys::proc::id();
    let wnohang = 1;
    let wuntraced = 2;
    crate::serial_println!(
        "[wait4] pid={} target={} options={:#x}",
        current_pid,
        pid,
        options,
    );

    loop {
        let mut reaped_pid = None;
        let mut wait_status = 0;
        let mut has_children = false;
        let mut stopped_pid = None;

        {
            #[allow(static_mut_refs)]
            let table = unsafe { crate::sys::proc::PROCESS_TABLE.get_mut().unwrap() };

            // We need to iterate and find a child.
            // Since we might remove it, we collect index first.
            let mut remove_idx = None;

            for (i, p) in table.proc_list.iter().enumerate() {
                if !p.is_thread && p.parent_pid == current_pid {
                    if pid == -1 || p.pid as i32 == pid {
                        has_children = true;
                        if matches!(p.state, crate::sys::proc::ProcessState::Dead) {
                            remove_idx = Some(i);
                            wait_status = p.wait_status;
                            break;
                        } else if (options & wuntraced) != 0
                            && matches!(p.state, crate::sys::proc::ProcessState::Stopped)
                            && !p.wait_reported
                        {
                            stopped_pid = Some(i);
                            wait_status = p.wait_status;
                            break;
                        }
                    }
                }
            }

            if let Some(idx) = remove_idx {
                if let Some(mut p) = table.proc_list.remove(idx) {
                    let tgid = p.tgid;
                    table
                        .proc_list
                        .retain(|process| !(process.is_thread && process.tgid == tgid));
                    reaped_pid = Some(p.pid);
                    let table_frame = p.page_table_frame;
                    crate::serial_println!(
                        "[wait4] pid={} reap child={} status={:#x}",
                        current_pid,
                        p.pid,
                        p.wait_status,
                    );
                    p.cleanup(table_frame);
                    core::mem::forget(p);
                }
            } else if let Some(idx) = stopped_pid {
                if let Some(p) = table.proc_list.get_mut(idx) {
                    p.wait_reported = true;
                    reaped_pid = Some(p.pid);
                }
            } else if !has_children {
                crate::serial_println!("[wait4] pid={} no children", current_pid);
                return -(ECHILD as i64);
            }
        }

        if let Some(rpid) = reaped_pid {
            if status_ptr != 0 {
                let status_ref = unsafe { &mut *(status_ptr as *mut i32) };
                *status_ref = wait_status;
            }
            return rpid as i64;
        }

        if (options & wnohang) != 0 {
            return 0;
        }

        // Wait (block)
        {
            #[allow(static_mut_refs)]
            let table = unsafe { crate::sys::proc::PROCESS_TABLE.get_mut().unwrap() };
            if let Some(me) = table.proc_list.iter_mut().find(|p| p.pid == current_pid) {
                crate::serial_println!("[wait4] pid={} block", current_pid);
                me.state = crate::sys::proc::ProcessState::Waiting;
            }
        }

        crate::sys::proc::schedule_now(); // Yield
        crate::serial_println!("[wait4] pid={} resumed", current_pid);
    }
}

pub fn sched_yield() -> i64 {
    crate::sys::proc::schedule_now();
    0
}

pub fn sched_getaffinity(pid: i32, cpusetsize: usize, mask_ptr: usize) -> i64 {
    let cpu_count = crate::driver::cpu::cpu_count();
    let word_size = size_of::<usize>();
    let mask_size = cpu_count.div_ceil(usize::BITS as usize) * word_size;

    if cpusetsize < cpu_count.div_ceil(8) || cpusetsize % word_size != 0 {
        return -(EINVAL as i64);
    }

    let target_pid = if pid == 0 {
        crate::sys::proc::id()
    } else if pid > 0 && pid <= u16::MAX as i32 {
        pid as u16
    } else {
        return -(ESRCH as i64);
    };

    #[allow(static_mut_refs)]
    let target_exists = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .proc_list
            .iter()
            .any(|process| process.pid == target_pid)
    };
    if !target_exists {
        return -(ESRCH as i64);
    }
    if mask_ptr == 0 {
        return -(EFAULT as i64);
    }

    let copy_len = core::cmp::min(cpusetsize, mask_size);
    let mask = unsafe { core::slice::from_raw_parts_mut(mask_ptr as *mut u8, copy_len) };
    mask.fill(0);

    // Userspace tasks currently run only on the bootstrap processor.
    mask[0] = 1;
    copy_len as i64
}

pub fn pread64(fd: i32, buf_ptr: usize, count: usize, offset: u64) -> i64 {
    if buf_ptr == 0 {
        return -(EFAULT as i64);
    }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, count) };

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };

    let file = file_ref.lock();
    let accmode = file.status_flags & O_ACCMODE;
    if accmode == O_WRONLY {
        return -(EBADF as i64);
    }

    match &file.kind {
        OpenFileKind::Vfs(node_ref) => {
            let mut node = node_ref.lock();
            match node.metadata.file_type {
                FileType::Dir => return -(EISDIR as i64),
                FileType::CharDevice => {}
                _ => {}
            }

            match node.read(offset as usize, buf) {
                Ok(n) => n as i64,
                Err(_) => -(EIO as i64),
            }
        }
        OpenFileKind::Pipe(_) => -(ESPIPE as i64),
        OpenFileKind::Socket(_) => -(ESPIPE as i64),
    }
}

pub fn uname(ptr: usize) -> i64 {
    let uname_ptr = ptr as *mut UtsName;

    fn fill(buf: &mut [u8; 65], s: &str) {
        buf.fill(0);
        let bytes = s.as_bytes();
        let n = core::cmp::min(bytes.len(), 64); // leave room for NUL
        buf[..n].copy_from_slice(&bytes[..n]);
        buf[n] = 0;
    }

    if !uname_ptr.is_null() {
        unsafe {
            let uname_s = &mut *uname_ptr;

            fill(&mut uname_s.sysname, "TwilightOS");
            fill(&mut uname_s.nodename, "twilight");
            fill(&mut uname_s.release, "0.1.0-testing-build.x86_64");
            fill(&mut uname_s.version, "#1 NON-SMP 16-02-2026");
            fill(&mut uname_s.machine, "x86_64");
            fill(&mut uname_s.domainname, "(none)");
        }
    }

    0
}

pub fn arch_prctl(code: u64, addr: u64) -> i64 {
    match code {
        ARCH_SET_FS => {
            wrmsr(IA32_FS_BASE, addr);

            #[allow(static_mut_refs)]
            if let Some(table) = unsafe { PROCESS_TABLE.get_mut() } {
                let current_pid = sys::proc::id();
                if let Some(process) = table.get_process(current_pid) {
                    process.fs_base = x86_64::VirtAddr::new(addr);
                }
            }

            0
        }
        ARCH_GET_FS => {
            if addr == 0 {
                -(EFAULT as i64)
            } else {
                unsafe { *(addr as *mut u64) = rdmsr(IA32_FS_BASE) };
                0
            }
        }
        ARCH_SET_GS => {
            let base = x86_64::VirtAddr::new(addr);
            crate::arch::x86_64::io::set_inactive_gsbase()(base);

            #[allow(static_mut_refs)]
            if let Some(table) = unsafe { PROCESS_TABLE.get_mut() } {
                let current_pid = sys::proc::id();
                if let Some(process) = table.get_process(current_pid) {
                    process.gs_base = base;
                }
            }

            0
        }
        ARCH_GET_GS => {
            if addr == 0 {
                -(EFAULT as i64)
            } else {
                unsafe {
                    *(addr as *mut u64) = crate::arch::x86_64::io::get_inactive_gsbase()().as_u64()
                };
                0
            }
        }
        _ => {
            logger!(
                "arch_prctl: unsupported code=0x{:x}, arg=0x{:x}",
                code,
                addr
            );
            -(EINVAL as i64)
        }
    }
}

pub fn prctl(option: i32, arg2: usize, _arg3: usize, _arg4: usize, _arg5: usize) -> i64 {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return -(ESRCH as i64);
    };

    match option {
        PR_SET_NAME => {
            if arg2 == 0 {
                return -(EFAULT as i64);
            }
            let name = unsafe { core::slice::from_raw_parts(arg2 as *const u8, 16) };
            process.set_comm(name);
            0
        }
        PR_GET_NAME => {
            if arg2 == 0 {
                return -(EFAULT as i64);
            }
            let out = unsafe { core::slice::from_raw_parts_mut(arg2 as *mut u8, 16) };
            out.copy_from_slice(&process.comm());
            0
        }
        _ => -(EINVAL as i64),
    }
}

pub fn capget(header_ptr: usize, data_ptr: usize) -> i64 {
    if header_ptr == 0 {
        return -(EFAULT as i64);
    }

    let header = unsafe { &mut *(header_ptr as *mut CapUserHeader) };
    let data_words = match header.version {
        LINUX_CAPABILITY_VERSION_1 => 1,
        LINUX_CAPABILITY_VERSION_2 | LINUX_CAPABILITY_VERSION_3 => 2,
        _ => {
            header.version = LINUX_CAPABILITY_VERSION_3;
            return -(EINVAL as i64);
        }
    };

    let current_pid = crate::sys::proc::id();
    let target_pid = if header.pid == 0 {
        current_pid
    } else if header.pid > 0 && header.pid <= u16::MAX as i32 {
        header.pid as u16
    } else {
        return -(EINVAL as i64);
    };

    #[allow(static_mut_refs)]
    let target_exists = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .proc_list
            .iter()
            .any(|process| process.pid == target_pid)
    };
    if !target_exists {
        return -(ESRCH as i64);
    }
    if data_ptr == 0 {
        return -(EFAULT as i64);
    }

    let data = unsafe { core::slice::from_raw_parts_mut(data_ptr as *mut CapUserData, data_words) };
    data.fill(CapUserData::default());
    0
}

fn valid_timeval(value: Timeval) -> bool {
    value.tv_sec >= 0 && (0..1_000_000).contains(&value.tv_usec)
}

pub fn setitimer(which: i32, new_value_ptr: usize, old_value_ptr: usize) -> i64 {
    const ITIMER_REAL: i32 = 0;

    if which != ITIMER_REAL {
        return -(ENOSYS as i64);
    }

    let new_value = if new_value_ptr == 0 {
        Itimerval::default()
    } else {
        unsafe { *(new_value_ptr as *const Itimerval) }
    };
    if !valid_timeval(new_value.it_interval) || !valid_timeval(new_value.it_value) {
        return -(EINVAL as i64);
    }
    if new_value.it_interval != Timeval::default() || new_value.it_value != Timeval::default() {
        return -(ENOSYS as i64);
    }

    if old_value_ptr != 0 {
        unsafe {
            *(old_value_ptr as *mut Itimerval) = Itimerval::default();
        }
    }
    0
}

pub fn writev(fd: i32, iov_ptr: u64, iovcnt: i32) -> i64 {
    if iovcnt < 0 {
        return -(EINVAL as i64);
    }
    if iovcnt > 0 && iov_ptr == 0 {
        return -(EFAULT as i64);
    }
    let n = iovcnt as usize;

    let iov = unsafe { core::slice::from_raw_parts(iov_ptr as *const Iovec, n) };
    let total_len = match iov
        .iter()
        .try_fold(0usize, |total, item| total.checked_add(item.iov_len))
    {
        Some(total) => total,
        None => return -(EINVAL as i64),
    };
    if total_len == 0 {
        return 0;
    }

    let mut data = Vec::with_capacity(total_len);
    for item in iov {
        if item.iov_len == 0 {
            continue;
        }
        if item.iov_base.is_null() {
            return -(EFAULT as i64);
        }
        let bytes =
            unsafe { core::slice::from_raw_parts(item.iov_base as *const u8, item.iov_len) };
        data.extend_from_slice(bytes);
    }

    write(fd, data.as_ptr() as usize, data.len())
}

pub fn fcntl(fd: i32, cmd: i32, arg: u64) -> i64 {
    const F_DUPFD: i32 = 0;
    const F_GETFD: i32 = 1;
    const F_SETFD: i32 = 2;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const F_DUPFD_CLOEXEC: i32 = 1030;

    if fd < 0 {
        return -(EBADF as i64);
    }

    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    match cmd {
        F_GETFD => match fd_slot(process, fd) {
            Ok(entry) => entry.fd_flags as i64,
            Err(code) => code as i64,
        },
        F_SETFD => {
            let new_flags = (arg as i32) & FD_CLOEXEC;
            match fd_slot_mut(process, fd) {
                Ok(entry) => {
                    entry.fd_flags = (entry.fd_flags & !FD_CLOEXEC) | new_flags;
                    0
                }
                Err(code) => code as i64,
            }
        }
        F_GETFL => match clone_open_file(process, fd) {
            Ok(file_ref) => file_ref.lock().status_flags as i64,
            Err(code) => code as i64,
        },
        F_SETFL => {
            let new_bits = (arg as i32) & STATUS_FLAG_MUTABLE;
            match clone_open_file(process, fd) {
                Ok(file_ref) => {
                    let mut file = file_ref.lock();
                    let preserved = file.status_flags & !STATUS_FLAG_MUTABLE;
                    file.status_flags = preserved | new_bits;
                    0
                }
                Err(code) => code as i64,
            }
        }
        F_DUPFD | F_DUPFD_CLOEXEC => {
            if arg > i32::MAX as u64 {
                return -(EINVAL as i64);
            }
            let min_fd = arg as i32;
            let fd_flags = if cmd == F_DUPFD_CLOEXEC {
                FD_CLOEXEC
            } else {
                0
            };
            match duplicate_fd(process, fd, min_fd, fd_flags) {
                Ok(new_fd) => new_fd as i64,
                Err(code) => code as i64,
            }
        }
        _ => -(EINVAL as i64),
    }
}

pub fn pr_limit64(
    pid: i32,
    resource: u32,
    _new_limit_ptr: Option<&Rlimit64>,
    old_limit_ptr: Option<&mut Rlimit64>,
) -> i64 {
    if pid != 0 {
        return -ESRCH as i64;
    }
    if resource != RLIMIT_STACK {
        return -EINVAL as i64;
    }

    if let Some(old_limit_ptr) = old_limit_ptr {
        old_limit_ptr.rlim_max = USER_STACK_SIZE as u64;
        old_limit_ptr.rlim_cur = USER_STACK_SIZE as u64;
    }

    0
}

struct DirentItem {
    ino: u64,
    dtype: u8,
    name: String,
    reclen: u16, // computed record length including name+NUL+padding
    next_cookie: i64,
}
#[inline(always)]
fn dt_from_filetype(ft: FileType) -> u8 {
    // DT_* values (Linux): UNKNOWN=0,FIFO=1,CHR=2,DIR=4,BLK=6,REG=8,LNK=10,SOCK=12, WHT=14
    match ft {
        FileType::Dir => 4, // DT_DIR
        FileType::File => 8,
        FileType::CharDevice => 2,
        FileType::BlockDevice => 6, // DT_REG
    }
}
#[inline(always)]
fn dirent64_reclen(name_len: usize) -> u16 {
    // the header is 19 bytes (packed), then name + NUL, then 8B align
    let base = size_of::<Dirent64Hdr>(); // 19
    let need = base + name_len + 1;
    let aligned = (need + 7) & !7;
    aligned as u16
}

pub fn getdent64(fd: i32, user_buf: *mut u8, buf_len: usize) -> i64 {
    if user_buf.is_null() {
        return -(EFAULT as i64);
    }
    if buf_len < (size_of::<Dirent64Hdr>() + 2) {
        // practically can't hold any useful name
        return -(EINVAL as i64);
    }

    // Get process and FD
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    match &file.kind {
        OpenFileKind::Vfs(node_ref) => {
            if node_ref.lock().metadata.file_type != FileType::Dir {
                return -(ENOTDIR as i64);
            }
        }
        OpenFileKind::Pipe(_) => return -(ENOTDIR as i64),
        OpenFileKind::Socket(_) => return -(ENOTDIR as i64),
    }
    if let Err(e) = check_encrypted_home_access(&file.path) {
        return e;
    }

    // Read directory entries from VFS (adjust API if yours differs)
    #[allow(static_mut_refs)]
    let entries = match unsafe { VFS.get_mut().ls(&file.path) } {
        Ok(v) => v, // Vec<DirEntry { name:String, inode:u64, file_type:FileType }>
        Err(_) => return -(EIO as i64),
    };

    // Current position (use as entry index 'cookie')
    let mut idx = file.seek;
    if idx >= entries.len() {
        return 0; // EOF
    }

    // 1) Build a struct list with sizes precomputed
    let mut items: Vec<DirentItem> = Vec::new(); // or Vec if you prefer
    let mut total_needed = 0usize;

    for (i, e) in entries.iter().enumerate().skip(idx) {
        let dtype = dt_from_filetype(e.file_type);
        let reclen = dirent64_reclen(e.name.len());
        let reclen_usize = reclen as usize;

        if total_needed + reclen_usize > buf_len {
            break; // stop when buffer would overflow
        }

        items.push(DirentItem {
            ino: e.ino as u64,
            dtype,
            name: e.name.clone(),
            reclen,
            next_cookie: (i as i64) + 1, // “position cookie” to next entry
        }); // ignore overflow if you swap heapless for Vec

        total_needed += reclen_usize;
    }

    if items.is_empty() {
        // Buffer too small to fit the next entry -> return 0 only at EOF,
        // otherwise userspace will retry with a bigger buffer or next loop.
        if idx >= entries.len() {
            return 0;
        }
        return 0; // Behave like Linux: 0 can also mean “no more for now”
    }

    // 2) Serialize struct list into user buffer
    let out = unsafe { core::slice::from_raw_parts_mut(user_buf, buf_len) };
    let mut off = 0usize;

    for it in &items {
        // header
        let hdr = Dirent64Hdr {
            d_ino: it.ino,
            d_off: it.next_cookie,
            d_reclen: it.reclen,
            d_type: it.dtype,
        };
        let hdr_bytes = unsafe {
            core::slice::from_raw_parts(
                (&hdr as *const Dirent64Hdr) as *const u8,
                size_of::<Dirent64Hdr>(),
            )
        };
        out[off..off + hdr_bytes.len()].copy_from_slice(hdr_bytes);
        off += hdr_bytes.len();

        // name + NUL
        let nb = it.name.as_bytes();
        out[off..off + nb.len()].copy_from_slice(nb);
        off += nb.len();
        out[off] = 0;
        off += 1;

        // padding to 8 bytes (reclen accounts for it)
        let pad = (it.reclen as usize) - (size_of::<Dirent64Hdr>() + nb.len() + 1);
        if pad > 0 {
            for b in &mut out[off..off + pad] {
                *b = 0;
            }
            off += pad;
        }

        idx += 1; // consumed this entry
    }

    // 3) Advance directory position
    file.seek = idx;

    off as i64
}

pub(crate) fn stat(file_name_ptr: usize, stat_ptr: usize) -> i64 {
    let file_name_ptr = UserPtr(file_name_ptr as *const u8);
    let Ok(file_path) = copy_cstr_from_user(file_name_ptr, 4096) else {
        return -1;
    };

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(sys::proc::id())
            .unwrap()
    };

    let file_path = if file_path.starts_with('/') {
        normalize_path(file_path.as_str())
    } else {
        normalize_path(&join_paths(process.pwd.as_str(), file_path.as_str()))
    };
    if let Err(e) = check_encrypted_home_access(file_path.as_str()) {
        return e;
    }

    #[allow(static_mut_refs)]
    let Ok(metadata) = (unsafe { VFS.get_mut().metadata(&file_path) }) else {
        return -1;
    };

    let user_stat = unsafe { &mut *(stat_ptr as *mut Stat) };
    fill_stat_from_meta(user_stat, &metadata);

    0
}

pub(crate) fn lstat(file_name_ptr: usize, stat_ptr: usize) -> i64 {
    // TwilightFS currently does not expose distinct symlink metadata semantics here,
    // so lstat behaves the same as stat for now.
    stat(file_name_ptr, stat_ptr)
}

pub fn access(path_ptr: usize, _mode: i32) -> i64 {
    let path_ptr = UserPtr(path_ptr as *const u8);
    let Ok(path) = copy_cstr_from_user(path_ptr, 4096) else {
        return -(EFAULT as i64);
    };

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    // AT_FDCWD is -100
    let full_path = if path.starts_with('/') {
        normalize_path(path.trim())
    } else {
        match base_for_dirfd(process, -100) {
            Ok(base) => normalize_path(&join_paths(&base, path.trim())),
            Err(e) => return e as i64,
        }
    };
    if let Err(e) = check_encrypted_home_access(&full_path) {
        return e;
    }

    #[allow(static_mut_refs)]
    match unsafe { VFS.get_mut().metadata(&full_path) } {
        Ok(_) => 0,
        Err(_) => -(ENOENT as i64),
    }
}

pub fn chmod(path_ptr: usize, mode: u32) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_ptr as *const u8), 4096) else {
        return -(EFAULT as i64);
    };
    if path.is_empty() {
        return -(ENOENT as i64);
    }

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return -(ESRCH as i64);
    };

    let full_path = if path.starts_with('/') {
        normalize_path(&path)
    } else {
        normalize_path(&join_paths(&process.pwd, &path))
    };
    if let Err(e) = check_encrypted_home_access(&full_path) {
        return e;
    }

    #[allow(static_mut_refs)]
    let vfs = unsafe { VFS.get_mut() };
    let Ok(metadata) = vfs.metadata(&full_path) else {
        return -(ENOENT as i64);
    };
    let current_uid = sys::proc::user::get_uid() as u32;
    if current_uid != 0 && current_uid != metadata.uid {
        return -(EPERM as i64);
    }

    match vfs.chmod(&full_path, mode as u16) {
        Ok(()) => 0,
        Err(sys::fs::vfs::VfsError::NotFound) => -(ENOENT as i64),
        Err(sys::fs::vfs::VfsError::NotDir) => -(ENOTDIR as i64),
        Err(sys::fs::vfs::VfsError::ReadOnly) => -(EROFS as i64),
        Err(sys::fs::vfs::VfsError::Invalid) => -(EINVAL as i64),
        Err(sys::fs::vfs::VfsError::Io) => -(EIO as i64),
        Err(sys::fs::vfs::VfsError::AlreadyExists) => -(EEXIST as i64),
    }
}

pub fn umask(mask: u32) -> i64 {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return -(ESRCH as i64);
    };

    let old = process.umask;
    process.umask = (mask as u16) & 0o777;
    old as i64
}

pub fn newfstatat(dirfd: i32, pathname_ptr: usize, stat_ptr: usize, _flags: i32) -> i64 {
    if stat_ptr == 0 {
        return -(EFAULT as i64);
    }
    let pathname_ptr = UserPtr(pathname_ptr as *const u8);
    let Ok(path) = copy_cstr_from_user(pathname_ptr, 4096) else {
        return -(EFAULT as i64);
    };

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let full_path = if path.starts_with('/') {
        normalize_path(path.trim())
    } else {
        match base_for_dirfd(process, dirfd) {
            Ok(base) => normalize_path(&join_paths(&base, path.trim())),
            Err(e) => return e as i64,
        }
    };
    if let Err(e) = check_encrypted_home_access(&full_path) {
        return e;
    }

    #[allow(static_mut_refs)]
    let Ok(meta) = (unsafe { VFS.get_mut().metadata(&full_path) }) else {
        return -(ENOENT as i64);
    };

    let out = unsafe { &mut *(stat_ptr as *mut Stat) };
    fill_stat_from_meta(out, &meta);
    0
}

pub fn fstat(fd: usize, fstat_ptr: usize) -> i64 {
    if fstat_ptr == 0 {
        return -(EFAULT as i64);
    }

    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    let user_stat = unsafe { &mut *(fstat_ptr as *mut Stat) };
    *user_stat = Stat::default();

    if fd > i32::MAX as usize {
        return -(EBADF as i64);
    }

    let file_ref = match clone_open_file(process, fd as i32) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };

    {
        let file = file_ref.lock();
        match &file.kind {
            OpenFileKind::Vfs(node_ref) => {
                let node = node_ref.lock();
                let metadata = node.metadata.clone();

                user_stat.st_size = metadata.size as i64;
                user_stat.st_mode = metadata.mode as u32;
                user_stat.st_uid = node.metadata.uid;
                user_stat.st_gid = node.metadata.gid;
                user_stat.st_ino = metadata.ino as u64;
                user_stat.st_nlink = 1;
                user_stat.st_rdev = 0;
                user_stat.st_blksize = 2048;
                user_stat.st_blocks = ((metadata.size as u64 + 511) / 512) as i64;
                user_stat.st_atim = Timespec {
                    tv_sec: metadata.access_time as i64,
                    tv_nsec: 0,
                };
                user_stat.st_mtim = Timespec {
                    tv_sec: metadata.modified_time as i64,
                    tv_nsec: 0,
                };
                user_stat.st_ctim = Timespec {
                    tv_sec: metadata.created_time as i64,
                    tv_nsec: 0,
                };
            }
            OpenFileKind::Pipe(pipe) => {
                let now = uptime() as i64;
                user_stat.st_mode = 0o010600;
                user_stat.st_uid = 0;
                user_stat.st_gid = 0;
                user_stat.st_ino = pipe.id() as u64;
                user_stat.st_nlink = 1;
                user_stat.st_size = 0;
                user_stat.st_rdev = 0;
                user_stat.st_blksize = crate::sys::fs::pipe::PIPE_BUF as i64;
                user_stat.st_blocks = 0;
                let ts = Timespec {
                    tv_sec: now,
                    tv_nsec: 0,
                };
                user_stat.st_atim = ts;
                user_stat.st_mtim = ts;
                user_stat.st_ctim = ts;
            }
            OpenFileKind::Socket(_) => {
                let now = uptime() as i64;
                user_stat.st_mode = 0o140777; // S_IFSOCK | rwxrwxrwx
                user_stat.st_uid = 0;
                user_stat.st_gid = 0;
                user_stat.st_ino = fd as u64;
                user_stat.st_nlink = 1;
                user_stat.st_size = 0;
                user_stat.st_rdev = 0;
                user_stat.st_blksize = 4096;
                user_stat.st_blocks = 0;
                let ts = Timespec {
                    tv_sec: now,
                    tv_nsec: 0,
                };
                user_stat.st_atim = ts;
                user_stat.st_mtim = ts;
                user_stat.st_ctim = ts;
            }
        }
    }

    0
}

pub fn statfs(path_ptr: usize, statfs_ptr: usize) -> i64 {
    if statfs_ptr == 0 {
        return -(EFAULT as i64);
    }
    let Ok(path) = copy_cstr_from_user(UserPtr(path_ptr as *const u8), 4096) else {
        return -(EFAULT as i64);
    };
    if path.is_empty() {
        return -(ENOENT as i64);
    }

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return -(ESRCH as i64);
    };
    let full_path = if path.starts_with('/') {
        normalize_path(&path)
    } else {
        normalize_path(&join_paths(&process.pwd, &path))
    };

    #[allow(static_mut_refs)]
    let vfs = unsafe { VFS.get_mut() };
    if vfs.metadata(&full_path).is_err() {
        return -(ENOENT as i64);
    }
    let Ok(stats) = vfs.stats(&full_path) else {
        return -(EIO as i64);
    };
    fill_statfs(unsafe { &mut *(statfs_ptr as *mut StatFs) }, stats);
    0
}

pub fn fstatfs(fd: i32, statfs_ptr: usize) -> i64 {
    if statfs_ptr == 0 {
        return -(EFAULT as i64);
    }
    if fd < 0 {
        return -(EBADF as i64);
    }

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return -(ESRCH as i64);
    };

    let file_ref = match clone_open_file(process, fd) {
        Ok(file) => file,
        Err(code) => return code as i64,
    };
    let path = {
        let file = file_ref.lock();
        if !matches!(file.kind, OpenFileKind::Vfs(_)) {
            return -(EBADF as i64);
        }
        file.path.clone()
    };

    #[allow(static_mut_refs)]
    let Ok(stats) = (unsafe { VFS.get_mut().stats(&path) }) else {
        return -(EIO as i64);
    };
    fill_statfs(unsafe { &mut *(statfs_ptr as *mut StatFs) }, stats);
    0
}

pub fn getcwd(buf_ptr: usize, buf_len: usize) -> i64 {
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) };
    #[allow(static_mut_refs)]
    let proc = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    buf.fill(0);
    let cwd = proc.pwd.as_str();
    let cwd_bytes = cwd.as_bytes();
    buf[..cwd_bytes.len()].copy_from_slice(cwd_bytes);

    buf.as_ptr() as i64
}

pub fn chdir(path_ptr: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_ptr as *const u8), 4096) else {
        return -1;
    };

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    let dir_path = if path.starts_with('/') {
        normalize_path(path.as_str())
    } else {
        normalize_path(&join_paths(process.pwd.as_str(), path.as_str()))
    };
    if let Err(e) = check_encrypted_home_access(&dir_path) {
        return e;
    }

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(inode) = fs.open(dir_path.as_str()) {
        if inode.metadata.file_type != FileType::Dir {
            return -1;
        }

        process.pwd = dir_path;
        0
    } else {
        -1
    }
}

pub fn rename(old_path_ptr: usize, new_path_ptr: usize) -> i64 {
    let Ok(old_path) = copy_cstr_from_user(UserPtr(old_path_ptr as *const u8), 4096) else {
        return -(EFAULT as i64);
    };
    let Ok(new_path) = copy_cstr_from_user(UserPtr(new_path_ptr as *const u8), 4096) else {
        return -(EFAULT as i64);
    };

    if old_path.is_empty() || new_path.is_empty() {
        return -(ENOENT as i64);
    }

    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    let old_full_path = if old_path.starts_with('/') {
        normalize_path(old_path.as_str())
    } else {
        normalize_path(&join_paths(process.pwd.as_str(), old_path.as_str()))
    };
    let new_full_path = if new_path.starts_with('/') {
        normalize_path(new_path.as_str())
    } else {
        normalize_path(&join_paths(process.pwd.as_str(), new_path.as_str()))
    };

    if old_full_path == "/" || new_full_path == "/" {
        return -(EINVAL as i64);
    }
    if old_full_path == new_full_path {
        return 0;
    }
    if let Err(e) = check_encrypted_home_access(&old_full_path) {
        return e;
    }
    if let Err(e) = check_encrypted_home_access(&new_full_path) {
        return e;
    }

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if fs.open(old_full_path.as_str()).is_err() {
        return -(ENOENT as i64);
    }

    if fs
        .rename(old_full_path.as_str(), new_full_path.as_str())
        .is_ok()
    {
        0
    } else {
        -(EIO as i64)
    }
}

pub fn unlink(path_ptr: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_ptr as *const u8), 4096) else {
        return -1;
    };

    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    // Resolve full path
    let full_path = if path.starts_with('/') {
        normalize_path(path.as_str())
    } else {
        format!("{}/{}", process.pwd.as_str(), path)
    };

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(mut inode) = fs.open(full_path.as_str()) {
        inode.unlink().unwrap() as i64
    } else {
        0
    }
}

pub fn lseek(fd: usize, offset: u64, whence: u8) -> i64 {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    let file_ref = match clone_open_file(process, fd as i32) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    match &file.kind {
        OpenFileKind::Vfs(node_ref)
            if matches!(node_ref.lock().metadata.file_type, FileType::CharDevice) =>
        {
            return -(ESPIPE as i64);
        }
        OpenFileKind::Pipe(_) | OpenFileKind::Socket(_) => return -(ESPIPE as i64),
        OpenFileKind::Vfs(_) => {}
    }

    match whence {
        0 => {
            file.seek = offset as usize;
            file.seek as i64
        }
        1 => file.seek as i64,
        2 => match &file.kind {
            OpenFileKind::Vfs(node_ref) => {
                let size = node_ref.lock().metadata.size;
                file.seek = size;
                file.seek as i64
            }
            OpenFileKind::Pipe(_) | OpenFileKind::Socket(_) => unreachable!(),
        },
        _ => -(EINVAL as i64),
    }
}

pub fn readv(fd: usize, iov_ptr: u64, iov_count: u64) -> i64 {
    if iov_count > 0 && iov_ptr == 0 {
        return -(EFAULT as i64);
    }
    let iov = unsafe { core::slice::from_raw_parts(iov_ptr as *const Iovec, iov_count as usize) };
    let total_len = match iov
        .iter()
        .try_fold(0usize, |total, item| total.checked_add(item.iov_len))
    {
        Some(total) => total,
        None => return -(EINVAL as i64),
    };
    if total_len == 0 {
        return 0;
    }

    let mut data = vec![0u8; total_len];
    let result = read(fd, &mut data);
    if result <= 0 {
        return result;
    }

    let mut copied = 0usize;
    for item in iov {
        if item.iov_len == 0 {
            continue;
        }
        if item.iov_base.is_null() {
            return -(EFAULT as i64);
        }
        let count = item.iov_len.min(result as usize - copied);
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr().add(copied),
                item.iov_base as *mut u8,
                count,
            );
        }
        copied += count;
        if copied == result as usize {
            break;
        }
    }

    result
}

pub fn preadv(fd: i32, iov_ptr: usize, iov_count: usize, offset: u64) -> i64 {
    if iov_count == 0 {
        return 0;
    }
    if iov_ptr == 0 {
        return -(EFAULT as i64);
    }
    let iov = unsafe { core::slice::from_raw_parts(iov_ptr as *const Iovec, iov_count) };

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };

    let file = file_ref.lock();
    let accmode = file.status_flags & O_ACCMODE;
    if accmode == O_WRONLY {
        return -(EBADF as i64);
    }

    match &file.kind {
        OpenFileKind::Vfs(node_ref) => {
            let mut node = node_ref.lock();
            if node.metadata.file_type == FileType::Dir {
                return -(EISDIR as i64);
            }
            if node.metadata.file_type == FileType::CharDevice {
                return -(ESPIPE as i64);
            }

            let mut total: usize = 0;
            for iv in iov {
                if iv.iov_len == 0 {
                    continue;
                }
                if iv.iov_base.is_null() {
                    return -(EFAULT as i64);
                }
                let buf =
                    unsafe { core::slice::from_raw_parts_mut(iv.iov_base as *mut u8, iv.iov_len) };
                match node.read((offset as usize).saturating_add(total), buf) {
                    Ok(n) => {
                        total = total.saturating_add(n);
                        if n < iv.iov_len {
                            break;
                        }
                    }
                    Err(_) => return -(EIO as i64),
                }
            }

            total as i64
        }
        OpenFileKind::Pipe(_) => -(ESPIPE as i64),
        OpenFileKind::Socket(_) => -(ESPIPE as i64),
    }
}

pub fn pwritev(fd: i32, iov_ptr: usize, iov_count: usize, offset: u64) -> i64 {
    if iov_count == 0 {
        return 0;
    }
    if iov_ptr == 0 {
        return -(EFAULT as i64);
    }
    let iov = unsafe { core::slice::from_raw_parts(iov_ptr as *const Iovec, iov_count) };

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };

    let file = file_ref.lock();
    let accmode = file.status_flags & O_ACCMODE;
    if accmode == O_RDONLY {
        return -(EBADF as i64);
    }

    match &file.kind {
        OpenFileKind::Vfs(node_ref) => {
            let mut node = node_ref.lock();
            if node.metadata.file_type == FileType::Dir {
                return -(EISDIR as i64);
            }
            if node.metadata.file_type == FileType::CharDevice {
                return -(ESPIPE as i64);
            }

            let mut total: usize = 0;
            for iv in iov {
                if iv.iov_len == 0 {
                    continue;
                }
                if iv.iov_base.is_null() {
                    return -(EFAULT as i64);
                }
                let buf =
                    unsafe { core::slice::from_raw_parts(iv.iov_base as *const u8, iv.iov_len) };
                let start = (offset as usize).saturating_add(total);
                let end = start.saturating_add(iv.iov_len);
                match node.write(start, buf) {
                    Ok(()) => {
                        total = total.saturating_add(iv.iov_len);
                        if end > node.metadata.size {
                            node.metadata.size = end;
                        }
                    }
                    Err(_) => return -(EIO as i64),
                }
            }

            // Do not change `file.seek` (pwritev semantics).
            total as i64
        }
        OpenFileKind::Pipe(_) => -(ESPIPE as i64),
        OpenFileKind::Socket(_) => -(ESPIPE as i64),
    }
}

pub fn ioctl(fd: usize, cmd: usize, arg: usize) -> i64 {
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap_unchecked()
            .get_process(crate::sys::proc::id())
            .unwrap_unchecked()
    };

    let file_ref = match clone_open_file(process, fd as i32) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };

    let mut file = file_ref.lock();
    match &mut file.kind {
        OpenFileKind::Vfs(node_ref) => node_ref
            .lock()
            .ioctl(cmd as u64, arg)
            .unwrap_or(-(ENOTTY as i64)),
        OpenFileKind::Pipe(_) => -(ENOTTY as i64),
        OpenFileKind::Socket(_) => -(ENOTTY as i64),
    }
}

pub fn utimenat(dirfd: i32, str_ptr: usize, _time_ptr: usize, _flags: usize) -> i64 {
    if dirfd != -100 {
        return -1;
    }

    let usr_ptr = UserPtr(str_ptr as *const u8);

    let Ok(path) = copy_cstr_from_user(usr_ptr, 4096) else {
        return -(EFAULT as i64);
    };

    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut_unchecked()
            .get_process(crate::sys::proc::id())
            .unwrap()
    };

    let can_path = if path.starts_with("/") {
        path
    } else {
        format!("{}/{}", process.pwd, path)
    };

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    let Ok(_node) = fs.open(can_path.as_str()) else {
        return -ENOENT as i64;
    };

    0
}

pub fn mkdir(path_str: usize, mode: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_str as *const u8), 4096) else {
        return -(EFAULT as i64);
    };

    let can_path = normalize_path(&format_path(path));
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return -(ESRCH as i64);
    };
    let creation_mode = apply_umask(mode as u32, process.umask);

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(_node) = fs.open(can_path.as_str()) {
        return -EEXIST as i64;
    };

    let trimmed = if can_path != "/" {
        can_path.trim_end_matches('/')
    } else {
        can_path.as_str()
    };
    let parent_path = parent_path(trimmed);
    let dir_name = trimmed.rsplit('/').next().unwrap_or("");
    if dir_name.is_empty() {
        return -(EINVAL as i64);
    }

    if let Ok(_) = fs.mkdir(parent_path, dir_name, creation_mode) {
        0
    } else {
        -(EIO as i64)
    }
}

pub fn mkdirat(dirfd: i32, path_ptr: usize, mode: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_ptr as *const u8), 4096) else {
        return -(EFAULT as i64);
    };
    if path.is_empty() {
        return -(ENOENT as i64);
    }

    // For now, support the most common libc usage: mkdirat(AT_FDCWD, ...).
    // If path is absolute, dirfd is ignored.
    let full = if path.starts_with('/') {
        path
    } else if dirfd == twilight_common::syscall::types::AT_FDCWD {
        format_path(path)
    } else {
        return -(ENOSYS as i64);
    };

    let can_path = normalize_path(&full);
    #[allow(static_mut_refs)]
    let process = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = process else {
        return -(ESRCH as i64);
    };
    let creation_mode = apply_umask(mode as u32, process.umask);

    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(_node) = fs.open(can_path.as_str()) {
        return -(EEXIST as i64);
    };

    let trimmed = if can_path != "/" {
        can_path.trim_end_matches('/')
    } else {
        can_path.as_str()
    };
    let parent_path = parent_path(trimmed);
    let dir_name = trimmed.rsplit('/').next().unwrap_or("");
    if dir_name.is_empty() {
        return -(EINVAL as i64);
    }

    if fs.mkdir(parent_path, dir_name, creation_mode).is_ok() {
        0
    } else {
        -(EIO as i64)
    }
}

pub fn rmdir(path_str: usize) -> i64 {
    let Ok(path) = copy_cstr_from_user(UserPtr(path_str as *const u8), 4096) else {
        return -1;
    };
    let can_path = format_path(path);
    #[allow(static_mut_refs)]
    let fs = unsafe { VFS.get_mut() };

    if let Ok(_) = fs.rmdir(can_path.as_str()) {
        0
    } else {
        -1
    }
}

pub fn setuid(uid: u64) -> i64 {
    sys::proc::user::set_uid(uid as usize);
    sys::proc::user::set_user_env();

    0
}

pub fn geteuid() -> i64 {
    sys::proc::user::get_uid() as i64
}

pub fn setgid(gid: u64) -> i64 {
    sys::proc::user::set_gid(gid as usize);
    0
}

pub fn getegid() -> i64 {
    sys::proc::user::get_gid() as i64
}

fn poll_fd_set(fds: &mut [PollFd], process: &mut Process) -> Result<usize, i64> {
    let mut ready_count = 0;

    for pfd in fds.iter_mut() {
        pfd.revents = 0;
        let fd = pfd.fd;
        if fd < 0 {
            continue;
        }

        let want_in = (pfd.events & POLLIN) != 0;
        let want_out = (pfd.events & POLLOUT) != 0;
        let mut revents: i16 = 0;

        let file_ref = match clone_open_file(process, fd) {
            Ok(file) => file,
            Err(_) => {
                pfd.revents = POLLNVAL;
                ready_count += 1;
                continue;
            }
        };
        let mut file = file_ref.lock();
        match &mut file.kind {
            OpenFileKind::Vfs(node_ref) => {
                if want_in {
                    match node_ref.lock().poll() {
                        Ok(true) => revents |= POLLIN,
                        Ok(false) => {}
                        Err(_) => revents |= POLLERR,
                    }
                }
                if want_out && file.status_flags & O_ACCMODE != O_RDONLY {
                    revents |= POLLOUT;
                }
            }
            OpenFileKind::Pipe(pipe) => {
                let state = pipe.poll();
                if want_in && state.readable {
                    revents |= POLLIN;
                }
                if want_out && state.writable {
                    revents |= POLLOUT;
                }
                if state.hangup {
                    revents |= POLLHUP;
                }
                if state.error {
                    revents |= POLLERR;
                }
            }
            OpenFileKind::Socket(sock) => {
                if want_in && sock.poll(IO::Read) {
                    revents |= POLLIN;
                }
                if want_out && sock.poll(IO::Write) {
                    revents |= POLLOUT;
                }
            }
        }

        if revents != 0 {
            pfd.revents = revents;
            ready_count += 1;
        }
    }

    Ok(ready_count)
}

fn poll_fd_set_for_pid(fds: &mut [PollFd], pid: u16) -> Result<usize, i64> {
    #[allow(static_mut_refs)]
    let proc_opt = unsafe { PROCESS_TABLE.get_mut().unwrap().get_process(pid) };
    let Some(process) = proc_opt else {
        return Err(-(ESRCH as i64));
    };

    poll_fd_set(fds, process)
}

pub fn poll(fds_ptr: usize, nfds: usize, timeout_ms: isize) -> i64 {
    if nfds == 0 {
        return 0;
    }
    if fds_ptr == 0 {
        return -(EFAULT as i64);
    }

    let fds = unsafe { core::slice::from_raw_parts_mut(fds_ptr as *mut PollFd, nfds) };
    let current_pid = sys::proc::id();

    let mut ready = match poll_fd_set_for_pid(fds, current_pid) {
        Ok(n) => n,
        Err(e) => return e,
    };

    if ready > 0 {
        return ready as i64;
    }
    if timeout_ms == 0 {
        return 0;
    }

    let infinite = timeout_ms < 0;
    let start = uptime();
    let deadline = if infinite {
        None
    } else {
        Some(start + (timeout_ms as f64) / 1000.0)
    };
    let wait_queue = sys::proc::poll_wait_queue();

    loop {
        if let Some(limit) = deadline {
            if uptime() >= limit {
                return 0;
            }
        }

        let wait_pid = wait_queue.prepare_current();

        ready = match poll_fd_set_for_pid(fds, current_pid) {
            Ok(n) => n,
            Err(e) => {
                wait_queue.finish_wait(wait_pid);
                return e;
            }
        };

        if ready > 0 {
            wait_queue.finish_wait(wait_pid);
            return ready as i64;
        }

        if let Some(limit) = deadline {
            if uptime() >= limit {
                wait_queue.finish_wait(wait_pid);
                return 0;
            }
        }

        sys::proc::await_io();
        wait_queue.finish_wait(wait_pid);

        ready = match poll_fd_set_for_pid(fds, current_pid) {
            Ok(n) => n,
            Err(e) => return e,
        };

        if ready > 0 {
            return ready as i64;
        }
    }
}

pub fn ppoll(
    fds_ptr: usize,
    nfds: usize,
    tmo_p: usize,
    _sigmask_ptr: usize,
    _sigsetsize: usize,
) -> i64 {
    if nfds == 0 {
        return 0;
    }
    if fds_ptr == 0 {
        return -(EFAULT as i64);
    }

    let fds = unsafe { core::slice::from_raw_parts_mut(fds_ptr as *mut PollFd, nfds) };
    let current_pid = sys::proc::id();

    let mut ready = match poll_fd_set_for_pid(fds, current_pid) {
        Ok(n) => n,
        Err(e) => return e,
    };

    if ready > 0 {
        return ready as i64;
    }

    let deadline = if tmo_p != 0 {
        let ts_ptr = tmo_p as *const Timespec;
        let ts = unsafe { &*ts_ptr };
        if ts.tv_sec == 0 && ts.tv_nsec == 0 {
            return 0;
        }
        let now = uptime();
        let dur = (ts.tv_sec as f64) + (ts.tv_nsec as f64) / 1_000_000_000.0;
        Some(now + dur)
    } else {
        None
    };
    let wait_queue = sys::proc::poll_wait_queue();

    loop {
        if let Some(limit) = deadline {
            if uptime() >= limit {
                return 0;
            }
        }

        // Check for signals here if we implemented them?
        // But for now just poll fds.

        let wait_pid = wait_queue.prepare_current();

        ready = match poll_fd_set_for_pid(fds, current_pid) {
            Ok(n) => n,
            Err(e) => {
                wait_queue.finish_wait(wait_pid);
                return e;
            }
        };

        if ready > 0 {
            wait_queue.finish_wait(wait_pid);
            return ready as i64;
        }

        if let Some(limit) = deadline {
            if uptime() >= limit {
                wait_queue.finish_wait(wait_pid);
                return 0;
            }
        }

        sys::proc::await_io();
        wait_queue.finish_wait(wait_pid);

        ready = match poll_fd_set_for_pid(fds, current_pid) {
            Ok(n) => n,
            Err(e) => return e,
        };

        if ready > 0 {
            return ready as i64;
        }
    }
}

pub fn socket(domain: i32, sock_type: i32, _protocol: i32) -> i64 {
    const SOCK_NONBLOCK: i32 = 0x800;
    const SOCK_CLOEXEC: i32 = 0x80000;

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    if domain as u16 != AF_INET {
        return -(EAFNOSUPPORT as i64);
    }

    let nonblock = (sock_type & SOCK_NONBLOCK) != 0;
    let cloexec = (sock_type & SOCK_CLOEXEC) != 0;
    let base_type = sock_type & 0xF;

    let sock = match base_type {
        SOCK_STREAM => SocketFile::Tcp(TcpSocket::new()),
        SOCK_DGRAM => SocketFile::Udp(UdpSocket::new()),
        _ => return -(EPROTONOSUPPORT as i64),
    };

    let open_file = OpenFile {
        kind: OpenFileKind::Socket(sock),
        seek: 0,
        path: "socket".to_string(),
        status_flags: O_RDWR | if nonblock { O_NONBLOCK } else { 0 },
    };
    let entry = FdEntry {
        file: Arc::new(Mutex::new(open_file)),
        fd_flags: if cloexec { FD_CLOEXEC } else { 0 },
    };

    match install_fd_entry(process, entry, 0) {
        Ok(fd) => fd as i64,
        Err(code) => -(code as i64),
    }
}

pub fn connect(fd: i32, addr_ptr: usize, addr_len: usize) -> i64 {
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }
    let ep = match parse_sockaddr_in(addr_ptr, addr_len) {
        Ok(ep) => ep,
        Err(e) => return -(e as i64),
    };

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };

    match sock.connect(ep.addr, ep.port) {
        Ok(()) => 0,
        Err(_) => -(ETIMEDOUT as i64),
    }
}

pub fn bind(fd: i32, addr_ptr: usize, addr_len: usize) -> i64 {
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }
    let ep = match parse_sockaddr_in(addr_ptr, addr_len) {
        Ok(ep) => ep,
        Err(e) => return -(e as i64),
    };

    let port = if ep.port == 0 {
        random_ephemeral_port()
    } else {
        ep.port
    };

    #[allow(static_mut_refs)]
    if unsafe { GLOBAL_PORT_MAP.lock().contains_key(&port.clone()) } {
        return -1;
    }

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };

    #[allow(static_mut_refs)]
    unsafe {
        GLOBAL_PORT_MAP.lock().insert(port, process.pid);
    }

    match sock.bind(port) {
        Ok(()) => 0,
        Err(_) => -(EADDRINUSE as i64),
    }
}

pub fn listen(fd: i32, _backlog: i32) -> i64 {
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };

    let port = match sock {
        SocketFile::Tcp(t) => t.bound_port.unwrap_or(0),
        SocketFile::Udp(_) => return -(EOPNOTSUPP as i64),
    };
    if port == 0 {
        return -(EINVAL as i64);
    }

    match sock.listen(port) {
        Ok(()) => 0,
        Err(_) => -(EIO as i64),
    }
}

pub fn accept(fd: i32, addr_ptr: usize, addrlen_ptr: usize) -> i64 {
    accept4(fd, addr_ptr, addrlen_ptr, 0)
}

pub fn accept4(fd: i32, addr_ptr: usize, addrlen_ptr: usize, flags: i32) -> i64 {
    const SOCK_NONBLOCK: i32 = 0x800;
    const SOCK_CLOEXEC: i32 = 0x80000;

    if fd < 0 {
        return -(ENOTSOCK as i64);
    }

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let status_flags = file.status_flags;
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };

    let nonblock = (status_flags & O_NONBLOCK) != 0 || (flags & SOCK_NONBLOCK) != 0;
    let res = if nonblock {
        match sock.try_accept_new() {
            Ok(Some(v)) => Ok(v),
            Ok(None) => Err(-(EAGAIN as i64)),
            Err(_) => Err(-(EIO as i64)),
        }
    } else {
        sock.accept_new().map_err(|_| -(EAGAIN as i64))
    };

    let (new_sock, peer) = match res {
        Ok(v) => v,
        Err(e) => return e,
    };

    if let Err(e) = write_sockaddr_in(addr_ptr, addrlen_ptr, peer) {
        return e;
    }

    let cloexec = (flags & SOCK_CLOEXEC) != 0;
    let open_file = OpenFile {
        kind: OpenFileKind::Socket(new_sock),
        seek: 0,
        path: "socket".to_string(),
        status_flags: O_RDWR | if nonblock { O_NONBLOCK } else { 0 },
    };
    let entry = FdEntry {
        file: Arc::new(Mutex::new(open_file)),
        fd_flags: if cloexec { FD_CLOEXEC } else { 0 },
    };

    let new_fd = match install_fd_entry(process, entry, 0) {
        Ok(fd) => fd,
        Err(code) => return -(code as i64),
    };

    new_fd as i64
}

pub fn sendto(
    fd: i32,
    buf_ptr: usize,
    len: usize,
    flags: i32,
    addr_ptr: usize,
    addr_len: usize,
) -> i64 {
    if buf_ptr == 0 && len != 0 {
        return -(EFAULT as i64);
    }
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }

    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, len) };

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let nonblock = (file.status_flags & O_NONBLOCK) != 0 || (flags & MSG_DONTWAIT) != 0;
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };
    if nonblock && !sock.poll(IO::Write) {
        return -(EAGAIN as i64);
    }

    let dest = if addr_ptr != 0 {
        match parse_sockaddr_in(addr_ptr, addr_len) {
            Ok(ep) => Some(ep),
            Err(e) => return -(e as i64),
        }
    } else {
        None
    };

    let res = match dest {
        Some(ep) => sock.send_to(buf, ep),
        None => sock.write(buf),
    };

    match res {
        Ok(n) => n as i64,
        Err(_) => -(EDESTADDRREQ as i64),
    }
}

pub fn recvfrom(
    fd: i32,
    buf_ptr: usize,
    len: usize,
    flags: i32,
    addr_ptr: usize,
    addrlen_ptr: usize,
) -> i64 {
    if buf_ptr == 0 && len != 0 {
        return -(EFAULT as i64);
    }
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }

    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, len) };

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let nonblock = (file.status_flags & O_NONBLOCK) != 0 || (flags & MSG_DONTWAIT) != 0;
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };
    if nonblock && !sock.poll(IO::Read) {
        return -(EAGAIN as i64);
    }

    let (n, src) = match sock {
        SocketFile::Udp(u) => match u.recv_from(buf) {
            Ok((n, ep)) => (n, Some(ep)),
            Err(_) => return -(EIO as i64),
        },
        _ => match sock.read(buf) {
            Ok(n) => (n, None),
            Err(_) => return -(EIO as i64),
        },
    };

    if let Some(src) = src {
        if let Err(e) = write_sockaddr_in(addr_ptr, addrlen_ptr, src) {
            return e;
        }
    }

    n as i64
}

pub fn shutdown(fd: i32, _how: i32) -> i64 {
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };
    sock.close();
    0
}

pub fn setsockopt(fd: i32, level: i32, optname: i32, _optval: usize, _optlen: usize) -> i64 {
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let file = file_ref.lock();
    if !matches!(&file.kind, OpenFileKind::Socket(_)) {
        return -(ENOTSOCK as i64);
    }
    if level == SOL_SOCKET && optname == SO_REUSEADDR {
        return 0;
    }
    -(ENOPROTOOPT as i64)
}

pub fn getsockopt(fd: i32, _level: i32, _optname: i32, _optval: usize, _optlen_ptr: usize) -> i64 {
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let file = file_ref.lock();
    if !matches!(&file.kind, OpenFileKind::Socket(_)) {
        return -(ENOTSOCK as i64);
    }
    -(ENOPROTOOPT as i64)
}

pub fn getsockname(fd: i32, addr_ptr: usize, addrlen_ptr: usize) -> i64 {
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };

    let ep = match sock {
        SocketFile::Tcp(t) => t.local_endpoint().unwrap_or(IpEndpoint::new(
            IpAddress::Ipv4(Ipv4Address::UNSPECIFIED),
            t.bound_port.unwrap_or(0),
        )),
        SocketFile::Udp(u) => {
            let port = u.local_port().or(u.bound_port).unwrap_or(0);
            IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::UNSPECIFIED), port)
        }
    };

    match write_sockaddr_in(addr_ptr, addrlen_ptr, ep) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub fn getpeername(fd: i32, addr_ptr: usize, addrlen_ptr: usize) -> i64 {
    if fd < 0 {
        return -(ENOTSOCK as i64);
    }

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };
    let file_ref = match clone_open_file(process, fd) {
        Ok(f) => f,
        Err(code) => return code as i64,
    };
    let mut file = file_ref.lock();
    let OpenFileKind::Socket(sock) = &mut file.kind else {
        return -(ENOTSOCK as i64);
    };

    let peer = match sock {
        SocketFile::Tcp(t) => t.remote_endpoint().ok_or(-(ENOTCONN as i64)),
        SocketFile::Udp(u) => u.remote_endpoint().ok_or(-(ENOTCONN as i64)),
    };
    let peer = match peer {
        Ok(ep) => ep,
        Err(e) => return e,
    };

    match write_sockaddr_in(addr_ptr, addrlen_ptr, peer) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

pub fn getpid() -> i64 {
    crate::sys::proc::id() as i64
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rusage {
    ru_utime: Timeval,
    ru_stime: Timeval,
    ru_maxrss: i64,
    ru_ixrss: i64,
    ru_idrss: i64,
    ru_isrss: i64,
    ru_minflt: i64,
    ru_majflt: i64,
    ru_nswap: i64,
    ru_inblock: i64,
    ru_oublock: i64,
    ru_msgsnd: i64,
    ru_msgrcv: i64,
    ru_nsignals: i64,
    ru_nvcsw: i64,
    ru_nivcsw: i64,
    reserved: [i64; 16],
}

pub fn getrusage(who: i32, usage: usize) -> i64 {
    const RUSAGE_CHILDREN: i32 = -1;
    const RUSAGE_SELF: i32 = 0;
    const RUSAGE_THREAD: i32 = 1;

    if usage == 0 {
        return -(EFAULT as i64);
    }
    if !matches!(who, RUSAGE_CHILDREN | RUSAGE_SELF | RUSAGE_THREAD) {
        return -(EINVAL as i64);
    }

    unsafe {
        *(usage as *mut Rusage) = Rusage::default();
    }
    0
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxSigAction {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LinuxSigAltStack {
    ss_sp: u64,
    ss_flags: i32,
    ss_size: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UserSignalFrame {
    magic: u64,
    old_mask: [u64; 2],
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbp: u64,
    rbx: u64,
    r11: u64,
    r10: u64,
    r9: u64,
    r8: u64,
    rdi: u64,
    rsi: u64,
    rdx: u64,
    rcx: u64,
    rax: u64,
    rip: u64,
    rsp: u64,
    rflags: u64,
}

const SIGNAL_FRAME_MAGIC: u64 = 0x5457_5349_4746_524d;
const SIG_IGN: u64 = 1;
const SIGHUP: usize = 1;
const SIGINT: usize = 2;
const SIGQUIT: usize = 3;
const SIGALRM: usize = 14;
const SIGTERM: usize = 15;
const SIGCONT: usize = 18;
const SIGTSTP: usize = 20;
const SIGTTIN: usize = 21;
const SIGTTOU: usize = 22;
const SA_NODEFER: u64 = 0x4000_0000;
const SS_DISABLE: i32 = 2;

fn linux_sigaltstack_from(stack: SignalAltStack) -> LinuxSigAltStack {
    LinuxSigAltStack {
        ss_sp: stack.sp,
        ss_flags: stack.flags,
        ss_size: stack.size,
    }
}

fn signal_altstack_from(stack: LinuxSigAltStack) -> SignalAltStack {
    if (stack.ss_flags & SS_DISABLE) != 0 {
        SignalAltStack::default()
    } else {
        SignalAltStack {
            sp: stack.ss_sp,
            flags: 0,
            size: stack.ss_size,
        }
    }
}

pub fn sigaltstack(new_stack: usize, old_stack: usize) -> i64 {
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    if old_stack != 0 {
        unsafe {
            *(old_stack as *mut LinuxSigAltStack) =
                linux_sigaltstack_from(process.signal_alt_stack);
        }
    }
    if new_stack != 0 {
        let stack = unsafe { *(new_stack as *const LinuxSigAltStack) };
        if stack.ss_flags & !SS_DISABLE != 0 {
            return -(EINVAL as i64);
        }
        process.signal_alt_stack = signal_altstack_from(stack);
    }

    0
}

fn valid_signal(sig: i32) -> bool {
    (0..=64).contains(&sig)
}

fn is_fatal_default(sig: usize) -> bool {
    matches!(
        sig,
        SIGHUP | SIGINT | SIGQUIT | SIGKILL | SIGPIPE | SIGALRM | SIGTERM
    )
}

fn is_stop_default(sig: usize) -> bool {
    matches!(sig, SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU)
}

fn linux_sigaction_from(action: SignalAction) -> LinuxSigAction {
    LinuxSigAction {
        handler: action.handler,
        flags: action.flags,
        restorer: action.restorer,
        mask: action.mask[0],
    }
}

fn signal_action_from(action: LinuxSigAction) -> SignalAction {
    SignalAction {
        handler: action.handler,
        mask: [action.mask & !signal_bit(SIGKILL) & !signal_bit(SIGSTOP), 0],
        flags: action.flags,
        restorer: action.restorer,
    }
}

fn notify_parent_for_child_event(processes: &mut [Process], parent_pid: u16) {
    if let Some(parent) = processes.iter_mut().find(|p| p.pid == parent_pid) {
        parent.queue_signal(SIGCHLD);
    }
}

fn apply_signal_to_process(process: &mut Process, sig: usize) -> Option<u16> {
    if sig == 0 {
        return None;
    }

    if sig == SIGCONT {
        if matches!(process.state, crate::sys::proc::ProcessState::Stopped) {
            process.state = crate::sys::proc::ProcessState::Running;
            process.wait_status = 0xffff;
            process.wait_reported = false;
        }
        process.queue_signal(sig);
        return Some(process.parent_pid);
    }

    let action = process.signal_actions[sig];
    if action.handler == SIG_IGN && sig != SIGKILL && sig != SIGSTOP {
        return None;
    }
    if action.handler > SIG_IGN {
        process.queue_signal(sig);
        return None;
    }

    if is_stop_default(sig) {
        process.state = crate::sys::proc::ProcessState::Stopped;
        process.wait_status = ((sig as i32) << 8) | 0x7f;
        process.wait_reported = false;
        return Some(process.parent_pid);
    }

    if is_fatal_default(sig) {
        process.close_all_fds();
        process.state = crate::sys::proc::ProcessState::Dead;
        process.wait_status = sig as i32 & 0x7f;
        process.wait_reported = false;
        return Some(process.parent_pid);
    }

    process.queue_signal(sig);
    None
}

pub fn kill(pid: i32, sig: i32) -> i64 {
    if !valid_signal(sig) {
        return -(EINVAL as i64);
    }

    let current_pid = crate::sys::proc::id();
    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    let current_pgid = table
        .proc_list
        .iter()
        .find(|p| p.pid == current_pid)
        .map(|p| p.pgid)
        .unwrap_or(current_pid);
    let target_pgid = if pid < -1 {
        match pid.checked_neg() {
            Some(pgid) if pgid <= u16::MAX as i32 => Some(pgid as u16),
            _ => return -(EINVAL as i64),
        }
    } else {
        None
    };

    let targets = table
        .proc_list
        .iter()
        .filter(|p| match pid {
            n if n > 0 && n <= u16::MAX as i32 => p.pid == n as u16,
            n if n > 0 => false,
            0 => p.pgid == current_pgid,
            -1 => p.pid > 1,
            _ => p.pgid == target_pgid.unwrap(),
        })
        .map(|p| p.pid)
        .collect::<Vec<u16>>();

    if targets.is_empty() {
        return -(ESRCH as i64);
    }
    if sig == 0 {
        return 0;
    }

    let mut parents_to_notify = Vec::new();
    let slice = table.proc_list.make_contiguous();
    for target in targets {
        if let Some(process) = slice.iter_mut().find(|p| p.pid == target) {
            if let Some(parent_pid) = apply_signal_to_process(process, sig as usize) {
                parents_to_notify.push(parent_pid);
            }
        }
    }
    for parent_pid in parents_to_notify {
        notify_parent_for_child_event(slice, parent_pid);
    }
    let current_should_yield = slice.iter().any(|p| {
        p.pid == current_pid
            && matches!(
                p.state,
                crate::sys::proc::ProcessState::Dead | crate::sys::proc::ProcessState::Stopped
            )
    });
    if current_should_yield {
        crate::sys::proc::schedule_now();
    }

    0
}

pub fn getppid() -> i64 {
    let current_pid = crate::sys::proc::id();

    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    match table.proc_list.iter().find(|p| p.pid == current_pid) {
        Some(process) => process.parent_pid as i64,
        None => -(ESRCH as i64),
    }
}

pub fn getpgrp() -> i64 {
    getpgid(0)
}

pub fn getpgid(pid: i32) -> i64 {
    if pid < 0 || pid > u16::MAX as i32 {
        return -(EINVAL as i64);
    }

    let target_pid = if pid == 0 {
        crate::sys::proc::id()
    } else {
        pid as u16
    };

    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    match table.proc_list.iter().find(|p| p.pid == target_pid) {
        Some(process) => process.pgid as i64,
        None => -(ESRCH as i64),
    }
}

pub fn setpgid(pid: i32, pgid: i32) -> i64 {
    if pid < 0 || pgid < 0 || pid > u16::MAX as i32 || pgid > u16::MAX as i32 {
        return -(EINVAL as i64);
    }

    let current_pid = crate::sys::proc::id();
    let target_pid = if pid == 0 { current_pid } else { pid as u16 };
    let new_pgid = if pgid == 0 { target_pid } else { pgid as u16 };

    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    match table.proc_list.iter_mut().find(|p| p.pid == target_pid) {
        Some(process) => {
            process.pgid = new_pgid;
            0
        }
        None => -(ESRCH as i64),
    }
}

pub fn getsid(pid: i32) -> i64 {
    if pid < 0 || pid > u16::MAX as i32 {
        return -(EINVAL as i64);
    }

    let target_pid = if pid == 0 {
        crate::sys::proc::id()
    } else {
        pid as u16
    };

    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    match table.proc_list.iter().find(|p| p.pid == target_pid) {
        Some(process) => process.sid as i64,
        None => -(ESRCH as i64),
    }
}

pub fn rt_sigaction(signum: i32, act: usize, oldact: usize, _sigsetsize: usize) -> i64 {
    if !(1..=64).contains(&signum) || signum as usize == SIGKILL || signum as usize == SIGSTOP {
        return -(EINVAL as i64);
    }

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let idx = signum as usize;
    if oldact != 0 {
        unsafe {
            *(oldact as *mut LinuxSigAction) = linux_sigaction_from(process.signal_actions[idx]);
        }
    }
    if act != 0 {
        let user_action = unsafe { *(act as *const LinuxSigAction) };
        process.signal_actions[idx] = signal_action_from(user_action);
    }

    0
}

pub fn rt_sigprocmask(how: i32, set: usize, oldset: usize, sigsetsize: usize) -> i64 {
    let copy_len = core::cmp::min(sigsetsize, core::mem::size_of::<[u64; 2]>());

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    if oldset != 0 && copy_len != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                process.signal_mask.as_ptr() as *const u8,
                oldset as *mut u8,
                copy_len,
            )
        };
    }
    if set != 0 {
        let mut new_mask = [0u64; 2];
        if copy_len != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    set as *const u8,
                    new_mask.as_mut_ptr() as *mut u8,
                    copy_len,
                )
            };
        }
        new_mask[0] &= !signal_bit(SIGKILL) & !signal_bit(SIGSTOP);
        match how {
            0 => process.signal_mask[0] |= new_mask[0],
            1 => process.signal_mask[0] &= !new_mask[0],
            2 => process.signal_mask = new_mask,
            _ => return -(EINVAL as i64),
        }
    }
    0
}

pub fn rt_sigsuspend(mask: usize, sigsetsize: usize) -> i64 {
    if mask == 0 {
        return -(EFAULT as i64);
    }
    let copy_len = core::cmp::min(sigsetsize, core::mem::size_of::<[u64; 2]>());

    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return -(ESRCH as i64);
    };

    let mut new_mask = [0u64; 2];
    if copy_len != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                mask as *const u8,
                new_mask.as_mut_ptr() as *mut u8,
                copy_len,
            )
        };
    }
    new_mask[0] &= !signal_bit(SIGKILL) & !signal_bit(SIGSTOP);
    process.sigsuspend_saved_mask = process.signal_mask;
    process.signal_mask = new_mask;
    process.in_sigsuspend = true;

    if !process.has_unblocked_signal() {
        process.state = crate::sys::proc::ProcessState::SignalWait;
        crate::sys::proc::schedule_now();
    }

    -(EINTR as i64)
}

pub fn deliver_pending_signal(
    stack_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    regs: &mut crate::arch::x86_64::idt::Registers,
) {
    #[allow(static_mut_refs)]
    let proc_opt = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_opt else {
        return;
    };
    let Some(sig) = process.next_unblocked_signal() else {
        return;
    };

    let action = process.signal_actions[sig];
    if action.handler <= SIG_IGN || action.restorer == 0 {
        process.dequeue_signal(sig);
        if process.in_sigsuspend {
            process.signal_mask = process.sigsuspend_saved_mask;
            process.in_sigsuspend = false;
        }
        return;
    }

    process.dequeue_signal(sig);
    let effective_mask = process.signal_mask;
    let old_mask = if process.in_sigsuspend {
        process.in_sigsuspend = false;
        process.sigsuspend_saved_mask
    } else {
        process.signal_mask
    };

    let old_rsp = stack_frame.stack_pointer.as_u64();
    let frame_addr = (old_rsp - core::mem::size_of::<UserSignalFrame>() as u64) & !0xf;
    let ret_slot = frame_addr - 8;
    let ret_addr = ret_slot as *mut u64;
    let frame_ptr = frame_addr as *mut UserSignalFrame;

    unsafe {
        *ret_addr = action.restorer;
        *frame_ptr = UserSignalFrame {
            magic: SIGNAL_FRAME_MAGIC,
            old_mask,
            r15: regs.r15,
            r14: regs.r14,
            r13: regs.r13,
            r12: regs.r12,
            rbp: regs.rbp,
            rbx: regs.rbx,
            r11: regs.r11,
            r10: regs.r10,
            r9: regs.r9,
            r8: regs.r8,
            rdi: regs.rdi,
            rsi: regs.rsi,
            rdx: regs.rdx,
            rcx: regs.rcx,
            rax: regs.rax,
            rip: stack_frame.instruction_pointer.as_u64(),
            rsp: old_rsp,
            rflags: stack_frame.cpu_flags.bits(),
        };
    }

    process.signal_mask = [
        effective_mask[0] | action.mask[0],
        effective_mask[1] | action.mask[1],
    ];
    if (action.flags & SA_NODEFER) == 0 {
        process.signal_mask[0] |= signal_bit(sig);
    }
    process.signal_mask[0] &= !signal_bit(SIGKILL) & !signal_bit(SIGSTOP);

    regs.rdi = sig as u64;
    regs.rsi = 0;
    regs.rdx = 0;

    unsafe {
        let frame_value =
            stack_frame as *mut _ as *mut x86_64::structures::idt::InterruptStackFrameValue;
        (*frame_value).instruction_pointer = x86_64::VirtAddr::new(action.handler);
        (*frame_value).stack_pointer = x86_64::VirtAddr::new(ret_slot);
    }
}

pub fn rt_sigreturn(
    stack_frame: &mut x86_64::structures::idt::InterruptStackFrame,
    regs: &mut crate::arch::x86_64::idt::Registers,
) -> bool {
    let frame_addr = stack_frame.stack_pointer.as_u64();
    let frame_ptr = frame_addr as *const UserSignalFrame;
    let frame = unsafe { *frame_ptr };
    if frame.magic != SIGNAL_FRAME_MAGIC {
        return false;
    }

    regs.r15 = frame.r15;
    regs.r14 = frame.r14;
    regs.r13 = frame.r13;
    regs.r12 = frame.r12;
    regs.rbp = frame.rbp;
    regs.rbx = frame.rbx;
    regs.r11 = frame.r11;
    regs.r10 = frame.r10;
    regs.r9 = frame.r9;
    regs.r8 = frame.r8;
    regs.rdi = frame.rdi;
    regs.rsi = frame.rsi;
    regs.rdx = frame.rdx;
    regs.rcx = frame.rcx;
    regs.rax = frame.rax;

    #[allow(static_mut_refs)]
    if let Some(process) = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    } {
        process.signal_mask = frame.old_mask;
        process.signal_mask[0] &= !signal_bit(SIGKILL) & !signal_bit(SIGSTOP);
    }

    unsafe {
        let frame_value =
            stack_frame as *mut _ as *mut x86_64::structures::idt::InterruptStackFrameValue;
        (*frame_value).instruction_pointer = x86_64::VirtAddr::new(frame.rip);
        (*frame_value).stack_pointer = x86_64::VirtAddr::new(frame.rsp);
        (*frame_value).cpu_flags =
            x86_64::registers::rflags::RFlags::from_bits_truncate(frame.rflags);
    }

    true
}

pub fn tgkill(tgid: i32, tid: i32, _sig: i32) -> i64 {
    let cur = crate::sys::proc::id() as i32;
    if tgid != cur || tid != cur {
        return -(ESRCH as i64);
    }
    0
}

pub fn set_robust_list(_head: usize, _len: usize) -> i64 {
    // Robust futex lists are ignored for now.
    0
}

static GETRANDOM_SEED: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags))
    };
    ((hi as u64) << 32) | (lo as u64)
}

pub fn getrandom(buf: usize, len: usize, _flags: u32) -> i64 {
    if buf == 0 && len != 0 {
        return -(EFAULT as i64);
    }
    let out = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len) };

    let now = rdtsc() ^ (crate::sys::proc::id() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut x = GETRANDOM_SEED
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v ^ now ^ (len as u64).rotate_left(17))
        })
        .unwrap_or(0)
        ^ now;

    for b in out.iter_mut() {
        // xorshift64*
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let y = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        *b = (y & 0xFF) as u8;
    }

    len as i64
}

pub fn rseq(_rseq: usize, _rseq_len: u32, _flags: u32, _sig: u32) -> i64 {
    // glibc probes this; returning ENOSYS makes it fall back.
    -(ENOSYS as i64)
}

pub fn futex(uaddr: usize, op: i32, val: u32, _timeout: usize, _uaddr2: usize, _val3: u32) -> i64 {
    const FUTEX_WAIT: i32 = 0;
    const FUTEX_WAKE: i32 = 1;
    const FUTEX_WAIT_BITSET: i32 = 9;
    const FUTEX_WAKE_BITSET: i32 = 10;
    const FUTEX_PRIVATE_FLAG: i32 = 128;
    const FUTEX_CLOCK_REALTIME: i32 = 256;

    let cmd = op & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

    if uaddr == 0 {
        return -(EFAULT as i64);
    }

    match cmd {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
            if cur != val {
                return -(EAGAIN as i64);
            }

            // A spurious wake is valid for futex waiters. Yield once so another
            // task can make progress instead of halting in kernel mode.
            if !crate::sys::proc::schedule_now() {
                halt();
            }
            0
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => 0,
        _ => -(ENOSYS as i64),
    }
}

pub fn readlink(path: usize, buf_ptr: usize, buf_len: usize) -> i64 {
    readlinkat(AT_FDCWD, path, buf_ptr, buf_len)
}

fn proc_exe_readlink_target(
    full_path: &str,
    current_pid: u16,
    current_exe: &str,
) -> Result<Option<String>, i64> {
    if full_path == "/proc/self/exe" {
        return Ok(Some(current_exe.to_string()));
    }

    let Some(rest) = full_path.strip_prefix("/proc/") else {
        return Ok(None);
    };
    let Some(pid_part) = rest.strip_suffix("/exe") else {
        return Ok(None);
    };
    if pid_part.is_empty() || pid_part.contains('/') {
        return Ok(None);
    }

    let Ok(pid) = pid_part.parse::<u16>() else {
        return Ok(None);
    };
    if pid == current_pid {
        return Ok(Some(current_exe.to_string()));
    }

    #[allow(static_mut_refs)]
    let table = unsafe { PROCESS_TABLE.get_mut().unwrap() };
    let Some(process) = table.proc_list.iter().find(|process| process.pid == pid) else {
        return Err(-(ENOENT as i64));
    };

    Ok(Some(process.exe_path.clone()))
}

pub fn readlinkat(dirfd: i32, path: usize, buf_ptr: usize, buf_len: usize) -> i64 {
    if path == 0 || buf_ptr == 0 {
        return -(EFAULT as i64);
    }
    if buf_len == 0 {
        return -(EINVAL as i64);
    }

    let current_pid = crate::sys::proc::id();
    #[allow(static_mut_refs)]
    let (full_path, current_exe) = {
        let proc_option = unsafe { PROCESS_TABLE.get_mut().unwrap().get_process(current_pid) };
        let Some(process) = proc_option else {
            return -(ESRCH as i64);
        };

        let upath = UserPtr(path as *const u8);
        let path_str = match copy_cstr_from_user(upath, 4096) {
            Ok(s) => s,
            _ => return -(EFAULT as i64),
        };

        let full_path = if path_str.starts_with('/') {
            normalize_path(&path_str)
        } else {
            match base_for_dirfd(process, dirfd) {
                Ok(base) => normalize_path(&join_paths(&base, &path_str)),
                Err(e) => return e as i64,
            }
        };

        (full_path, process.exe_path.clone())
    };

    let Some(target) = (match proc_exe_readlink_target(&full_path, current_pid, &current_exe) {
        Ok(target) => target,
        Err(code) => return code,
    }) else {
        #[allow(static_mut_refs)]
        if unsafe { VFS.get_mut().metadata(&full_path).is_err() } {
            return -(ENOENT as i64);
        }

        return -(EINVAL as i64);
    };

    let bytes = target.as_bytes();
    let copy_len = core::cmp::min(buf_len, bytes.len());
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf_ptr as *mut u8, copy_len);
    }

    copy_len as i64
}

fn listxattr_for_path(path: usize, follow_relative_to_pwd: bool) -> i64 {
    if path == 0 {
        return -(EFAULT as i64);
    }

    let path_str = match copy_cstr_from_user(UserPtr(path as *const u8), 4096) {
        Ok(s) => s,
        _ => return -(EFAULT as i64),
    };

    let full_path = if path_str.starts_with('/') || !follow_relative_to_pwd {
        normalize_path(&path_str)
    } else {
        #[allow(static_mut_refs)]
        let proc_option = unsafe {
            PROCESS_TABLE
                .get_mut()
                .unwrap()
                .get_process(crate::sys::proc::id())
        };
        let Some(process) = proc_option else {
            return -(ESRCH as i64);
        };

        normalize_path(&join_paths(&process.pwd, &path_str))
    };

    #[allow(static_mut_refs)]
    if unsafe { VFS.get_mut().metadata(&full_path).is_err() } {
        return -(ENOENT as i64);
    }

    0
}

pub fn listxattr(path: usize, _list: usize, _size: usize) -> i64 {
    listxattr_for_path(path, true)
}

pub fn llistxattr(path: usize, _list: usize, _size: usize) -> i64 {
    listxattr_for_path(path, true)
}

pub fn flistxattr(fd: i32, _list: usize, _size: usize) -> i64 {
    #[allow(static_mut_refs)]
    let proc_option = unsafe {
        PROCESS_TABLE
            .get_mut()
            .unwrap()
            .get_process(crate::sys::proc::id())
    };
    let Some(process) = proc_option else {
        return -(ESRCH as i64);
    };

    match fd_slot(process, fd) {
        Ok(_) => 0,
        Err(code) => code as i64,
    }
}
